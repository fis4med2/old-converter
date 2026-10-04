# FFmpeg binaries (not committed)

Place prebuilt static builds here:

- `ffmpeg/windows/ffmpeg.exe` + `ffprobe.exe`
- `ffmpeg/linux/ffmpeg` + `ffprobe`

Recommended sources:

- Windows: https://www.gyan.dev/ffmpeg/builds/ (ffmpeg-release-essentials.zip)
- Linux: https://johnvansickle.com/ffmpeg/ (ffmpeg-release-amd64-static.tar.xz)

Both are GPL/LGPL builds. Keep `ffmpeg/` binaries out of git; CI downloads them automatically (see `scripts/download-ffmpeg.sh` and workflows). The app also falls back to system `ffmpeg`/`ffprobe` on PATH if bundled files are missing.

License note: FFmpeg is LGPL/GPL. If you redistribute binaries, include their license text and link to sources. See README section "FFmpeg".
