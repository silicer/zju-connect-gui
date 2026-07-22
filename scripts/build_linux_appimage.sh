#!/usr/bin/env bash
# Build a Linux AppImage from the cargo release binary.
#
# Prerequisites:
#   - cargo (stable rust toolchain pinned by rust-toolchain.toml)
#   - linuxdeploy (downloaded automatically into ./build/tools/ if missing)
#
# Output: build/AppDir/ + zju-connect-gui-x86_64.AppImage at the repo root.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

APP_NAME="zju-connect-gui"
TARGET_TRIPLE="${TARGET_TRIPLE:-x86_64-unknown-linux-gnu}"
BUILD_DIR="$REPO_ROOT/build"
APPDIR="$BUILD_DIR/AppDir"
TOOLS_DIR="$BUILD_DIR/tools"

mkdir -p "$BUILD_DIR" "$TOOLS_DIR"

# 1. Cross-aware release build
echo "==> cargo build --release --target $TARGET_TRIPLE"
cargo build --release --target "$TARGET_TRIPLE"

BINARY="$REPO_ROOT/target/$TARGET_TRIPLE/release/$APP_NAME"
if [[ ! -x "$BINARY" ]]; then
  echo "error: release binary not found at $BINARY" >&2
  exit 1
fi

# 2. Fetch linuxdeploy if missing
LINUXDEPLOY="$TOOLS_DIR/linuxdeploy-x86_64.AppImage"
if [[ ! -x "$LINUXDEPLOY" ]]; then
  echo "==> downloading linuxdeploy"
  curl -fL --retry 3 \
    https://github.com/linuxdeploy/linuxdeploy/releases/download/continuous/linuxdeploy-x86_64.AppImage \
    -o "$LINUXDEPLOY"
  chmod +x "$LINUXDEPLOY"
fi

# 3. Stage AppDir
echo "==> staging $APPDIR"
rm -rf "$APPDIR"
mkdir -p "$APPDIR/usr/bin" "$APPDIR/usr/share/applications" \
         "$APPDIR/usr/share/icons/hicolor/512x512/apps"

cp "$BINARY" "$APPDIR/usr/bin/$APP_NAME"
cp "packaging/linux/${APP_NAME}.desktop" "$APPDIR/usr/share/applications/${APP_NAME}.desktop"
cp "assets/gemini.png" "$APPDIR/usr/share/icons/hicolor/512x512/apps/${APP_NAME}.png"

# Top-level AppImage requirements
cp "assets/gemini.png" "$APPDIR/${APP_NAME}.png"
cp "packaging/linux/${APP_NAME}.desktop" "$APPDIR/${APP_NAME}.desktop"
cp "packaging/linux/appimage-runtime-launcher.sh" "$APPDIR/AppRun"
chmod +x "$APPDIR/AppRun"

# 4. Run linuxdeploy
echo "==> linuxdeploy"
ARCH=x86_64 "$LINUXDEPLOY" \
  --appdir "$APPDIR" \
  --executable "$APPDIR/usr/bin/$APP_NAME" \
  --desktop-file "$APPDIR/${APP_NAME}.desktop" \
  --icon-file "$APPDIR/${APP_NAME}.png" \
  --output appimage

# linuxdeploy emits the .AppImage in CWD
mv "${APP_NAME}-x86_64.AppImage" "$REPO_ROOT/${APP_NAME}-x86_64.AppImage"
echo "==> ${APP_NAME}-x86_64.AppImage"
ls -lh "$REPO_ROOT/${APP_NAME}-x86_64.AppImage"
