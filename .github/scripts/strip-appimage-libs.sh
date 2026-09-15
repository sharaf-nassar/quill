#!/usr/bin/env bash
# Remove host display-stack libraries from Tauri AppImages, then rebuild and
# re-sign the updater artifacts.
#
# Why this exists: the Tauri AppImage bundler (via its pinned linuxdeploy)
# copies the build host's libwayland/libxkbcommon/libxcb/libXau/libXdmcp into
# usr/lib, where AppRun puts them ahead of the system copies. Mesa 25+ cannot
# create an EGL display through those older libraries, so WebKitWebProcess
# aborts with "Could not create default EGL display: EGL_BAD_PARAMETER" and
# the transparent, undecorated window stays blank (tauri-apps/tauri#15976,
# #15665). Every desktop with GTK already provides these libraries.
#
# Usage: strip-appimage-libs.sh <path/to/App.AppImage>...
# Rewrites each AppImage in place, rebuilds <AppImage>.tar.gz (a single-file
# tarball, matching Tauri's v1Compatible updater bundle), and signs both with
# `tauri signer sign` (TAURI_SIGNING_PRIVATE_KEY[_PASSWORD]). Set
# SKIP_SIGNING=1 for local testing only. Requires readelf and squashfs-tools.
set -euo pipefail

LIBS=(
  libwayland-client.so.0 libwayland-cursor.so.0 libwayland-egl.so.1 libwayland-server.so.0
  libxkbcommon.so.0
  libxcb-randr.so.0 libxcb-render.so.0 libxcb-shm.so.0
  libXau.so.6 libXdmcp.so.6
)

header_field() { readelf -h "$1" | awk -F: -v k="$2" '$1 ~ k { gsub(/[^0-9]/, "", $2); print $2 }'; }

for app in "$@"; do
  # The type-2 runtime is an ELF whose section header table ends the file's
  # executable part; the squashfs payload starts right after it.
  shoff=$(header_field "$app" "Start of section headers")
  shentsize=$(header_field "$app" "Size of section headers")
  shnum=$(header_field "$app" "Number of section headers")
  offset=$((shoff + shentsize * shnum))

  stat=$(unsquashfs -s -o "$offset" "$app")
  comp=$(awk '/^Compression/ { print $2 }' <<<"$stat")
  block=$(awk '/^Block size/ { print $3 }' <<<"$stat")

  work=$(mktemp -d)
  trap 'rm -rf "$work"' EXIT
  unsquashfs -q -n -o "$offset" -d "$work/root" "$app"

  removed=()
  for lib in "${LIBS[@]}"; do
    if [[ -e "$work/root/usr/lib/$lib" || -L "$work/root/usr/lib/$lib" ]]; then
      rm -f "$work/root/usr/lib/$lib"
      removed+=("$lib")
    fi
  done
  echo "$(basename "$app"): removed ${#removed[@]} libraries: ${removed[*]:-none}" >&2

  head -c "$offset" "$app" >"$work/app.AppImage"
  mksquashfs "$work/root" "$work/payload" -all-root -noappend -comp "$comp" -b "$block" >/dev/null
  cat "$work/payload" >>"$work/app.AppImage"
  chmod 755 "$work/app.AppImage"
  mv -f "$work/app.AppImage" "$app"

  tar -C "$(dirname "$app")" --owner=0 --group=0 -czf "$app.tar.gz" "$(basename "$app")"

  if [[ "${SKIP_SIGNING:-}" != 1 ]]; then
    for f in "$app" "$app.tar.gz"; do
      rm -f "$f.sig"
      npx tauri signer sign "$f" >/dev/null
      [[ -s "$f.sig" ]] || { echo "::error::signing did not produce $f.sig" >&2; exit 1; }
    done
  fi

  rm -rf "$work"
  trap - EXIT
done
