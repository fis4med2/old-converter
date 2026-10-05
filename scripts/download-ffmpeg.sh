#!/usr/bin/env bash
# Download static FFmpeg builds for bundling. Run from repo root.
# Usage: ./scripts/download-ffmpeg.sh [windows|linux|all]
set -euo pipefail
TARGET="${1:-all}"

if [[ "$TARGET" == "windows" || "$TARGET" == "all" ]]; then
  echo "== Windows FFmpeg =="
  mkdir -p ffmpeg/windows
  if [[ ! -f ffmpeg/windows/ffmpeg.exe ]]; then
    curl -L -o /tmp/opencode/ffmpeg-win.zip https://www.gyan.dev/ffmpeg/builds/ffmpeg-release-essentials.zip
    unzip -o /tmp/opencode/ffmpeg-win.zip -d /tmp/opencode/ffwin
    cp /tmp/opencode/ffwin/*/bin/ffmpeg.exe ffmpeg/windows/ffmpeg.exe
    cp /tmp/opencode/ffwin/*/bin/ffprobe.exe ffmpeg/windows/ffprobe.exe || echo "ffprobe missing in essentials build, trying full build"
    echo "Windows binaries ready."
  else
    echo "Windows binaries already present, skipping."
  fi
fi

if [[ "$TARGET" == "linux" || "$TARGET" == "all" ]]; then
  echo "== Linux FFmpeg =="
  mkdir -p ffmpeg/linux
  if [[ ! -f ffmpeg/linux/ffmpeg ]]; then
    curl -L -o /tmp/opencode/ffmpeg-linux.tar.xz https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-master-latest-linux64-gpl.tar.xz
    tar -xf /tmp/opencode/ffmpeg-linux.tar.xz -C /tmp/opencode
    cp /tmp/opencode/ffmpeg-master-latest-linux64-gpl/bin/ffmpeg ffmpeg/linux/ffmpeg
    cp /tmp/opencode/ffmpeg-master-latest-linux64-gpl/bin/ffprobe ffmpeg/linux/ffprobe
    chmod +x ffmpeg/linux/ffmpeg ffmpeg/linux/ffprobe
    echo "Linux binaries ready."
  else
    echo "Linux binaries already present, skipping."
  fi
fi
