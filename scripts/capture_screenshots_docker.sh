#!/usr/bin/env bash
# shellcheck disable=SC2317
# Cleanup handlers are invoked through traps.
# Builds and runs Quill in a private Docker/Xvfb desktop, then replaces the
# canonical documentation screenshots only after the complete capture passes.

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

inside_container() {
	export DISPLAY=:99
	export GDK_BACKEND=x11
	export LIBGL_ALWAYS_SOFTWARE=1
	export GALLIUM_DRIVER=llvmpipe
	export WEBKIT_DISABLE_COMPOSITING_MODE=1
	export WEBKIT_DISABLE_DMABUF_RENDERER=1
	export NO_AT_BRIDGE=1
	export XDG_RUNTIME_DIR=/tmp/quill-runtime
	mkdir -p "$XDG_RUNTIME_DIR"
	chmod 700 "$XDG_RUNTIME_DIR"

	Xvfb "$DISPLAY" -screen 0 1280x1024x24 -dpi 96 -nolisten tcp +extension GLX +render -noreset &
	XVFB_PID=$!
	DEMO_PID=""
	OPENBOX_PID=""
	COMPOSITOR_PID=""
	# shellcheck disable=SC2329
	cleanup() {
		if [[ -n "$DEMO_PID" ]]; then kill "$DEMO_PID" 2>/dev/null || true; fi
		if [[ -n "$COMPOSITOR_PID" ]]; then kill "$COMPOSITOR_PID" 2>/dev/null || true; fi
		if [[ -n "$OPENBOX_PID" ]]; then kill "$OPENBOX_PID" 2>/dev/null || true; fi
		kill "$XVFB_PID" 2>/dev/null || true
	}
	trap cleanup EXIT

	for _ in $(seq 1 80); do
		xdotool getdisplaygeometry >/dev/null 2>&1 && break
		sleep 0.1
	done
	xdotool getdisplaygeometry >/dev/null 2>&1 || {
		echo "ERROR: Xvfb did not start." >&2
		exit 1
	}

	openbox --sm-disable >/tmp/openbox.log 2>&1 &
	OPENBOX_PID=$!
	xcompmgr -a >/tmp/xcompmgr.log 2>&1 &
	COMPOSITOR_PID=$!

	./scripts/run_quill_demo.sh --clean --bin /opt/quill/quill --keep-on-exit \
		>/tmp/quill-demo.log 2>&1 &
	DEMO_PID=$!

	for _ in $(seq 1 360); do
		if xdotool search --onlyvisible --name '^Quill$' >/dev/null 2>&1; then
			break
		fi
		if ! kill -0 "$DEMO_PID" 2>/dev/null; then
			cat /tmp/quill-demo.log >&2
			echo "ERROR: demo Quill exited before opening a window." >&2
			exit 1
		fi
		sleep 0.5
	done
	xdotool search --onlyvisible --name '^Quill$' >/dev/null 2>&1 || {
		cat /tmp/quill-demo.log >&2
		echo "ERROR: demo Quill did not open within 180 seconds." >&2
		exit 1
	}

	rm -rf /output/*
	OUTDIR=/output RETINA=1 ./scripts/take_screenshots.sh
	for file in "${EXPECTED[@]}"; do
		[[ -s "/output/$file" ]] || {
			echo "ERROR: missing screenshot: $file" >&2
			exit 1
		}
	done
	for file in hero.png live.png models.png analytics-context.png; do
		[[ "$(identify -format '%wx%h' "/output/$file")" == "720x1600" ]] || {
			echo "ERROR: unexpected widget dimensions: $file" >&2
			exit 1
		}
	done
	for file in sessions.png learning.png memory.png brevity.png; do
		[[ "$(identify -format '%wx%h' "/output/$file")" == "1920x1360" ]] || {
			echo "ERROR: unexpected Tools dimensions: $file" >&2
			exit 1
		}
	done
	[[ "$(identify -format '%wx%h' /output/settings.png)" == "1920x1020" ]] || {
		echo "ERROR: unexpected Integrations dimensions: settings.png" >&2
		exit 1
	}
	cmp -s /output/hero.png /output/live.png || {
		echo "ERROR: live.png must be the Usage capture copy." >&2
		exit 1
	}
	if cmp -s /output/settings.png /output/brevity.png; then
		echo "ERROR: Context navigation failed; brevity.png matches settings.png." >&2
		exit 1
	fi
}

if [[ "${1:-}" == "--inside" ]]; then
	inside_container
	exit 0
fi

command -v docker >/dev/null 2>&1 || {
	echo "ERROR: docker is not installed." >&2
	exit 1
}
docker info >/dev/null 2>&1 || {
	echo "ERROR: Docker is not running." >&2
	exit 1
}

STAGING="$(mktemp -d "${TMPDIR:-/tmp}/quill-docker-shots-XXXXXX")"
CONTAINER_ID=""
cleanup_host() {
	if [[ -n "$CONTAINER_ID" ]]; then
		docker rm -f "$CONTAINER_ID" >/dev/null 2>&1 || true
	fi
	rm -rf "$STAGING"
}
trap cleanup_host EXIT

echo "Building $IMAGE..."
docker build -f "$REPO_ROOT/Dockerfile.screenshots" -t "$IMAGE" "$REPO_ROOT"

CONTAINER_ID="$(docker create --network none --hostname demo-workstation "$IMAGE")"
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
