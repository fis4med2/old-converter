use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::Mutex;

const VERSION: &str = env!("CARGO_PKG_VERSION");

type JobMap = Arc<Mutex<HashMap<String, Arc<Mutex<tokio::process::Child>>>>>;

struct AppState {
    jobs: JobMap,
}

// ---------- ffmpeg resolution ----------

fn candidate_bins(app: &AppHandle, base: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let is_win = cfg!(windows);
    let exe = if is_win { format!("{base}.exe") } else { base.to_string() };
    if let Ok(res) = app.path().resource_dir() {
        #[cfg(windows)]
        out.push(res.join("ffmpeg").join("windows").join(&exe));
        #[cfg(not(windows))]
        out.push(res.join("ffmpeg").join("linux").join(&exe));
        out.push(res.join("bin").join(&exe));
        out.push(res.join(&exe));
    }
    if let Ok(cwd) = std::env::current_dir() {
        for p in [
            cwd.join("ffmpeg").join(if cfg!(windows) { "windows" } else { "linux" }).join(&exe),
            cwd.join("../ffmpeg").join(if cfg!(windows) { "windows" } else { "linux" }).join(&exe),
            cwd.join("../../ffmpeg").join(if cfg!(windows) { "windows" } else { "linux" }).join(&exe),
        ] {
            out.push(p);
        }
    }
    if let Ok(exe_dir) = std::env::current_exe() {
        if let Some(d) = exe_dir.parent() {
            out.push(d.join(&exe));
            out.push(d.join("ffmpeg").join(&exe));
            if let Some(p) = d.parent() {
                out.push(p.join("ffmpeg").join(if cfg!(windows) { "windows" } else { "linux" }).join(&exe));
            }
        }
    }
    out.push(PathBuf::from(exe));
    out
}

fn resolve_bin(app: &AppHandle, base: &str) -> Result<PathBuf, String> {
    for c in candidate_bins(app, base) {
        if c.components().count() > 1 && c.exists() {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&c, std::fs::Permissions::from_mode(0o755));
            }
            return Ok(c);
        }
        if c.components().count() == 1 {
            // PATH fallback: try running `<bin> -version`
            let probe = std::process::Command::new(&c).arg("-version").output();
            if probe.map(|o| o.status.success()).unwrap_or(false) {
                return Ok(c);
            }
        }
    }
    Err(format!("{} not found. Install FFmpeg or bundle it under ffmpeg/windows|linux.", base))
}

#[derive(Serialize)]
struct BinPaths {
    ffmpeg: String,
    ffprobe: String,
}

#[tauri::command]
fn cmd_get_version() -> String {
    VERSION.to_string()
}

#[tauri::command]
fn cmd_ffmpeg_paths(app: AppHandle) -> Result<BinPaths, String> {
    Ok(BinPaths {
        ffmpeg: resolve_bin(&app, "ffmpeg")?.to_string_lossy().to_string(),
        ffprobe: resolve_bin(&app, "ffprobe").unwrap_or(PathBuf::from("ffprobe")).to_string_lossy().to_string(),
    })
}

// ---------- probe ----------

#[derive(Debug, Serialize)]
struct ProbeInfo {
    file: String,
    file_name: String,
    file_size: u64,
    file_size_human: String,
    width: u32,
    height: u32,
    resolution: String,
    fps: f64,
    codec: String,
    duration: f64,
    duration_str: String,
    video_bitrate: Option<u64>,
    video_bitrate_str: String,
    audio: String,
    audio_codec: String,
    audio_bitrate_str: String,
}

fn human_size(b: u64) -> String {
    const U: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = b as f64;
    let mut i = 0;
    while v >= 1024.0 && i < 4 {
        v /= 1020.0 + 4.0; // == 1024, avoids magic-number lint noise
        i += 1;
    }
    if i == 0 {
        format!("{} {}", b, U[i])
    } else {
        format!("{:.1} {}", v, U[i])
    }
}

fn fmt_time(s: f64) -> String {
    let t = s.max(0.0) as u64;
    format!("{:02}:{:02}", t / 60, t % 60)
}

fn parse_fps(s: &str) -> f64 {
    if s.contains('/') {
        let p: Vec<&str> = s.split('/').collect();
        let n: f64 = p[0].parse().unwrap_or(0.0);
        let d: f64 = p.get(1).and_then(|x| x.parse().ok()).unwrap_or(1.0);
        if d == 0.0 { 0.0 } else { n / d }
    } else {
        s.parse().unwrap_or(0.0)
    }
}

#[tauri::command]
async fn cmd_probe(app: AppHandle, path: String) -> Result<ProbeInfo, String> {
    let p = Path::new(&path);
    if !p.exists() {
        return Err("File does not exist.".into());
    }
    let meta = std::fs::metadata(p).map_err(|e| format!("Cannot read file: {e}"))?;
    let ffprobe = resolve_bin(&app, "ffprobe")?;
    let out = tokio::process::Command::new(&ffprobe)
        .args(["-v", "quiet", "-print_format", "json", "-show_format", "-show_streams", &path])
        .output()
        .await
        .map_err(|e| format!("Cannot run ffprobe: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(format!("ffprobe failed (corrupted file?): {}", err.chars().take(300).collect::<String>()));
    }
    let v: serde_json::Value =
        serde_json::from_slice(&out.stdout).map_err(|_| "ffprobe returned invalid data.".to_string())?;
    let streams = v.get("streams").and_then(|s| s.as_array()).cloned().unwrap_or_default();
    let fmt = v.get("format").cloned().unwrap_or_default();
    let vs = streams.iter().find(|s| s.get("codec_type").and_then(|c| c.as_str()) == Some("video"));
    let au = streams.iter().find(|s| s.get("codec_type").and_then(|c| c.as_str()) == Some("audio"));
    let width = vs.and_then(|s| s.get("width").and_then(|x| x.as_u64())).unwrap_or(0) as u32;
    let height = vs.and_then(|s| s.get("height").and_then(|x| x.as_u64())).unwrap_or(0) as u32;
    let fps = vs
        .and_then(|s| s.get("avg_frame_rate").or(s.get("r_frame_rate")).and_then(|x| x.as_str()))
        .map(parse_fps)
        .unwrap_or(0.0);
    let codec = vs.and_then(|s| s.get("codec_name").and_then(|x| x.as_str())).unwrap_or("unknown").to_string();
    let duration: f64 = fmt
        .get("duration")
        .and_then(|d| d.as_str().and_then(|x| x.parse().ok()))
        .or(vs.and_then(|s| s.get("duration").and_then(|x| x.as_str()).and_then(|x| x.parse().ok())))
        .unwrap_or(0.0);
    let vbr: Option<u64> = fmt
        .get("bit_rate")
        .and_then(|b| b.as_str()?.parse().ok());
    let acodec = au.and_then(|s| s.get("codec_name").and_then(|x| x.as_str())).unwrap_or("none").to_string();
    let abr = au.and_then(|s| s.get("bit_rate").and_then(|x| x.as_str()).and_then(|x| x.parse::<u64>().ok()));
    let file_name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    Ok(ProbeInfo {
        file: path,
        file_name,
        file_size: meta.len(),
        file_size_human: human_size(meta.len()),
        resolution: if width > 0 { format!("{width}x{height}") } else { "unknown".into() },
        width,
        height,
        fps: (fps * 100.0).round() / 100.0,
        codec,
        duration,
        duration_str: fmt_time(duration),
        video_bitrate: vbr,
        video_bitrate_str: vbr.map(|b| format!("{} kb/s", b / 1000)).unwrap_or_else(|| "unknown".into()),
        audio: if acodec == "none" { "no audio".into() } else { format!("{acodec}, {}", abr.map(|b| format!("{}k", b / 1000)).unwrap_or_else(|| "unknown bitrate".into())) },
        audio_codec: acodec,
        audio_bitrate_str: abr.map(|b| format!("{}k", b / 1000)).unwrap_or_else(|| "128k".into()),
    })
}

// ---------- settings / args builder ----------

#[derive(Debug, Clone, Deserialize)]
struct OldStyle {
    #[serde(default)]
    blur: bool,
    #[serde(default)]
    fade: bool,
    #[serde(default)]
    lowbitrate: bool,
    #[serde(default)]
    sharpen: bool,
    #[serde(default)]
    noise: bool,
    #[serde(default)]
    ratio43: bool,
    #[serde(default)]
    reduce_fps: bool,
    #[serde(default)]
    audio_comp: bool,
    #[serde(default)]
    vhs: bool,
}

impl Default for OldStyle {
    fn default() -> Self {
        Self { blur: false, fade: false, lowbitrate: false, sharpen: false, noise: false, ratio43: false, reduce_fps: false, audio_comp: false, vhs: false }
    }
}

#[derive(Debug, Clone, Deserialize)]
struct ConvertSettings {
    #[serde(default)]
    width: Option<u32>,
    #[serde(default)]
    height: Option<u32>,
    #[serde(default)]
    fps: Option<f64>,
    #[serde(default = "d_vcodec")]
    video_codec: String, // h264|h265|mpeg4|vp9
    #[serde(default)]
    crf: Option<i32>,
    #[serde(default)]
    video_bitrate: Option<String>, // e.g. "1200k", empty = none
    #[serde(default = "d_acodec")]
    audio_codec: String, // aac|mp3|opus|none
    #[serde(default = "d_abitrate")]
    audio_bitrate: String,
    #[serde(default = "d_format")]
    format: String, // mp4|mkv|avi|mov|webm
    #[serde(default = "d_preset")]
    x264_preset: String, // ultrafast..veryslow
    #[serde(default)]
    old_style: OldStyle,
}

fn d_vcodec() -> String { "h264".into() }
fn d_acodec() -> String { "aac".into() }
fn d_abitrate() -> String { "128k".into() }
fn d_format() -> String { "mp4".into() }
fn d_preset() -> String { "medium".into() }

fn encoder_args(vcodec: &str) -> Vec<String> {
    match vcodec {
        "h265" => vec!["-c:v".into(), "libx265".into()],
        "mpeg4" => vec!["-c:v".into(), "mpeg4".into()],
        "vp9" => vec!["-c:v".into(), "libvpx-vp9".into()],
        _ => vec!["-c:v".into(), "libx264".into()],
    }
}

fn build_args(input: &str, output: &str, s: &ConvertSettings, preview_secs: Option<u64>) -> Vec<String> {
    let mut a: Vec<String> = vec!["-y".into(), "-nostats".into()];
    let mut vf: Vec<String> = Vec::new();

    if s.old_style.ratio43 {
        vf.push("scale=640:480:flags=lanczos,setsar=1".into());
    } else if let (Some(w), Some(h)) = (s.width, s.height) {
        if w > 0 && h > 0 {
            vf.push(format!("scale={w}:{h}:flags=lanczos"));
        }
    }
    if let Some(f) = s.fps {
        if f > 0.0 {
            vf.push(format!("fps={f}"));
        }
    } else if s.old_style.reduce_fps {
        vf.push("fps=24".into());
    }
    if s.old_style.blur {
        vf.push("boxblur=2:1".into());
    }
    if s.old_style.fade {
        vf.push("eq=saturation=0.65:contrast=0.92:brightness=0.03".into());
    }
    if s.old_style.vhs {
        vf.push("noise=alls=10:allf=t+u,eq=contrast=0.95:saturation=0.8,vignette=PI/4.5".into());
    } else if s.old_style.noise {
        vf.push("noise=alls=8:allf=t".into());
    }
    if s.old_style.sharpen {
        vf.push("unsharp=5:5:0.8:5:5:0.0".into());
    }

    // preview trim: -ss 5 -t N
    if preview_secs.is_some() {
        a.push("-ss".into());
        a.push("5".into());
    }
    a.push("-i".into());
    a.push(input.into());
    if let Some(secs) = preview_secs {
        a.push("-t".into());
        a.push(secs.to_string());
    }

    if !vf.is_empty() {
        a.push("-vf".into());
        a.push(vf.join(","));
    }

    a.extend(encoder_args(&s.video_codec));
    if s.video_codec == "h264" || s.video_codec == "h265" {
        a.push("-preset".into());
        a.push(s.x264_preset.clone());
    }

    let vb = s.video_bitrate.as_ref().map(|x| x.trim()).filter(|x| !x.is_empty());
    let mut crf = s.crf;
    if s.old_style.lowbitrate && vb.is_none() {
        crf = Some(crf.unwrap_or(26) + 4);
    }
    if let Some(b) = vb {
        a.push("-b:v".into());
        a.push(b.to_string());
        if s.old_style.lowbitrate {
            a.push("-maxrate".into());
            a.push(b.to_string());
            a.push("-bufsize".into());
            a.push("1600k".into());
        }
    } else if let Some(c) = crf {
        a.push("-crf".into());
        a.push(c.clamp(0, 51).to_string());
    }

    // audio
    let mut af: Vec<String> = Vec::new();
    if s.old_style.audio_comp {
        af.push("acompressor=threshold=-18dB:ratio=3:attack=20:release=250".into());
    }
    match s.audio_codec.as_str() {
        "none" => {
            a.push("-an".into());
        }
        "mp3" => {
            a.push("-c:a".into());
            a.push("libmp3lame".into());
            a.push("-b:a".into());
            a.push(s.audio_bitrate.clone());
        }
        "opus" => {
            a.push("-c:a".into());
            a.push("libopus".into());
            a.push("-b:a".into());
            a.push(s.audio_bitrate.clone());
        }
        _ => {
            a.push("-c:a".into());
            a.push("aac".into());
            a.push("-b:a".into());
            a.push(s.audio_bitrate.clone());
        }
    }
    if !af.is_empty() && s.audio_codec != "none" {
        a.push("-af".into());
        a.push(af.join(","));
    }

    if s.format == "mp4" || s.format == "mov" {
        a.push("-pix_fmt".into());
        a.push("yuv420p".into());
    }
    if s.format == "mp4" {
        a.push("-movflags".into());
        a.push("+faststart".into());
    }
    // progress last so parse works
    a.push("-progress".into());
    a.push("pipe:1".into());
    a.push(output.into());
    a
}

#[derive(Debug, Clone, Deserialize)]
struct ConvertRequest {
    id: String,
    input: String,
    output: String,
    duration: f64,
    settings: ConvertSettings,
}

#[derive(Serialize, Clone)]
struct ProgressPayload {
    id: String,
    percent: f64,
    current_secs: f64,
    total_secs: f64,
    current_str: String,
    total_str: String,
    speed: String,
    eta_str: String,
    done: bool,
}

#[tauri::command]
fn cmd_build_args(req: ConvertRequest) -> Vec<String> {
    build_args(&req.input, &req.output, &req.settings, None)
}

#[tauri::command]
async fn cmd_cancel(state: State<'_, AppState>, id: String) -> Result<(), String> {
    let map = state.jobs.lock().await;
    if let Some(child) = map.get(&id) {
        let mut c = child.lock().await;
        c.start_kill().map_err(|e| e.to_string())?;
        Ok(())
    } else {
        Err("Job not running.".into())
    }
}

#[tauri::command]
async fn cmd_convert(app: AppHandle, state: State<'_, AppState>, req: ConvertRequest) -> Result<String, String> {
    if !Path::new(&req.input).exists() {
        return Err("Input file does not exist.".into());
    }
    if let Some(parent) = Path::new(&req.output).parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(|e| format!("Cannot create output folder: {e}"))?;
        }
    }
    let ffmpeg = resolve_bin(&app, "ffmpeg")?;
    let args = build_args(&req.input, &req.output, &req.settings, None);

    let mut child = tokio::process::Command::new(&ffmpeg)
        .args(&args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("Cannot start ffmpeg: {e}"))?;

    let stdout = child.stdout.take().expect("piped stdout");
    let id = req.id.clone();
    let total = req.duration.max(0.1);
    let handle: Arc<Mutex<tokio::process::Child>> = Arc::new(Mutex::new(child));
    state.jobs.lock().await.insert(id.clone(), handle.clone());

    let app2 = app.clone();
    let id2 = id.clone();
    tokio::spawn(async move {
        let mut reader = BufReader::new(stdout).lines();
        let mut cur_ms: i64 = 0;
        let mut speed = String::from("--");
        while let Ok(Some(line)) = reader.next_line().await {
            let line = line.trim().to_string();
            if let Some(v) = line.strip_prefix("out_time_ms=") {
                cur_ms = v.parse().unwrap_or(cur_ms);
            } else if let Some(v) = line.strip_prefix("speed=") {
                let v = v.trim().to_string();
                if !v.is_empty() && v != "n/a" {
                    speed = v;
                }
            } else if line == "progress=end" {
                break;
            }
            if line.starts_with("out_time_ms=") || line.starts_with("speed=") {
                let cur = cur_ms.max(0) as f64 / 1_000_000.0;
                let pct = ((cur / total) * 100.0).clamp(0.0, 100.0);
                let factor: f64 = speed.trim_end_matches('x').parse().unwrap_or(0.0);
                let eta = if factor > 0.0 { (total - cur).max(0.0) / factor } else { 0.0 };
                let _ = app2.emit(
                    "convert-progress",
                    ProgressPayload {
                        id: id2.clone(),
                        percent: (pct * 10.0).round() / 10.0,
                        current_secs: cur,
                        total_secs: total,
                        current_str: fmt_time(cur),
                        total_str: fmt_time(total),
                        speed: speed.clone(),
                        eta_str: fmt_time(eta),
                        done: false,
                    },
                );
            }
        }
    });

    let status = {
        let mut c = handle.lock().await;
        c.wait().await.map_err(|e| format!("ffmpeg error: {e}"))?
    };
    state.jobs.lock().await.remove(&id);

    if !status.success() {
        // cancelled (SIGKILL / 255) vs real error: check output exists
        let cancelled = status.code().is_none();
        if cancelled || !Path::new(&req.output).exists() {
            let _ = std::fs::remove_file(&req.output);
            return Err("cancelled".into());
        }
        return Err(format!("ffmpeg exited with code {:?}", status.code()));
    }
    let _ = app.emit(
        "convert-progress",
        ProgressPayload {
            id: id.clone(),
            percent: 100.0,
            current_secs: total,
            total_secs: total,
            current_str: fmt_time(total),
            total_str: fmt_time(total),
            speed: "done".into(),
            eta_str: "00:00".into(),
            done: true,
        },
    );
    Ok(req.output)
}

#[derive(Debug, Deserialize)]
struct PreviewRequest {
    input: String,
    duration: f64,
    settings: ConvertSettings,
}

#[tauri::command]
async fn cmd_preview(app: AppHandle, req: PreviewRequest) -> Result<String, String> {
    if !Path::new(&req.input).exists() {
        return Err("Input file does not exist.".into());
    }
    let ffmpeg = resolve_bin(&app, "ffmpeg")?;
    let tmp = std::env::temp_dir().join(format!("oc_preview_{}.mp4", uuid::Uuid::new_v4()));
    let out = tmp.to_string_lossy().to_string();
    let args = build_args(&req.input, &out, &req.settings, Some(10));
    // strip -progress pipe for preview run
    let filtered: Vec<String> = args.into_iter().filter(|x| x != "pipe:1" && x != "-progress").collect();
    let status = tokio::process::Command::new(&ffmpeg)
        .args(&filtered)
        .output()
        .await
        .map_err(|e| format!("Cannot run ffmpeg: {e}"))?;
    if !status.status.success() {
        return Err("Preview failed. Try a shorter file or different preset.".into());
    }
    Ok(out)
}

#[tauri::command]
fn cmd_estimate(duration: f64, width: u32, height: u32, crf: Option<i32>, video_bitrate: Option<String>, audio_bitrate: String) -> String {
    if let Some(vb) = video_bitrate {
        let v = parse_bitrate(&vb);
        let a = parse_bitrate(&audio_bitrate);
        if v > 0.0 {
            let bytes = (v + a) * duration.max(1.0) / 8.0;
            return format!("~{}", human_size(bytes as u64));
        }
    }
    // CRF approximation: base bitrate scales with pixels
    let px = (width as f64 * height as f64).max(307200.0);
    let scale = px / 921600.0; // 720p ref
    let c = crf.unwrap_or(26) as f64;
    // rough: crf23@720p ~= 5 Mbps
    let mbps = 5.0 * scale * (2.0_f64.powf((23.0 - c) / 6.0));
    let total_bps = mbps * 1_000_000.0 + parse_bitrate(&audio_bitrate);
    format!("~{} (approx.)", human_size((total_bps * duration.max(1.0) / 8.0) as u64))
}

fn parse_bitrate(s: &str) -> f64 {
    let t = s.trim().to_lowercase();
    if let Some(n) = t.strip_suffix('k') {
        n.parse::<f64>().unwrap_or(0.0) * 1000.0
    } else if let Some(n) = t.strip_suffix('m') {
        n.parse::<f64>().unwrap_or(0.0) * 1_000_000.0
    } else {
        t.parse().unwrap_or(0.0)
    }
}

#[tauri::command]
async fn cmd_pick_file(app: AppHandle) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;
    let fp = app.dialog().file().add_filter("Video", &["mp4", "mkv", "avi", "mov", "webm", "flv", "wmv", "m4v"]).blocking_pick_file();
    Ok(fp.map(|f| f.to_string()))
}

#[tauri::command]
fn cmd_reveal(path: String) -> Result<(), String> {
    #[cfg(windows)]
    {
        std::process::Command::new("explorer").arg(format!("/select,{}", path)).spawn().map_err(|e| e.to_string())?;
        return Ok(());
    }
    #[cfg(not(windows))]
    {
        let dir = Path::new(&path).parent().map(|p| p.to_string_lossy().to_string()).unwrap_or(path);
        std::process::Command::new("xdg-open").arg(dir).spawn().map_err(|e| e.to_string())?;
        return Ok(());
    }
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_opener::init())
        .manage(AppState { jobs: Arc::new(Mutex::new(HashMap::new())) })
        .invoke_handler(tauri::generate_handler![
            cmd_get_version,
            cmd_ffmpeg_paths,
            cmd_probe,
            cmd_build_args,
            cmd_convert,
            cmd_cancel,
            cmd_preview,
            cmd_estimate,
            cmd_pick_file,
            cmd_reveal
        ])
        .run(tauri::generate_context!())
        .expect("error while running Old Converter");
}

fn main() {
    run();
}
