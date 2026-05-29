#!/usr/bin/env bash
# Deterministically assemble the Tauri updater manifest (latest.json) for a
# release from its already-uploaded, signed updater assets.
#
# Why this exists: tauri-action uploads a per-platform latest.json from each
# parallel matrix build, and those uploads race on the single shared release
# asset (tauri-apps/tauri-action#1270). The losing writes silently drop
# platforms, shipping an updater manifest that is missing e.g. linux-x86_64 —
# which makes `check()` fail with TargetNotFound for those users.
#
# Each platform's *.sig asset has a distinct name, so the signatures never
# race and are always all present. This script runs once in the `publish`
# job (single writer, after asset rename) and rebuilds latest.json from those
# signatures, then refuses to continue if any base platform is missing.
#
# Usage: assemble-latest-json.sh <owner/repo> <vX.Y.Z tag> <release_id>
# Writes ./latest.json. Requires gh (authenticated via GH_TOKEN) and jq.
set -euo pipefail

REPO="${1:?usage: assemble-latest-json.sh <repo> <tag> <release_id>}"
TAG="${2:?missing tag}"
RELEASE_ID="${3:?missing release_id}"
VER="${TAG#v}"
NOTES="${RELEASE_NOTES:-Download the installer for your platform from the assets below.}"
PUB_DATE="$(date -u +%Y-%m-%dT%H:%M:%S.000Z)"

assets_json="$(gh api "repos/${REPO}/releases/${RELEASE_ID}/assets" --paginate)"

# The base platform keys the Tauri updater looks up. tauri-plugin-updater's
# get_urls() uses "{os}-{arch}" verbatim when the target is set (the default),
# so these four keys are exactly what check() requires. Each maps to the regex
# matching its updater bundle asset (the .sig is keyed off that name + ".sig").
keys=(linux-x86_64 darwin-aarch64 darwin-x86_64 windows-x86_64)
pats=('\.AppImage\.tar\.gz$' 'aarch64\.app\.tar\.gz$' 'x64\.app\.tar\.gz$' '\.nsis\.zip$')

platforms='{}'
for i in "${!keys[@]}"; do
  key="${keys[$i]}"
  pat="${pats[$i]}"

  read -r bname burl < <(echo "$assets_json" | jq -r --arg re "$pat" \
    '([.[] | select(.name | test($re))][0]) | if . == null then "" else "\(.name) \(.browser_download_url)" end')
  if [[ -z "${bname:-}" ]]; then
    echo "WARN: no bundle asset matching /${pat}/ for ${key}" >&2
    continue
  fi

  sid="$(echo "$assets_json" | jq -r --arg n "${bname}.sig" \
    '([.[] | select(.name == $n)][0]) | if . == null then "" else (.id | tostring) end')"
  if [[ -z "${sid:-}" ]]; then
    echo "WARN: no signature asset ${bname}.sig for ${key}" >&2
    continue
  fi

  sig="$(gh api "repos/${REPO}/releases/assets/${sid}" -H "Accept: application/octet-stream")"
  platforms="$(echo "$platforms" | jq --arg k "$key" --arg s "$sig" --arg u "$burl" \
    '. + {($k): {signature: $s, url: $u}}')"
  echo "  ${key} -> ${bname}" >&2
done

jq -n --arg v "$VER" --arg n "$NOTES" --arg d "$PUB_DATE" --argjson p "$platforms" \
  '{version: $v, notes: $n, pub_date: $d, platforms: $p}' > latest.json

# Refuse to publish a broken manifest: every base platform must resolve to a
# url + signature, or we fail loudly (instead of silently shipping a manifest
# that breaks the updater for the missing platform, as happened in v0.3.33).
for key in "${keys[@]}"; do
  if ! jq -e --arg k "$key" '.platforms[$k].url and .platforms[$k].signature' latest.json >/dev/null; then
    echo "::error::latest.json is missing platform '${key}' — refusing to publish a broken updater manifest" >&2
    exit 1
  fi
done
echo "OK: latest.json complete for: ${keys[*]}" >&2
