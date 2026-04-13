#!/usr/bin/env bash

set -euo pipefail

if [[ "$#" -ne 5 ]]; then
  echo "usage: $0 <staged-runtime-dir> <goarch> <artifact-name> <bundle-stamp> <output-path>" >&2
  exit 1
fi

staged_runtime_dir="$1"
goarch="$2"
artifact_name="$3"
bundle_stamp="$4"
output_path="$5"

if [[ ! -d "$staged_runtime_dir" ]]; then
  echo "staged runtime directory missing: $staged_runtime_dir" >&2
  exit 1
fi

case "$goarch" in
  amd64)
    linuxdeploy_arch="x86_64"
    ;;
  arm64)
    linuxdeploy_arch="aarch64"
    ;;
  *)
    echo "unsupported GOARCH for AppImage: $goarch" >&2
    exit 1
    ;;
esac

appdir_root="${RUNNER_TEMP:-/tmp}/appdir-${artifact_name}"
runner_temp_dir="${RUNNER_TEMP:-/tmp}"
linuxdeploy_bin="${RUNNER_TEMP:-/tmp}/linuxdeploy-${linuxdeploy_arch}.AppImage"
gtk_plugin_script="${RUNNER_TEMP:-/tmp}/linuxdeploy-plugin-gtk.sh"

if [[ ! -x "$linuxdeploy_bin" ]]; then
  echo "linuxdeploy not executable: $linuxdeploy_bin" >&2
  exit 1
fi
if [[ ! -x "$gtk_plugin_script" ]]; then
  echo "linuxdeploy gtk plugin script not executable: $gtk_plugin_script" >&2
  exit 1
fi

rm -rf "$appdir_root"
mkdir -p "$appdir_root/usr/bin"
mkdir -p "$appdir_root/usr/lib/zju-connect-gui/runtime"
mkdir -p "$appdir_root/usr/share/applications"
mkdir -p "$appdir_root/usr/share/icons/hicolor/512x512/apps"

cp -a "$staged_runtime_dir/." "$appdir_root/usr/lib/zju-connect-gui/runtime/"
printf '%s\n' "$bundle_stamp" >"$appdir_root/usr/lib/zju-connect-gui/runtime/.bundle-stamp"
cp "packaging/linux/appimage-runtime-launcher.sh" "$appdir_root/usr/bin/zju-connect-gui-launcher"
chmod +x "$appdir_root/usr/bin/zju-connect-gui-launcher"
cp "packaging/linux/zju-connect-gui.desktop" "$appdir_root/usr/share/applications/zju-connect-gui.desktop"
cp "assets/gemini.png" "$appdir_root/usr/share/icons/hicolor/512x512/apps/zju-connect-gui.png"

export APPIMAGE_EXTRACT_AND_RUN=1
export ARCH="$linuxdeploy_arch"
export LINUXDEPLOY_PLUGIN_GTK_APPDIR="$appdir_root"
export LINUXDEPLOY_OUTPUT_VERSION="$bundle_stamp"
export OUTPUT="$(basename "$output_path")"

export PATH="$runner_temp_dir:$PATH"

mkdir -p "$(dirname "$output_path")"
(
  cd "$(dirname "$output_path")"
  "$linuxdeploy_bin" \
    --appdir "$appdir_root" \
    --desktop-file "$appdir_root/usr/share/applications/zju-connect-gui.desktop" \
    --icon-file "$appdir_root/usr/share/icons/hicolor/512x512/apps/zju-connect-gui.png" \
    --executable "$appdir_root/usr/lib/zju-connect-gui/runtime/zju-connect-gui" \
    --plugin gtk \
    --output appimage
)

if [[ ! -f "$output_path" ]]; then
  detected_path="$(dirname "$output_path")/zju-connect-gui-${bundle_stamp}-${linuxdeploy_arch}.AppImage"
  if [[ -f "$detected_path" ]]; then
    mv "$detected_path" "$output_path"
  fi
fi

if [[ ! -f "$output_path" ]]; then
  echo "failed to create AppImage: $output_path" >&2
  exit 1
fi
