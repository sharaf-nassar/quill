#!/usr/bin/env bash
# Captures the canonical marketing screenshots of the Quill widget.
#
# Requires: xdotool, ImageMagick (`import` plus `convert` or `magick`), awk.
#
# ── Why this script does not click coordinates ────────────────────────────────
# The main window *is* the widget now: 360px wide, no decorations, and a
# content-derived height. The old titlebar strip of Live/Analytics/Learning/
# Sessions buttons at fixed x-offsets no longer exists, and the two surfaces
# that replaced it both move under the script's feet:
#
#   * The view dropdown sits in the band header *below* LIMITS, whose height is
#     one row per enabled provider. Its y-position is a function of the
#     dataset, not of the window.
#   * Learning and Session Search are no longer their own windows; they are
#     sections of one Manage workspace reached from a rail.
#
# So navigation is driven the way an operator drives it — keyboard first:
#
#   * Widget views: Tab-walk focus while pressing ArrowDown after each hop.
#     ArrowDown opens the view listbox only when the switcher trigger holds
#     focus, and the open popup is detectable because it is the one surface
#     painted in the raised menu colour. Once open, a row is chosen absolutely
#     with Home + ArrowDown xN + Enter, so the selection never depends on which
#     view happened to be showing.
#   * Manage sections: Ctrl+M opens the workspace, and its Ctrl+K command
#     palette selects a section by typing that section's label. No rail
#     coordinate is hardcoded.
#
# The only pixel offsets left are inside the fixed 40px titlebar band and the
# LIMITS padding directly under it — used solely to drop keyboard focus so the
# `:focus-visible` ring never lands in a published screenshot.
#
# ── Preconditions ─────────────────────────────────────────────────────────────
#   * One Quill instance running, widget visible, at least one provider enabled.
#     Use `scripts/run_quill_demo.sh --clean` so the shots come from the
#     sandboxed dummy dataset and never from personal state.
#   * On GNOME/Mutter (X11) the raw `target/{debug,release}` binaries render
#     through a GL surface `import` cannot read; capture against the packaged
#     AppImage build. A blank grab is reported per shot rather than saved
#     silently.
#
# ── Output ────────────────────────────────────────────────────────────────────
# Written to OUTDIR (default `marketing-site/assets/screenshots/`):
#
#   hero.png / live.png     widget on the Usage view (live.png is a copy — the
#                           LIMITS band the `#live` section sells is part of
#                           the same 360px frame now)
#   models.png              widget on the Models view
#   analytics-context.png   widget on the Context view  (site anchor #context)
#   sessions.png            Manage → Sessions
#   learning.png            Manage → Learning
#   settings.png            Manage → Settings
#
# `memory.png` and `brevity.png` stay manual: both live inside a Manage section
# rather than being one, and both need dataset grooming first (see
# lat.md/infrastructure.md, Screenshot Capture).

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OUTDIR="${OUTDIR:-$REPO_ROOT/marketing-site/assets/screenshots}"

# ── Tunables ──────────────────────────────────────────────────────────────────

# Logical widget width; mirrors `width`/`maxWidth` in src-tauri/tauri.conf.json
# and WIDGET_WIDTH in src/App.tsx.
WIDGET_WIDTH=360

# 2x device-pixel-ratio output for HiDPI rendering on the marketing site. The
# grab happens at the widget's real 360px; ImageMagick resamples afterwards.
RETINA="${RETINA:-1}"

# Raised surface of the open view listbox (`.wg-viewdd-menu` in
# src/styles/index.css, the `menu-raised` token in DESIGN.md). Nothing else in
# the widget paints this colour as a fill, which is what makes counting it a
# reliable "is the dropdown open" probe. The fuzz absorbs compositing drift
# while staying far under the 3.7% distance to the widget surface (#14181f), so
# the two never collapse into one bucket. Calibrated against the 360px renders
# in specs/018-widget-ui-redesign/verification/: every closed state lands
# between 373 and 3617 matching pixels (antialiased text edges), dropdown-open
# lands at 17917.
MENU_COLOR="#1b212b"
MENU_FUZZ="1%"
MENU_PIXELS_MIN=8000

# Focus hops to try before giving up on finding the view switcher. The widget's
# tab ring is short, so anything past one full cycle is slack.
MAX_TAB_PROBE=24

DELAY_KEY=0.12      # after a keystroke that only moves focus
DELAY_VIEW=0.9      # after committing a view, before capturing
DELAY_SECTION=1.6   # after selecting a Manage section (lazy chunk + first read)
DELAY_WINDOW=8      # seconds to wait for a window to appear

# Blur target: logical offsets into the LIMITS band, just under the 40px
# titlebar and its hairline. Nothing there is focusable or a drag region, so a
# click drops focus without moving the window.
BLUR_X=180
BLUR_Y=52

# Pointer parking distance outside the captured window.
PARK_GAP=60

# View rows, in the order `VIEWS` declares them in
# src/components/widget/ViewRegion.tsx. Selection is by absolute row index, so
# this order is load-bearing — keep it in sync when a view is added.
VIEW_ROW_USAGE=0
VIEW_ROW_MODELS=1
VIEW_ROW_CONTEXT=2

# ── Bootstrap ─────────────────────────────────────────────────────────────────

log()  { echo "  $*"; }
warn() { echo "  WARNING: $*" >&2; }
die()  { echo "ERROR: $*" >&2; exit 1; }

usage() {
	cat <<EOF >&2
Usage: $(basename "$0")

Captures every widget view and Manage section of a running Quill instance.

Environment:
  OUTDIR=<dir>   Output directory (default marketing-site/assets/screenshots)
  RETINA=0       Skip the 2x upscale and keep the native 360px grab
EOF
}

case "${1:-}" in
	-h|--help) usage; exit 0 ;;
	"") ;;
	*) echo "unknown argument: $1" >&2; usage; exit 1 ;;
esac

command -v xdotool >/dev/null 2>&1 || die "xdotool is not installed."
command -v import  >/dev/null 2>&1 || die "ImageMagick 'import' is not installed."
command -v awk     >/dev/null 2>&1 || die "awk is not installed."

# ImageMagick 7 renames `convert` to `magick`; both are accepted.
if command -v convert >/dev/null 2>&1; then
	IM_CONVERT="convert"
elif command -v magick >/dev/null 2>&1; then
	IM_CONVERT="magick"
else
	die "ImageMagick 'convert' (or 'magick') is not installed."
fi

mkdir -p "$OUTDIR"

SCRATCH="$(mktemp -d "${TMPDIR:-/tmp}/quill-shots-XXXXXX")"
cleanup() { rm -rf "$SCRATCH"; }
trap cleanup EXIT

BLANK_SHOTS=0

# ── Window plumbing ───────────────────────────────────────────────────────────

# geom_field <window-id> <X|Y|WIDTH|HEIGHT>
geom_field() {
	xdotool getwindowgeometry --shell "$1" | sed -n "s/^$2=//p"
}

# wait_for_window <name-regex> — echoes the id of the first visible match.
wait_for_window() {
	local name="$1" waited=0 wid=""
	while (( waited < DELAY_WINDOW * 4 )); do
		wid="$(xdotool search --onlyvisible --name "$name" 2>/dev/null | head -1 || true)"
		if [[ -n "$wid" ]]; then
			echo "$wid"
			return 0
		fi
		sleep 0.25
		waited=$(( waited + 1 ))
	done
	return 1
}

activate() {
	xdotool windowactivate --sync "$1"
	sleep 0.25
}

# park_pointer <window-id>
# Moves the pointer off the window so no hover state, chart legend or native
# tooltip is baked into the next capture. Parks beside the window rather than
# at a screen coordinate: `getdisplaygeometry` reports one monitor, and the
# widget habitually lives on another. X clamps an out-of-range target to the
# screen, which is still off the window.
park_pointer() {
	local wid="$1" wx wy ww px
	wx="$(geom_field "$wid" X)"
	wy="$(geom_field "$wid" Y)"
	ww="$(geom_field "$wid" WIDTH)"
	if (( wx >= PARK_GAP )); then
		px=$(( wx - PARK_GAP ))
	else
		px=$(( wx + ww + PARK_GAP ))
	fi
	xdotool mousemove "$px" $(( wy + 40 ))
	sleep 0.2
}

# ── Capture ───────────────────────────────────────────────────────────────────

# looks_blank <png> — true when the grab carries no image (the GL-surface case).
looks_blank() {
	local deviation
	deviation="$("$IM_CONVERT" "$1" -alpha off -colorspace Gray \
		-format "%[fx:standard_deviation]" info:)"
	awk -v value="$deviation" 'BEGIN { exit (value < 0.005) ? 0 : 1 }'
}

# capture <output-path> <window-id>
capture() {
	local output="$1" wid="$2"
	activate "$wid"
	park_pointer "$wid"
	import -window "$wid" "$output"
	if looks_blank "$output"; then
		warn "$(basename "$output") came back blank — capture against the packaged AppImage build."
		BLANK_SHOTS=$(( BLANK_SHOTS + 1 ))
	fi
	if [[ "$RETINA" == "1" ]]; then
		"$IM_CONVERT" "$output" -filter Catrom -resize 200% "$output"
	fi
	log "Saved: $output"
}

# ── Widget navigation ─────────────────────────────────────────────────────────

# menu_pixels <window-id> — pixels painted in the raised menu colour.
menu_pixels() {
	local shot="$SCRATCH/probe.png"
	import -window "$1" "$shot"
	"$IM_CONVERT" "$shot" -alpha off -fuzz "$MENU_FUZZ" \
		-fill black +opaque "$MENU_COLOR" \
		-fill white -opaque "$MENU_COLOR" \
		-format "%[fx:int(mean*w*h+0.5)]" info:
}

# press <key> [times]
press() {
	local key="$1" times="${2:-1}" i
	for (( i = 0; i < times; i++ )); do
		xdotool key --clearmodifiers "$key"
		sleep "$DELAY_KEY"
	done
}

# drop_focus <window-id>
# Clicks the LIMITS band so document focus returns to the body. Keyboard focus
# draws the global `:focus-visible` ring, which must never reach a published
# shot, and a clean body focus also keeps the next Tab walk predictable.
drop_focus() {
	local wid="$1" wx wy
	wx="$(geom_field "$wid" X)"
	wy="$(geom_field "$wid" Y)"
	xdotool mousemove $(( wx + BLUR_X * WIDGET_SCALE )) $(( wy + BLUR_Y * WIDGET_SCALE ))
	xdotool click 1
	park_pointer "$wid"
}

# scroll_to_top <window-id>
# ArrowDown on a control that is not the listbox scrolls the widget's content
# column. The widget only scrolls once its content passes the 900px cap, but a
# shot of a half-scrolled instrument is worse than a redundant wheel spin.
scroll_to_top() {
	local wid="$1" wx wy wh
	wx="$(geom_field "$wid" X)"
	wy="$(geom_field "$wid" Y)"
	wh="$(geom_field "$wid" HEIGHT)"
	xdotool mousemove $(( wx + 8 * WIDGET_SCALE )) $(( wy + wh / 2 ))
	xdotool click --repeat 12 --delay 20 4
	park_pointer "$wid"
}

# open_view_menu <window-id>
# Walks focus until ArrowDown opens the view listbox. Returns non-zero when the
# switcher never answered, so the caller can fail loudly instead of capturing
# whatever view happened to be showing.
open_view_menu() {
	local wid="$1" hop
	drop_focus "$wid"
	for (( hop = 1; hop <= MAX_TAB_PROBE; hop++ )); do
		press Tab
		press Down
		if (( $(menu_pixels "$wid") >= MENU_PIXELS_MIN )); then
			log "View switcher reached in $hop focus hop(s)"
			return 0
		fi
	done
	return 1
}

# select_view <row-index>
# Home anchors the active row at Usage, so the hop count is absolute and the
# script never has to track which view is currently showing.
select_view() {
	local row="$1"
	press Home
	if (( row > 0 )); then
		press Down "$row"
	fi
	press Return
}

# capture_view <row-index> <label> <output-path>
capture_view() {
	local row="$1" label="$2" output="$3"
	activate "$WIDGET_WID"
	open_view_menu "$WIDGET_WID" || die "could not open the view dropdown for '$label'."
	select_view "$row"
	sleep "$DELAY_VIEW"
	drop_focus "$WIDGET_WID"
	scroll_to_top "$WIDGET_WID"
	capture "$output" "$WIDGET_WID"
}

# ── Manage navigation ─────────────────────────────────────────────────────────

# capture_section <palette-label> <output-path>
capture_section() {
	local label="$1" output="$2"
	activate "$MANAGE_WID"
	xdotool key --clearmodifiers ctrl+k
	sleep 0.4
	xdotool type --clearmodifiers --delay 25 "$label"
	sleep 0.3
	xdotool key --clearmodifiers Return
	sleep "$DELAY_SECTION"
	capture "$output" "$MANAGE_WID"
}

# ── Locate the widget ─────────────────────────────────────────────────────────

echo "Searching for the Quill widget..."
WIDGET_WID="$(wait_for_window '^Quill$' || true)"
[[ -n "$WIDGET_WID" ]] || die "no visible window titled 'Quill'. Is Quill running?"
log "Widget window: $WIDGET_WID"

WIDGET_W="$(geom_field "$WIDGET_WID" WIDTH)"
WIDGET_H="$(geom_field "$WIDGET_WID" HEIGHT)"
log "Geometry: ${WIDGET_W}x${WIDGET_H}"

# Physical width is the logical 360 times the display scale factor, so derive
# the scale rather than assuming 1:1 — every offset above is logical.
WIDGET_SCALE=$(( WIDGET_W / WIDGET_WIDTH ))
(( WIDGET_SCALE < 1 )) && WIDGET_SCALE=1
if (( WIDGET_W != WIDGET_WIDTH * WIDGET_SCALE )); then
	warn "widget is ${WIDGET_W}px wide, expected a multiple of ${WIDGET_WIDTH}px; shots may not match the design width."
fi

# ── Widget views ──────────────────────────────────────────────────────────────

echo ""
echo "[1/6] hero.png + live.png (Usage view)"
capture_view "$VIEW_ROW_USAGE" "Usage" "$OUTDIR/hero.png"
cp "$OUTDIR/hero.png" "$OUTDIR/live.png"
log "Saved: $OUTDIR/live.png (copy of hero.png)"

echo ""
echo "[2/6] models.png (Models view)"
capture_view "$VIEW_ROW_MODELS" "Models" "$OUTDIR/models.png"

echo ""
echo "[3/6] analytics-context.png (Context view)"
capture_view "$VIEW_ROW_CONTEXT" "Context" "$OUTDIR/analytics-context.png"

# ── Manage workspace ──────────────────────────────────────────────────────────

echo ""
echo "Opening the Manage workspace (Ctrl+M)..."
activate "$WIDGET_WID"
xdotool key --clearmodifiers ctrl+m
MANAGE_WID="$(wait_for_window '^Manage$' || true)"
[[ -n "$MANAGE_WID" ]] || die "the Manage workspace did not open within ${DELAY_WINDOW}s."
log "Manage window: $MANAGE_WID"
sleep 0.6

echo ""
echo "[4/6] sessions.png (Manage → Sessions)"
capture_section "Sessions" "$OUTDIR/sessions.png"

echo ""
echo "[5/6] learning.png (Manage → Learning)"
capture_section "Learning" "$OUTDIR/learning.png"

echo ""
echo "[6/6] settings.png (Manage → Settings)"
capture_section "Settings" "$OUTDIR/settings.png"

xdotool windowclose "$MANAGE_WID" 2>/dev/null || true
sleep 0.5

# Leave the widget on Usage so a re-run — and the operator — start from the
# state the app launches in.
activate "$WIDGET_WID"
if open_view_menu "$WIDGET_WID"; then
	select_view "$VIEW_ROW_USAGE"
	drop_focus "$WIDGET_WID"
fi

# ── Done ──────────────────────────────────────────────────────────────────────

echo ""
if (( BLANK_SHOTS > 0 )); then
	echo "Finished with $BLANK_SHOTS blank capture(s) — see the warnings above." >&2
	echo "Output directory: $OUTDIR" >&2
	exit 1
fi
echo "All screenshots saved to: $OUTDIR"
echo "Still manual: memory.png (Manage -> Learning -> Memories) and brevity.png"
echo "(Manage -> Settings -> Brevity); both are sub-panels, not sections."
