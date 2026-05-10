#!/usr/bin/env bash
# Launches a sandboxed Quill instance against pre-seeded dummy data so a maintainer
# can capture marketing-site screenshots without touching their personal Quill state.
#
# Contract: specs/001-marketing-site/contracts/launcher-cli.md
#
# Usage:
#   scripts/run_quill_demo.sh [--clean] [--bin PATH] [--keep-on-exit]
#
# Exit codes:
#   0   demo Quill exited cleanly
#   1   bad argument(s)
#   2   sandbox setup failed
#   3   seeder failed
#   4   no Quill binary found
#   >4  forwarded from the Quill child process

set -euo pipefail

# ── Defaults ──────────────────────────────────────────────────────────────────

CLEAN=0
KEEP_ON_EXIT=0
BIN_OVERRIDE=""

# Sandbox path is stable per user so re-running the launcher reuses the same dataset.
SANDBOX="${TMPDIR:-/tmp}/quill-demo-${USER:-$(id -un)}"

# ── Argument parsing ──────────────────────────────────────────────────────────

usage() {
	cat <<EOF >&2
Usage: $(basename "$0") [--clean] [--bin PATH] [--keep-on-exit]

  --clean           Delete sandbox before launch (forces fresh seed).
  --bin PATH        Override Quill binary auto-discovery.
  --keep-on-exit    On Quill exit, suppress the teardown-command hint.
EOF
}

while [[ $# -gt 0 ]]; do
	case "$1" in
		--clean)         CLEAN=1; shift ;;
		--bin)           BIN_OVERRIDE="${2:-}"; [[ -z "$BIN_OVERRIDE" ]] && { usage; exit 1; }; shift 2 ;;
		--bin=*)         BIN_OVERRIDE="${1#--bin=}"; shift ;;
		--keep-on-exit)  KEEP_ON_EXIT=1; shift ;;
		-h|--help)       usage; exit 0 ;;
		*) echo "[demo] unknown argument: $1" >&2; usage; exit 1 ;;
	esac
done

# ── Locate Quill binary ───────────────────────────────────────────────────────

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"

discover_bin() {
	if [[ -n "$BIN_OVERRIDE" ]]; then
		[[ -x "$BIN_OVERRIDE" ]] && { printf '%s\n' "$BIN_OVERRIDE"; return 0; }
		return 1
	fi
	if command -v quill >/dev/null 2>&1; then
		command -v quill
		return 0
	fi
	for candidate in "$REPO_ROOT/src-tauri/target/release/quill" "$REPO_ROOT/src-tauri/target/debug/quill"; do
		[[ -x "$candidate" ]] && { printf '%s\n' "$candidate"; return 0; }
	done
	return 1
}

QUILL_BIN="$(discover_bin)" || {
	echo "[demo] ERROR: no Quill binary found." >&2
	echo "[demo] Tried: --bin override, \$PATH, $REPO_ROOT/src-tauri/target/{release,debug}/quill" >&2
	echo "[demo] Install Quill or build it first:  cargo build --release --manifest-path src-tauri/Cargo.toml" >&2
	exit 4
}

# ── Sandbox prep ──────────────────────────────────────────────────────────────

if (( CLEAN )); then
	rm -rf "$SANDBOX" || { echo "[demo] ERROR: could not clean $SANDBOX" >&2; exit 2; }
fi

mkdir -p "$SANDBOX/data" "$SANDBOX/rules" "$SANDBOX/projects" || {
	echo "[demo] ERROR: could not create sandbox dirs under $SANDBOX" >&2
	exit 2
}

export QUILL_DEMO_MODE=1
export QUILL_DATA_DIR="$SANDBOX/data"
export QUILL_RULES_DIR="$SANDBOX/rules"
export QUILL_CLAUDE_PROJECTS_DIR="$SANDBOX/projects"

echo "[demo] sandbox at $SANDBOX" >&2
echo "[demo] data:     $QUILL_DATA_DIR" >&2
echo "[demo] rules:    $QUILL_RULES_DIR" >&2
echo "[demo] projects: $QUILL_CLAUDE_PROJECTS_DIR" >&2

# ── Seed ──────────────────────────────────────────────────────────────────────

if ! python3 "$REPO_ROOT/scripts/populate_dummy_data.py" \
		--data-dir "$QUILL_DATA_DIR" \
		--rules-dir "$QUILL_RULES_DIR" \
		--projects-dir "$QUILL_CLAUDE_PROJECTS_DIR" \
		--no-backup \
		--quiet; then
	echo "[demo] ERROR: seeder failed; sandbox left at $SANDBOX for inspection" >&2
	exit 3
fi

# ── Launch Quill ──────────────────────────────────────────────────────────────

echo "[demo] launching $QUILL_BIN ..." >&2

CHILD_RC=0
"$QUILL_BIN" "$@" || CHILD_RC=$?

# ── Teardown hint ─────────────────────────────────────────────────────────────

if (( ! KEEP_ON_EXIT )); then
	echo "" >&2
	echo "[demo] sandbox preserved at $SANDBOX" >&2
	echo "[demo] to clean up:  rm -rf $SANDBOX" >&2
fi

exit $CHILD_RC
