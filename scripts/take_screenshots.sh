#!/usr/bin/env bash
# Captures screenshots of all Quill views.
# Requires: xdotool, import (imagemagick)
#
# Quill titlebar buttons (left to right):
#   Live | Analytics | 🧠 (Learning) | ⌕ (Sessions) | ⚙ (Plugins) | ↻ (Restart) | [close]
#
# Secondary windows open as separate Tauri windows with titles:
#   "Learning", "Session Search", "Plugin Manager"
set -euo pipefail

OUTDIR="$(cd "$(dirname "$0")/.." && pwd)/screenshots"
mkdir -p "$OUTDIR"

DELAY_SHORT=0.4   # seconds after a click before capturing
DELAY_WINDOW=1.5  # seconds to wait for a new window to open

# ── Helpers ───────────────────────────────────────────────────────────────────

log() { echo "  $*"; }

# capture <output-path> <window-id>
capture() {
	local output="$1" wid="$2"
	import -window "$wid" "$output"
	log "Saved: $output"
}

# wait_for_window <title> [max_wait_secs]
# Polls until a window with the given title appears; echoes its window ID.
wait_for_window() {
	local title="$1"
	local max_wait="${2:-6}"
	local waited=0
	while (( waited < max_wait )); do
		local wid
		wid=$(xdotool search --name "$title" 2>/dev/null | head -1 || true)
		if [[ -n "$wid" ]]; then
			echo "$wid"
			return 0
		fi
		sleep 0.3
		(( waited++ )) || true
	done
	echo "ERROR: window '$title' did not appear within ${max_wait}s" >&2
	return 1
}

# click_offset <window-id> <x-offset> <y-offset>
# Clicks at a position relative to the window's top-left corner.
click_offset() {
	local wid="$1" xoff="$2" yoff="$3"
	local geo
	geo=$(xdotool getwindowgeometry --shell "$wid")
	local wx wy
	wx=$(echo "$geo" | grep '^X=' | cut -d= -f2)
	wy=$(echo "$geo" | grep '^Y=' | cut -d= -f2)
	xdotool mousemove $(( wx + xoff )) $(( wy + yoff ))
	xdotool click 1
	sleep "$DELAY_SHORT"
}

# close_window_by_id <window-id>
close_window_by_id() {
	local wid="$1"
	xdotool windowclose "$wid" 2>/dev/null || true
	sleep 0.3
}

# ── Find main Quill window ─────────────────────────────────────────────────────

echo "Searching for Quill main window..."
QUILL_WID=$(xdotool search --name "^Quill$" 2>/dev/null | head -1 || true)
if [[ -z "$QUILL_WID" ]]; then
	echo "ERROR: Could not find a window titled 'Quill'. Is Quill running?" >&2
	exit 1
fi
log "Found Quill window: $QUILL_WID"

# Bring window to focus and get its geometry
xdotool windowactivate --sync "$QUILL_WID"
sleep 0.3

GEO=$(xdotool getwindowgeometry --shell "$QUILL_WID")
WIN_W=$(echo "$GEO" | grep '^WIDTH='  | cut -d= -f2)
WIN_H=$(echo "$GEO" | grep '^HEIGHT=' | cut -d= -f2)
log "Window geometry: ${WIN_W}x${WIN_H}"

# ── Button x-offsets within the titlebar ──────────────────────────────────────
# The titlebar is ~28px tall. Buttons are packed from the left edge:
#   [Live ~40px] [Analytics ~80px] [🧠 ~115px] [⌕ ~140px] [⚙ ~165px] [↻ ~190px]
# These offsets are approximate; adjust if your build differs.
TITLEBAR_Y=14        # vertical center of the titlebar row

BTN_LIVE=25
BTN_ANALYTICS=65
BTN_LEARNING=100
BTN_SESSIONS=125
BTN_PLUGINS=150
# BTN_RESTART=175  # not captured as a screenshot currently

# ── 1. Live view (default) ────────────────────────────────────────────────────

echo ""
echo "[1/6] live-view.png"
xdotool windowactivate --sync "$QUILL_WID"
sleep 0.3
# Click Live to ensure it is the active pane
click_offset "$QUILL_WID" "$BTN_LIVE" "$TITLEBAR_Y"
sleep 0.3
capture "$OUTDIR/live-view.png" "$QUILL_WID"

# ── 2. Analytics – Now tab ────────────────────────────────────────────────────

echo ""
echo "[2/6] analytics-view.png (Now tab)"
xdotool windowactivate --sync "$QUILL_WID"
click_offset "$QUILL_WID" "$BTN_ANALYTICS" "$TITLEBAR_Y"
sleep "$DELAY_SHORT"
# The Analytics view opens to the Now tab by default.
# Give it a moment to load data, then capture.
sleep 0.4
capture "$OUTDIR/analytics-view.png" "$QUILL_WID"

# ── 3. Analytics – Charts tab ─────────────────────────────────────────────────

echo ""
echo "[3/6] analytics-charts.png (Charts tab)"
# The tab bar sits below the titlebar (~50px from top).
# Tab order: Now | Charts | Trends | Breakdown
# "Charts" tab is approximately the second pill (~25% from left of window).
CHARTS_TAB_X=$(( WIN_W * 25 / 100 ))
CHARTS_TAB_Y=55
click_offset "$QUILL_WID" "$CHARTS_TAB_X" "$CHARTS_TAB_Y"
sleep 0.5
capture "$OUTDIR/analytics-charts.png" "$QUILL_WID"

# Return to Now tab so the analytics pane is in its default state
NOW_TAB_X=$(( WIN_W * 8 / 100 ))
click_offset "$QUILL_WID" "$NOW_TAB_X" "$CHARTS_TAB_Y"
sleep 0.3

# ── 4. Learning panel ─────────────────────────────────────────────────────────

echo ""
echo "[4/6] learning-panel.png"
xdotool windowactivate --sync "$QUILL_WID"
click_offset "$QUILL_WID" "$BTN_LEARNING" "$TITLEBAR_Y"
LEARN_WID=$(wait_for_window "Learning" 8)
xdotool windowactivate --sync "$LEARN_WID"
sleep "$DELAY_WINDOW"
capture "$OUTDIR/learning-panel.png" "$LEARN_WID"
close_window_by_id "$LEARN_WID"

# ── 5. Session Search ─────────────────────────────────────────────────────────

echo ""
echo "[5/6] session-search.png"
xdotool windowactivate --sync "$QUILL_WID"
click_offset "$QUILL_WID" "$BTN_SESSIONS" "$TITLEBAR_Y"
SESSIONS_WID=$(wait_for_window "Session Search" 8)
xdotool windowactivate --sync "$SESSIONS_WID"
sleep "$DELAY_WINDOW"
capture "$OUTDIR/session-search.png" "$SESSIONS_WID"
close_window_by_id "$SESSIONS_WID"

# ── 6. Plugin Manager ─────────────────────────────────────────────────────────

echo ""
echo "[6/6] plugins.png"
xdotool windowactivate --sync "$QUILL_WID"
click_offset "$QUILL_WID" "$BTN_PLUGINS" "$TITLEBAR_Y"
PLUGINS_WID=$(wait_for_window "Plugin Manager" 8)
xdotool windowactivate --sync "$PLUGINS_WID"
sleep "$DELAY_WINDOW"
capture "$OUTDIR/plugins.png" "$PLUGINS_WID"
close_window_by_id "$PLUGINS_WID"

# ── Done ──────────────────────────────────────────────────────────────────────

echo ""
echo "All screenshots saved to: $OUTDIR"
ls -lh "$OUTDIR"/*.png 2>/dev/null || true
