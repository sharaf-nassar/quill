#!/bin/sh
# Quill installer for Linux (x86_64 AppImage).
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/sharaf-nassar/quill/main/install.sh | sh
#
# Browsers save downloads non-executable, so a raw AppImage download otherwise
# needs a manual `chmod +x` before it runs. This script removes that step: it
# fetches the latest Quill AppImage, marks it executable, installs it to
# ~/Applications/Quill.AppImage, and launches it. On first launch Quill offers
# to add itself to your applications menu (with an icon) and auto-updates in
# place afterward.
set -eu

REPO="sharaf-nassar/quill"
DEST_DIR="${HOME}/Applications"
DEST="${DEST_DIR}/Quill.AppImage"

info() { printf 'quill-install: %s\n' "$1"; }
err() {
  printf 'quill-install: error: %s\n' "$1" >&2
  exit 1
}

# --- preconditions -------------------------------------------------------
[ "$(uname -s)" = "Linux" ] ||
  err "the AppImage is Linux-only (this is $(uname -s)); see the releases page for macOS/Windows."
arch="$(uname -m)"
[ "$arch" = "x86_64" ] ||
  err "only x86_64 Linux is currently built (this is ${arch})."
command -v curl >/dev/null 2>&1 || err "curl is required but not installed."

# --- resolve the latest AppImage URL ------------------------------------
info "Finding the latest Quill release..."
url="$(
  curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" |
    grep '"browser_download_url"' |
    grep '_linux_amd64\.AppImage"' |
    head -n 1 |
    cut -d '"' -f 4
)"
[ -n "$url" ] || err "could not find an x86_64 Linux AppImage in the latest release."

# --- download atomically and install ------------------------------------
mkdir -p "$DEST_DIR"
tmp="$(mktemp "${DEST_DIR}/.Quill.AppImage.XXXXXX")"
trap 'rm -f "$tmp"' EXIT INT TERM
info "Downloading $(basename "$url")..."
curl -fSL --progress-bar "$url" -o "$tmp"
chmod +x "$tmp"
mv -f "$tmp" "$DEST"
trap - EXIT INT TERM
info "Installed to ${DEST}"

# --- launch so first-run integration can add the menu entry -------------
if [ -n "${DISPLAY:-}" ] || [ -n "${WAYLAND_DISPLAY:-}" ]; then
  info "Launching Quill - accept the prompt to add it to your applications menu."
  if command -v setsid >/dev/null 2>&1; then
    setsid "$DEST" >/dev/null 2>&1 </dev/null &
  else
    nohup "$DEST" >/dev/null 2>&1 </dev/null &
  fi
else
  info "No graphical session detected - run ${DEST} once to finish setup."
fi
