# OLD CONVERTER

**Make your videos feel old again.**

Desktop video converter with a 2015–2017 freeware look and real FFmpeg conversion underneath. Turns modern videos into files that feel uploaded to the internet in 2015–2017: real resolution / FPS / codec / CRF / bitrate / audio changes, plus optional old-style filters.

![placeholder screenshot](assets/README.md)

> Screenshots: add `assets/screenshot-main.png` after first run.

---

## Features

- **Real conversion** — bundled FFmpeg, args built as arrays (no shell concat), spaces/special chars safe
- **FFprobe inspect** — file, resolution, FPS, codec, duration, video bitrate, audio, file size
- **5 presets**: YouTube 2015 · YouTube 2017 · Potato 2016 · Old Internet 720p · Low Quality Upload
- **Old Style (9 toggles, all real)**:
  - Slight Blur → `boxblur`
  - Color Fading → `eq=saturation/contrast`
  - Low Bitrate Look → CRF bump / maxrate
  - Sharpen → `unsharp`
  - Slight Noise → `noise`
  - 4:3 Aspect Ratio → `scale=640:480,setsar=1`
  - Reduce FPS → `fps=24`
  - Audio Compression → `acompressor`
  - VHS-like Distortion → `noise+eq+vignette`
- **Custom Mode** — resolution, FPS, codec (H.264/H.265/MPEG-4/VP9), CRF, video bitrate, audio codec/bitrate, format (MP4/MKV/AVI/MOV/WEBM), x264 preset
- **Real progress** — ffmpeg `-progress pipe:1`, parsed `out_time_ms` vs duration, speed + ETA, cancel button, UI never blocks (async task)
- **10s preview** — renders 10s clip (from 00:05) with current settings; Original vs Old Version side by side
- **Size estimate** — `(video+audio bitrate)*duration/8`, CRF approximation table; always labeled approx.
- **Queue** — multiple videos, per-item preset snapshot, Waiting/Converting/Done/Error
- **History** — last 100 conversions in localStorage, clear button
- **Drag & drop** — Tauri drop event + HTML5 fallback, Windows + Linux paths supported
- **Done screen** — output path, original/converted/saved sizes, Open Folder, Convert Again, *"Welcome back to 2016."*

## Supported platforms

- Windows 10/11 64-bit → `OldConverter-Windows-x64-setup.exe` (NSIS) + portable `.zip`
- Linux 64-bit → `OldConverter-Linux-x86_64.AppImage` + `.tar.gz`

## Install (users)

Download from **Releases**, no dev tools needed:

- Windows: run the `-setup.exe`, FFmpeg is bundled.
- Linux: `chmod +x OldConverter-Linux-x86_64.AppImage && ./OldConverter-Linux-x86_64.AppImage`

## Development

No Rust/Node needed on user machines — only for dev:

```bash
git clone <repository>
cd old-converter

# optional: fetch bundled FFmpeg for local dev
./scripts/download-ffmpeg.sh all

npm install
npx tauri dev
```

Project layout:

```text
src/               # vanilla HTML/CSS/JS frontend (no bundler)
src-tauri/         # Rust backend (probe/convert/preview/estimate)
ffmpeg/windows|linux/  # bundled binaries (gitignored, CI downloads)
presets/           # 5 JSON presets
assets/
.github/workflows/    # build-windows, build-linux, release
```

Version is centralized: `VERSION` + `package.json` + `src-tauri/Cargo.toml` + `src-tauri/tauri.conf.json` (all `1.0.0`). Tag `v1.0.0` surfaces as `Old Converter v1.0.0` via `cmd_get_version`.

## Build

```bash
npx tauri build
# windows: src-tauri/target/release/bundle/nsis/*setup.exe
# linux:   src-tauri/target/release/bundle/appimage/*.AppImage
```

## Usage

1. Open Video or drop file → info appears via FFprobe.
2. Pick preset, toggle Old Style, or switch to Custom Mode.
3. Check estimated size (approx.).
4. Optional: Generate 10s Preview.
5. CONVERT → real progress → Conversion Complete → Open Folder.

## Presets

| Preset | Res | Codec | CRF | Audio |
|---|---|---|---|---|
| YouTube 2015 | 1280x720 | H.264 medium | 23 | AAC 128k |
| YouTube 2017 | 1920x1080 | H.264 medium | 21 | AAC 192k |
| Potato 2016 | 854x480 | H.264 veryfast | 30 | AAC 96k |
| Old Internet 720p | 1280x720 | H.264 medium | 27 | AAC 128k |
| Low Quality Upload | 640x360 | H.264 veryfast | 32 | AAC 96k |

JSON copies in `presets/`, also hardcoded in `src/app.js` for offline use.

## FFmpeg

- Windows CI: `gyan.dev` release-essentials (`ffmpeg.exe` + `ffprobe.exe`).
- Linux CI: BtbN FFmpeg-Builds static (`ffmpeg` + `ffprobe`, chmod +x).
- Resolution order: bundled `ffmpeg/{windows,linux}/` → `resourceDir` (prod) → dev relative paths → system `PATH` fallback.
- Errors handled: missing file, corrupted input, missing binary, no space, cancelled (output deleted).
- License: FFmpeg is LGPL-2.1+/GPL-2+. Bundles are third-party builds; see their pages for source + license text. This repo is MIT; FFmpeg binaries keep their own licenses.

## Releases (automatic)

```bash
git tag v1.0.0
git push origin v1.0.0
```

`release.yml` builds Windows + Linux, collects the 4 files, and publishes a GitHub Release with them attached. Uses only the default `GITHUB_TOKEN` — no secrets to configure. `push`/`pull_request` builds run `build-windows.yml` + `build-linux.yml` as checks.

## License

MIT — see `LICENSE`.
