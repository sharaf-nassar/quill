#!/usr/bin/env bash
# Captures the real React app in Browser Mock Mode inside headless Chromium.
# Only validated PNGs leave the network-disabled runtime container.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
IMAGE="${IMAGE:-quill-screenshots:local}"
OUTPUT="$REPO_ROOT/marketing-site/assets/screenshots"
EXPECTED=(
	hero.png
	live.png
	models.png
	analytics-context.png
	sessions.png
	learning.png
	memory.png
	settings.png
	brevity.png
)

command -v docker >/dev/null 2>&1 || {
	echo "ERROR: docker is not installed." >&2
	exit 1
}
docker info >/dev/null 2>&1 || {
	echo "ERROR: Docker is not running." >&2
	exit 1
}

STAGING="$(mktemp -d "${TMPDIR:-/tmp}/quill-browser-shots-XXXXXX")"
CONTAINER_ID=""
# shellcheck disable=SC2317
cleanup() {
	if [[ -n "$CONTAINER_ID" ]]; then
		docker rm -f "$CONTAINER_ID" >/dev/null 2>&1 || true
	fi
	rm -rf "$STAGING"
}
trap cleanup EXIT

echo "Building $IMAGE..."
docker build -f "$REPO_ROOT/Dockerfile.screenshots" -t "$IMAGE" "$REPO_ROOT"

CONTAINER_ID="$(docker create --network none --shm-size 512m "$IMAGE")"
docker start -a "$CONTAINER_ID"
docker cp "$CONTAINER_ID:/output/." "$STAGING/"

for file in "${EXPECTED[@]}"; do
	[[ -s "$STAGING/$file" ]] || {
		echo "ERROR: container did not produce $file" >&2
		exit 1
	}
done

mkdir -p "$OUTPUT"
for file in "${EXPECTED[@]}"; do
	install -m 0644 "$STAGING/$file" "$OUTPUT/$file"
done

echo "Updated ${#EXPECTED[@]} screenshots in $OUTPUT"
