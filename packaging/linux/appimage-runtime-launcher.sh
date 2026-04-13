#!/usr/bin/env bash

set -euo pipefail

launcher_dir="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"
source_runtime_root="$(CDPATH= cd -- "$launcher_dir/../lib/zju-connect-gui/runtime" && pwd)"

source_stamp_file="$source_runtime_root/.bundle-stamp"
source_stamp="unknown"
if [[ -f "$source_stamp_file" ]]; then
  source_stamp="$(<"$source_stamp_file")"
fi

state_home="${XDG_STATE_HOME:-$HOME/.local/state}"
runtime_cache_root="$state_home/zju-connect-gui/appimage"
target_runtime_root="$runtime_cache_root/runtime-$source_stamp"

mkdir -p "$runtime_cache_root"

if [[ ! -x "$target_runtime_root/zju-connect-gui" ]] || [[ ! -x "$target_runtime_root/bin/zju-connect" ]]; then
  temp_runtime_root="$runtime_cache_root/runtime-$source_stamp.tmp-$$"
  rm -rf "$temp_runtime_root"
  mkdir -p "$temp_runtime_root"
  cp -a "$source_runtime_root/." "$temp_runtime_root/"
  chmod -R u+w "$temp_runtime_root"
  rm -rf "$target_runtime_root"
  mv "$temp_runtime_root" "$target_runtime_root"
fi

exec "$target_runtime_root/zju-connect-gui" "$@"
