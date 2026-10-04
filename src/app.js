/* Old Converter frontend - vanilla JS, no bundler. Uses Tauri v2 globals. */
const $ = (id) => document.getElementById(id);
const invoke = (...a) => window.__TAURI__.core.invoke(...a);

let current = null; // ProbeInfo
let runningId = null;
let lastOutput = null;
let queue = [];

const PRESETS = {
  yt2015:  { w: 1280, h: 720,  vcodec: "h264", crf: 23, vbr: "", abitrate: "128k", acodec: "aac", format: "mp4", preset: "medium" },
  yt2017:  { w: 1920, h: 1080, vcodec: "h264", crf: 21, vbr: "", abitrate: "192k", acodec: "aac", format: "mp4", preset: "medium" },
  potato:  { w: 854,  h: 480,  vcodec: "h264", crf: 30, vbr: "", abitrate: "96k",  acodec: "aac", format: "mp4", preset: "veryfast" },
  old720:  { w: 1280, h: 720,  vcodec: "h264", crf: 27, vbr: "", abitrate: "128k", acodec: "aac", format: "mp4", preset: "medium" },
  lowq:    { w: 640,  h: 360,  vcodec: "h264", crf: 32, vbr: "", abitrate: "96k",  acodec: "aac", format: "mp4", preset: "veryfast" },
};

function presetId() {
  const el = document.querySelector('input[name="preset"]:checked');
  return el ? el.value : "yt2015";
}
function oldStyle() {
  return {
    blur: $("os_blur").checked, fade: $("os_fade").checked, lowbitrate: $("os_lowbitrate").checked,
    sharpen: $("os_sharpen").checked, noise: $("os_noise").checked, ratio43: $("os_43").checked,
    reduce_fps: $("os_fps").checked, audio_comp: $("os_acomp").checked, vhs: $("os_vhs").checked,
  };
}
function applyPresetToCustom(pid) {
  const p = PRESETS[pid];
  if (!p) return;
  $("c_width").value = p.w; $("c_height").value = p.h;
  $("c_vcodec").value = p.vcodec; $("c_crf").value = p.crf;
  $("c_vbitrate").value = p.vbr; $("c_abitrate").value = p.abitrate;
  $("c_acodec").value = p.acodec; $("c_format").value = p.format; $("c_preset").value = p.preset;
}
function settingsFromUI() {
  const pid = presetId();
  if (pid !== "custom") applyPresetToCustom(pid);
  const fpsRaw = $("c_fps").value.trim();
  return {
    width: parseInt($("c_width").value) || null,
    height: parseInt($("c_height").value) || null,
    fps: fpsRaw ? parseFloat(fpsRaw) : null,
    video_codec: $("c_vcodec").value,
    crf: $("c_vbitrate").value.trim() ? null : (parseInt($("c_crf").value) ?? 23),
    video_bitrate: $("c_vbitrate").value.trim() || null,
    audio_codec: $("c_acodec").value,
    audio_bitrate: $("c_abitrate").value,
    format: $("c_format").value,
    x264_preset: $("c_preset").value,
    old_style: oldStyle(),
  };
}
function outputFor(input, format) {
  const sep = input.includes("\\") ? "\\" : "/";
  const i = input.lastIndexOf(sep);
  const dir = i >= 0 ? input.slice(0, i) : ".";
  const base = (i >= 0 ? input.slice(i + 1) : input).replace(/\.[^.]+$/, "");
  return `${dir}${sep}${base}_old.${format}`;
}
function fileSrc(p) {
  try {
    const c = window.__TAURI__.core.convertFileSrc;
    if (c) return c(p);
  } catch {}
  return `file:///${p.replace(/\\/g, "/")}`;
}
function err(msg) {
  const b = $("errBox");
  b.textContent = msg;
  b.classList.remove("hidden");
  setTimeout(() => b.classList.add("hidden"), 6000);
}

async function loadFile(path) {
  try {
    $("errBox").classList.add("hidden");
    const info = await invoke("cmd_probe", { path });
    current = info;
    $("fiFile").textContent = info.file_name;
    $("fiRes").textContent = info.resolution;
    $("fiFps").textContent = info.fps ? info.fps + " fps" : "unknown";
    $("fiCodec").textContent = info.codec;
    $("fiDur").textContent = `${info.duration_str} (${Math.round(info.duration)}s)`;
    $("fiVbr").textContent = info.video_bitrate_str;
    $("fiAudio").textContent = info.audio;
    $("fiSize").textContent = info.file_size_human;
    $("videoOrig").src = fileSrc(info.file);
    $("videoOld").removeAttribute("src");
    $("videoOld").load();
    $("doneBox").classList.add("hidden");
    await refreshEstimate();
  } catch (e) {
    err(typeof e === "string" ? e : JSON.stringify(e));
  }
}

async function refreshEstimate() {
  if (!current) return;
  const s = settingsFromUI();
  const pid = presetId();
  $("outPath").textContent = outputFor(current.file, s.format);
  $("estOrig").textContent = current.file_size_human;
  try {
    const est = await invoke("cmd_estimate", {
      duration: current.duration, width: s.width || current.width, height: s.height || current.height,
      crf: s.crf, videoBitrate: s.video_bitrate, audioBitrate: s.audio_bitrate,
    });
    $("estNew").textContent = est;
  } catch { $("estNew").textContent = "~unknown (approx.)"; }
}

function renderQueue() {
  const box = $("queueList");
  if (!queue.length) { box.innerHTML = '<div class="empty">Queue empty.</div>'; return; }
  box.innerHTML = "";
  queue.forEach((q, i) => {
    const d = document.createElement("div");
    d.className = "qitem";
    d.innerHTML = `<span>${q.name}</span><span class="st">${q.status}</span>`;
    d.title = `${q.input} · ${q.presetLabel}`;
    d.onclick = () => { if (q.status !== "Converting") loadFile(q.input); };
    const rm = document.createElement("button");
    rm.textContent = "x"; rm.className = "mini";
    rm.onclick = (e) => { e.stopPropagation(); queue.splice(i, 1); renderQueue(); };
    d.appendChild(rm);
    box.appendChild(d);
  });
}

function getHistory() {
  try { return JSON.parse(localStorage.getItem("oc_history") || "[]"); } catch { return []; }
}
function renderHistory() {
  const h = getHistory();
  const box = $("histList");
  if (!h.length) { box.innerHTML = '<div class="empty">No conversions yet.</div>'; return; }
  box.innerHTML = "";
  h.slice().reverse().forEach((it) => {
    const d = document.createElement("div");
    d.className = "hitem";
    d.innerHTML = `<span>${it.in} → ${it.out}<br><small>${it.preset} · ${it.size}</small></span>`;
    box.appendChild(d);
  });
}
function pushHistory(entry) {
  const h = getHistory();
  h.push(entry);
  localStorage.setItem("oc_history", JSON.stringify(h.slice(-100)));
  renderHistory();
}

async function doConvertOne(input, info, settings, presetLabel) {
  const id = `job-${Date.now()}-${Math.floor(Math.random() * 9999)}`;
  runningId = id;
  const output = outputFor(input, settings.format);
  $("btnConvert").disabled = true;
  $("btnCancel").disabled = false;
  $("progWrap").classList.remove("hidden");
  $("doneBox").classList.add("hidden");
  try {
    const out = await invoke("cmd_convert", { req: { id, input, output, duration: info.duration, settings } });
    lastOutput = out;
    let outSize = "-";
    try {
      const probe = await invoke("cmd_probe", { path: out });
      outSize = probe.file_size_human;
    } catch {}
    $("doneBox").classList.remove("hidden");
    $("doneOutput").textContent = out;
    $("doneOrig").textContent = info.file_size_human;
    $("doneNew").textContent = outSize;
    $("doneSaved").textContent = "see sizes above";
    $("videoOld").src = fileSrc(out);
    pushHistory({ in: info.file_name, out: out.split(/[/\\]/).pop(), preset: presetLabel, size: outSize });
    return true;
  } catch (e) {
    if (e === "cancelled") {
      $("progWrap").classList.add("hidden");
    } else {
      err(typeof e === "string" ? e : "Conversion failed.");
    }
    return false;
  } finally {
    runningId = null;
    $("btnConvert").disabled = false;
    $("btnCancel").disabled = true;
  }
}

async function onConvert() {
  const jobs = queue.length ? queue : (current ? [{ input: current.file, info: current, settings: settingsFromUI(), presetLabel: presetId(), name: current.file_name, status: "Waiting" }] : []);
  if (!jobs.length) { err("Open a video first."); return; }
  for (const j of jobs) {
    j.status = "Converting"; renderQueue();
    const ok = await doConvertOne(j.input, j.info, j.settings, j.presetLabel);
    j.status = ok ? "Done" : "Error"; renderQueue();
    if (!ok) break;
  }
  if (queue.length) { queue = queue.filter((q) => q.status !== "Done"); renderQueue(); }
}

window.addEventListener("DOMContentLoaded", async () => {
  if (!window.__TAURI__ || !window.__TAURI__.core) {
    $("ffStatus").textContent = "FATAL: Tauri API missing — app.withGlobalTauri must be true in tauri.conf.json.";
    return;
  }
  renderQueue(); renderHistory();
  document.querySelectorAll('input[name="preset"]').forEach((r) => r.addEventListener("change", () => { applyPresetToCustom(presetId()); refreshEstimate(); }));
  ["c_width","c_height","c_fps","c_vcodec","c_crf","c_vbitrate","c_acodec","c_abitrate","c_format","c_preset"].forEach((id) => $(id).addEventListener("input", refreshEstimate));
  document.querySelectorAll('.checks input').forEach((c) => c.addEventListener("change", refreshEstimate));
  applyPresetToCustom("yt2015");

  try {
    const v = await invoke("cmd_get_version");
    $("ver").textContent = "v" + v; $("ver2").textContent = "v" + v;
  } catch {}
  try {
    const p = await invoke("cmd_ffmpeg_paths");
    $("ffStatus").textContent = `FFmpeg OK: ${p.ffmpeg}`;
  } catch (e) {
    $("ffStatus").textContent = "FFmpeg NOT FOUND — bundle ffmpeg/windows|linux or install system ffmpeg.";
  }

  try {
    await window.__TAURI__.event.listen("convert-progress", (ev) => {
      const p = ev.payload;
      if (p.id !== runningId && runningId) return;
      $("progFill").style.width = p.percent + "%";
      $("progPct").textContent = p.percent.toFixed(1) + "%";
      $("progTime").textContent = `${p.current_str} / ${p.total_str}`;
      $("progSpeed").textContent = "Speed: " + p.speed;
      $("progEta").textContent = p.eta_str;
    });
  } catch {}

  // Tauri v2 drag-drop
  try {
    const wv = window.__TAURI__.webview.getCurrentWebview();
    await wv.onDragDropEvent((ev) => {
      if (ev.payload.type === "drop" && ev.payload.paths && ev.payload.paths.length) {
        loadFile(ev.payload.paths[0]);
      }
    });
  } catch {}

  const dz = $("dropZone");
  const pick = async () => {
    try {
      const f = await invoke("cmd_pick_file");
      if (f) loadFile(f);
    } catch (e) { err(typeof e === "string" ? e : "Cannot open dialog: " + JSON.stringify(e)); }
  };
  $("btnOpen").onclick = pick;
  dz.onclick = pick;
  ["dragover", "dragenter"].forEach((e) => dz.addEventListener(e, (ev) => { ev.preventDefault(); dz.classList.add("over"); }));
  ["dragleave", "drop"].forEach((e) => dz.addEventListener(e, (ev) => { ev.preventDefault(); dz.classList.remove("over"); }));

  $("btnQueue").onclick = () => {
    if (!current) { err("Open a video first."); return; }
    queue.push({ input: current.file, info: current, settings: settingsFromUI(), presetLabel: presetId(), name: current.file_name, status: "Waiting" });
    renderQueue();
  };
  $("btnConvert").onclick = onConvert;
  $("btnCancel").onclick = async () => { if (runningId) { try { await invoke("cmd_cancel", { id: runningId }); } catch {} } };
  $("btnPreview").onclick = async () => {
    if (!current) { err("Open a video first."); return; }
    $("btnPreview").disabled = true;
    try {
      const out = await invoke("cmd_preview", { req: { input: current.file, duration: current.duration, settings: settingsFromUI() } });
      $("videoOld").src = fileSrc(out);
    } catch (e) { err(typeof e === "string" ? e : "Preview failed."); }
    $("btnPreview").disabled = false;
  };
  $("btnOpenFolder").onclick = async () => { if (lastOutput) { try { await invoke("cmd_reveal", { path: lastOutput }); } catch {} } };
  $("btnAgain").onclick = () => { $("doneBox").classList.add("hidden"); $("progWrap").classList.add("hidden"); };
  $("btnClearHist").onclick = () => { localStorage.removeItem("oc_history"); renderHistory(); };
});
