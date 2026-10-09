#!/usr/bin/env bash
# Debian/Ubuntu packages for building Clipward: GPUI (Wayland, X11, fonts), rodio
# (ALSA), the keychain (D-Bus) and whisper.cpp (cmake, clang for bindgen).
set -euo pipefail
sudo apt-get update
sudo apt-get install -y --no-install-recommends \
  build-essential cmake clang pkg-config \
  libasound2-dev libdbus-1-dev libfontconfig-dev libfreetype-dev \
  libwayland-dev libx11-xcb-dev libxcb1-dev libxkbcommon-dev libxkbcommon-x11-dev \
  libvulkan-dev libssl-dev libzstd-dev
