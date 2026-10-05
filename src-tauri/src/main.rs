use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::{Mutex, Notify};

const VERSION: &str = env!("CARGO_PKG_VERSION");

struct JobEntry {
    kill: Arc<Notify>,
    cancelled: Arc<AtomicBool>,
}

struct AppState {
    jobs: Arc<Mutex<HashMap<String, JobEntry>>>,
}

// ---------- ffmpeg resolution ----------

fn candidate_bins(app: &AppHandle, base: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let plat = if cfg!(windows) { "windows" } else { "linux" };
    let exe = if cfg!(windows) { format!("{base}.exe") } else { base.to_string() };

    if let Ok(res) = app.path().resource_dir() {
        let mut roots = vec![res.clone()];
        let mut cur = res.as_path();
        for _ in 0..3 {
            match cur.parent() {
                Some(p) => {
                    roots.push(p.to_path_buf());
                    cur = p;
                }
                None => break,
            }
        }
        for r in roots {
            out.push(r.join("ffmpeg").join(plat).join(&exe));
            // linuxdeploy encodes ".." from resources escaping the dir as "_up_"
            out.push(r.join("_up_").join("ffmpeg").join(plat).join(&exe));
            out.push(r.join("bin").join(&exe));
            out.push(r.join(&exe));
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        out.push(cwd.join("ffmpeg").join(plat).join(&exe));
        out.push(cwd.join("..").join("ffmpeg").join(plat).join(&exe));
        out.push(cwd.join("..").join("..").join("ffmpeg").join(plat).join(&exe));
    }
    if let Ok(self_exe) = std::env::current_exe() {
        if let Some(d) = self_exe.parent() {
            out.push(d.join(&exe));
            out.push(d.join("ffmpeg").join(plat).join(&exe));
            if let Some(p) = d.parent() {
                out.push(p.join("ffmpeg").join(plat).join(&exe));
            }
        }
    }
    out.push(PathBuf::from(&exe));
    out
}

fn resolve_bin(app: &AppHandle, base: &str) -> Result<PathBuf, String> {
    for c in candidate_bins(app, base) {
        // NOTE: must be is_file — tauri-build copies the resources tree next
        // to the binary, so a bare `ffmpeg/` *directory* exists there and a
        // plain exists() check would return it as the binary.
        if c.components().count() > 1 && c.is_file() {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&c, std::fs::Permissions::from_mode(0o755));
            }
            return Ok(c);
        }
    }
    // last resort: system PATH (under AppImage, AppRun poisons LD_LIBRARY_PATH
    // and system binaries fail with exit 127 — drop it for the probe)
    let mut probe = std::process::Command::new(base);
    probe.arg("-version");
    if std::env::var_os("APPDIR").is_some() {
        probe.env_remove("LD_LIBRARY_PATH");
    }
    if probe.output().map(|o| o.status.success()).unwrap_or(false) {
        return Ok(PathBuf::from(base));
    }
    Err(format!("{base} not found. Expected it under ffmpeg/{plat}/ next to the app.", plat = if cfg!(windows) { "windows" } else { "linux" }))
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
        ffprobe: resolve_bin(&app, "ffprobe").unwrap_or_else(|_| PathBuf::from("ffprobe")).to_string_lossy().to_string(),
    })
}

#[tauri::command]
async fn cmd_pick_file(app: AppHandle) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .add_filter("Video", &["mp4", "mkv", "avi", "mov", "webm", "flv", "wmv", "m4v", "mpg", "mpeg", "ts"])
        .pick_file(move |f| {
            let _ = tx.send(f);
        });
    let picked = rx.await.unwrap_or(None);
    Ok(picked.map(|f| f.to_string()))
}

#[tauri::command]
fn cmd_reveal(path: String) -> Result<(), String> {
    #[cfg(windows)]
    {
        std::process::Command::new("explorer")
            .arg(format!("/select,{}", path))
            .spawn()
            .map_err(|e| e.to_string())?;
    }
    #[cfg(not(windows))]
    {
        let dir = Path::new(&path)
            .parent()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or(path);
        std::process::Command::new("xdg-open")
            .arg(dir)
            .spawn()
            .map_err(|e| e.to_string())?;
    }
    Ok(())
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
        v /= 1024.0;
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
    let mut cmd = tokio::process::Command::new(&ffprobe);
    cmd.args(["-v", "quiet", "-print_format", "json", "-show_format", "-show_streams", &path]);
    if std::env::var_os("APPDIR").is_some() {
        cmd.env_remove("LD_LIBRARY_PATH");
    }
    let out = cmd.output().await.map_err(|e| format!("Cannot run ffprobe: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(format!(
            "ffprobe failed (corrupted or unsupported file?): {}",
            err.chars().take(300).collect::<String>()
        ));
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
    let codec = vs
        .and_then(|s| s.get("codec_name").and_then(|x| x.as_str()))
        .unwrap_or("unknown")
        .to_string();
    let duration: f64 = fmt
        .get("duration")
        .and_then(|d| d.as_str().and_then(|x| x.parse().ok()))
        .or(vs.and_then(|s| s.get("duration").and_then(|x| x.as_str()).and_then(|x| x.parse().ok())))
        .unwrap_or(0.0);
    let vbr: Option<u64> = fmt.get("bit_rate").and_then(|b| b.as_str()?.parse().ok());
    let acodec = au
        .and_then(|s| s.get("codec_name").and_then(|x| x.as_str()))
        .unwrap_or("none")
        .to_string();
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
        video_bitrate_str: vbr
            .map(|b| format!("{} kb/s", b / 1000))
            .unwrap_or_else(|| "unknown".into()),
        audio: if acodec == "none" {
            "no audio".into()
        } else {
            format!(
                "{acodec}, {}",
                abr.map(|b| format!("{}k", b / 1000)).unwrap_or_else(|| "unknown bitrate".into())
            )
        },
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
        Self {
            blur: false,
            fade: false,
            lowbitrate: false,
            sharpen: false,
            noise: false,
            ratio43: false,
            reduce_fps: false,
            audio_comp: false,
            vhs: false,
        }
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
    video_bitrate: Option<String>,
    #[serde(default = "d_acodec")]
    audio_codec: String, // aac|mp3|opus|none
    #[serde(default = "d_abitrate")]
    audio_bitrate: String,
    #[serde(default = "d_format")]
    format: String, // mp4|mkv|avi|mov|webm
    #[serde(default = "d_preset")]
    x264_preset: String,
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

/// preview: Some((start_secs, clip_secs)) renders only a clip instead of the whole file.
fn build_args(input: &str, output: &str, s: &ConvertSettings, preview: Option<(u64, u64)>) -> Vec<String> {
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

    if let Some((start, _)) = preview {
        a.push("-ss".into());
        a.push(start.to_string());
    }
    a.push("-i".into());
    a.push(input.into());
    if let Some((_, secs)) = preview {
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
    if preview.is_none() {
        // progress last so it is easy to parse
        a.push("-progress".into());
        a.push("pipe:1".into());
    }
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
async fn cmd_cancel(state: State<'_, AppState>, id: String) -> Result<(), String> {
    let map = state.jobs.lock().await;
    if let Some(job) = map.get(&id) {
        job.cancelled.store(true, Ordering::SeqCst);
        job.kill.notify_one();
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

    let mut cc = tokio::process::Command::new(&ffmpeg);
    cc.args(&args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    if std::env::var_os("APPDIR").is_some() {
        cc.env_remove("LD_LIBRARY_PATH");
    }
    let mut child = cc.spawn().map_err(|e| format!("Cannot start ffmpeg: {e}"))?;

    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");

    let kill = Arc::new(Notify::new());
    let cancelled = Arc::new(AtomicBool::new(false));
    let kill2 = kill.clone();
    let waiter = tokio::spawn(async move {
        tokio::select! {
            r = child.wait() => r,
            _ = kill2.notified() => {
                let _ = child.start_kill();
                child.wait().await
            }
        }
    });

    let id = req.id.clone();
    let total = req.duration.max(0.1);
    state.jobs.lock().await.insert(
        id.clone(),
        JobEntry { kill, cancelled: cancelled.clone() },
    );

    // progress reader
    {
        let app2 = app.clone();
        let id2 = id.clone();
        tokio::spawn(async move {
            let mut reader = BufReader::new(stdout).lines();
            let mut cur_ms: i64 = 0;
            let mut speed = String::from("--");
            while let Ok(Some(line)) = reader.next_line().await {
                let line = line.trim().to_string();
                let mut changed = false;
                if let Some(v) = line.strip_prefix("out_time_ms=") {
                    cur_ms = v.parse().unwrap_or(cur_ms);
                    changed = true;
                } else if let Some(v) = line.strip_prefix("speed=") {
                    let v = v.trim().to_string();
                    if !v.is_empty() && v != "n/a" {
                        speed = v;
                        changed = true;
                    }
                } else if line == "progress=end" {
                    break;
                }
                if changed {
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
    }

    // stderr tail reader (diagnostics + avoids pipe deadlock)
    let tail = Arc::new(Mutex::new(String::new()));
    {
        let t = tail.clone();
        tokio::spawn(async move {
            let mut reader = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                let mut s = t.lock().await;
                s.push_str(&line);
                s.push('\n');
                if s.len() > 4000 {
                    let cut = s.len() - 4000;
                    let at = s[cut..].find('\n').map(|i| cut + i + 1).unwrap_or(cut);
                    s.drain(..at);
                }
            }
        });
    }

    let status = waiter
        .await
        .map_err(|e| format!("ffmpeg task failed: {e}"))?
        .map_err(|e| format!("ffmpeg wait failed: {e}"))?;
    state.jobs.lock().await.remove(&id);

    if !status.success() {
        let was_cancelled = cancelled.load(Ordering::SeqCst);
        let _ = std::fs::remove_file(&req.output);
        if was_cancelled || status.code().is_none() {
            return Err("cancelled".into());
        }
        let t = tail.lock().await.clone();
        let last: Vec<&str> = t.lines().rev().take(5).collect();
        let msg: String = last.iter().rev().cloned().collect::<Vec<&str>>().join(" | ");
        return Err(if msg.is_empty() {
            format!("ffmpeg exited with code {:?}", status.code())
        } else {
            format!("ffmpeg failed: {msg}")
        });
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

/// Compatible 480p H.264 proxy of any input, for in-app playback when the
/// webview cannot decode the original codec (HEVC/AV1/.... Cached by
/// path+size+mtime in the temp dir. Never touches the user's file.
#[tauri::command]
async fn cmd_proxy(app: AppHandle, input: String) -> Result<String, String> {
    let p = Path::new(&input);
    if !p.exists() {
        return Err("Input file does not exist.".into());
    }
    let meta = std::fs::metadata(p).map_err(|e| format!("Cannot read file: {e}"))?;
    let mut h = std::collections::hash_map::DefaultHasher::new();
    std::hash::Hash::hash(&input, &mut h);
    std::hash::Hash::hash(&meta.len(), &mut h);
    std::hash::Hash::hash(&meta.modified().ok(), &mut h);
    let out = std::env::temp_dir().join(format!("oc_orig_{:x}.mp4", std::hash::Hasher::finish(&h)));
    if !out.exists() {
        let ffmpeg = resolve_bin(&app, "ffmpeg")?;
        let outs = out.to_string_lossy().to_string();
        let args = [
            "-y",
            "-nostats",
            "-i",
            input.as_str(),
            "-vf",
            "scale=-2:'min(480,ih)':flags=fast_bilinear",
            "-c:v",
            "libx264",
            "-preset",
            "veryfast",
            "-crf",
            "28",
            "-c:a",
            "aac",
            "-b:a",
            "96k",
            "-pix_fmt",
            "yuv420p",
            "-movflags",
            "+faststart",
            outs.as_str(),
        ];
        let mut pc = tokio::process::Command::new(&ffmpeg);
        pc.args(&args);
        if std::env::var_os("APPDIR").is_some() {
            pc.env_remove("LD_LIBRARY_PATH");
        }
        let res = pc.output().await.map_err(|e| format!("Cannot run ffmpeg: {e}"))?;
        if !res.status.success() {
            return Err("Could not build a playable preview of this file.".into());
        }
    }
    Ok(out.to_string_lossy().to_string())
}

#[tauri::command]
async fn cmd_preview(app: AppHandle, req: PreviewRequest) -> Result<String, String> {
    if !Path::new(&req.input).exists() {
        return Err("Input file does not exist.".into());
    }
    let ffmpeg = resolve_bin(&app, "ffmpeg")?;
    let tmp = std::env::temp_dir().join(format!("oc_preview_{}.mp4", uuid::Uuid::new_v4()));
    let out = tmp.to_string_lossy().to_string();
    let start: u64 = if req.duration > 16.0 { 5 } else { 0 };
    let clip: u64 = if req.duration > 8.0 { 10 } else { req.duration.max(1.0) as u64 };
    // preview file is always .mp4: force a compatible encoder set no matter
    // what the conversion settings ask for (vp9-in-mp4 would not play back)
    let mut s = req.settings.clone();
    s.video_codec = "h264".into();
    s.audio_codec = "aac".into();
    s.format = "mp4".into();
    s.x264_preset = "veryfast".into();
    let args = build_args(&req.input, &out, &s, Some((start, clip)));
    let mut pc = tokio::process::Command::new(&ffmpeg);
    pc.args(&args);
    if std::env::var_os("APPDIR").is_some() {
        pc.env_remove("LD_LIBRARY_PATH");
    }
    let res = pc.output().await.map_err(|e| format!("Cannot run ffmpeg: {e}"))?;
    if !res.status.success() {
        let e = String::from_utf8_lossy(&res.stderr);
        let last: Vec<&str> = e.lines().rev().take(3).collect();
        return Err(format!(
            "Preview failed: {}",
            last.iter().rev().cloned().collect::<Vec<&str>>().join(" | ")
        ));
    }
    Ok(out)
}

#[tauri::command]
fn cmd_estimate(
    duration: f64,
    width: u32,
    height: u32,
    crf: Option<i32>,
    video_bitrate: Option<String>,
    audio_bitrate: String,
) -> String {
    if let Some(vb) = video_bitrate {
        let v = parse_bitrate(&vb);
        let a = parse_bitrate(&audio_bitrate);
        if v > 0.0 {
            let bytes = (v + a) * duration.max(1.0) / 8.0;
            return format!("~{}", human_size(bytes as u64));
        }
    }
    let px = (width as f64 * height as f64).max(307200.0);
    let scale = px / 921600.0; // 720p reference
    let c = crf.unwrap_or(26) as f64;
    // rough: ~5 Mbps at CRF23/720p, halves every ~6 CRF steps
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
            cmd_convert,
            cmd_cancel,
            cmd_preview,
            cmd_proxy,
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
