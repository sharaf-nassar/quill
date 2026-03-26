# Qbuild Visual Companion Guide

Browser-based visual companion for showing architecture options and UI previews during a build.

## When to Use

Only at two specific integration points — not for every question.

**Phase 3 — Architecture Options:** Present multiple architectural or UI design approaches visually so the user can compare layouts, component structures, or data flow diagrams side by side.

**Phase 6 — Implementation Preview:** After UI implementors complete a wave, show a wireframe preview of what changed.

The test: **would the user understand this better by seeing it than reading it?**

Visual companion activates automatically when UI changes are detected — you do not offer it.

## How It Works

The server watches a directory for HTML files and serves the newest one to the browser. Write HTML content fragments to `VISUAL_SCREEN_DIR`; the server wraps them in the frame template and serves them live. User clicks are captured via WebSocket and written to `VISUAL_STATE_DIR/events` as JSONL.

## Starting a Session

```bash
bash "${CLAUDE_PLUGIN_ROOT}/scripts/visual/start-server.sh" --project-dir "<PROJECT_DIR>"
```

Response JSON — save these variables:

```
VISUAL_SESSION_DIR  — session root directory
VISUAL_SCREEN_DIR   — write HTML files here
VISUAL_STATE_DIR    — read events and server-info from here
VISUAL_URL          — browser URL to share with user
```

**Platform notes:**
- **macOS / Linux:** Script backgrounds itself — run normally, capture stdout.
- **Windows:** Use `run_in_background: true` on the Bash tool call, then read `$VISUAL_STATE_DIR/server-info` on the next turn.
- **Codex:** Script auto-detects `CODEX_CI` and runs in foreground — no extra flags needed.

## The Loop

1. **Check server alive** — verify `$VISUAL_STATE_DIR/server-info` exists. If missing (or `server-stopped` exists), restart with `start-server.sh`.

2. **Write HTML fragment** to `VISUAL_SCREEN_DIR` using the Write tool. Never use cat/heredoc. Use semantic filenames. Server serves the newest file automatically.

3. **Tell the user** — share `VISUAL_URL` and a brief summary of what's on screen. Ask them to respond in the terminal when ready.

4. **Read events** on next turn — check `$VISUAL_STATE_DIR/events` for browser interactions. Combine with terminal text to get the full picture.

5. **Push a waiting screen** when returning to terminal-only interaction:

   ```html
   <div style="display:flex;align-items:center;justify-content:center;min-height:60vh">
     <p class="subtitle">Continuing in terminal...</p>
   </div>
   ```

## Writing Content Fragments

Write only the content that goes inside the page — no `<html>`, no `<head>`, no `<script>`. The server injects all infrastructure automatically.

**Minimal example:**

```html
<h2>Which architecture fits best?</h2>
<p class="subtitle">Consider maintainability and team familiarity</p>

<div class="options">
  <div class="option" data-choice="a" onclick="toggleSelect(this)">
    <div class="letter">A</div>
    <div class="content">
      <h3>Layered Monolith</h3>
      <p>Single deployable, clear layer boundaries</p>
    </div>
  </div>
  <div class="option" data-choice="b" onclick="toggleSelect(this)">
    <div class="letter">B</div>
    <div class="content">
      <h3>Service Modules</h3>
      <p>Domain-separated modules, shared runtime</p>
    </div>
  </div>
</div>
```

## CSS Classes Available

### Options (A/B/C choices)

```html
<div class="options">
  <div class="option" data-choice="a" onclick="toggleSelect(this)">
    <div class="letter">A</div>
    <div class="content"><h3>Title</h3><p>Description</p></div>
  </div>
</div>
```

Add `data-multiselect` to `.options` to allow selecting multiple items.

### Cards (visual designs)

```html
<div class="cards">
  <div class="card" data-choice="design1" onclick="toggleSelect(this)">
    <div class="card-image"><!-- mockup content --></div>
    <div class="card-body"><h3>Name</h3><p>Description</p></div>
  </div>
</div>
```

### Mockup container

```html
<div class="mockup">
  <div class="mockup-header">Preview: Dashboard</div>
  <div class="mockup-body"><!-- your mockup HTML --></div>
</div>
```

### Split view (side-by-side)

```html
<div class="split">
  <div class="mockup"><!-- left --></div>
  <div class="mockup"><!-- right --></div>
</div>
```

### Pros/Cons

```html
<div class="pros-cons">
  <div class="pros"><h4>Pros</h4><ul><li>Benefit</li></ul></div>
  <div class="cons"><h4>Cons</h4><ul><li>Drawback</li></ul></div>
</div>
```

### Mock elements (wireframe building blocks)

```html
<div class="mock-nav">Logo | Home | About | Settings</div>
<div style="display:flex;">
  <div class="mock-sidebar">Navigation</div>
  <div class="mock-content">Main content area</div>
</div>
<button class="mock-button">Action</button>
<input class="mock-input" placeholder="Input field">
<div class="placeholder">Placeholder area</div>
```

### Typography and sections

- `h2` — page title
- `h3` — section heading
- `.subtitle` — secondary text below title
- `.section` — content block with bottom margin
- `.label` — small uppercase label text

## Browser Events Format

Events are written to `$VISUAL_STATE_DIR/events` (one JSON object per line, cleared on each new screen):

```jsonl
{"type":"click","choice":"a","text":"Option A - Layered Monolith","timestamp":1706000101}
{"type":"click","choice":"b","text":"Option B - Service Modules","timestamp":1706000115}
```

The last `choice` event is typically the final selection. A pattern of clicks reveals hesitation worth probing. If the file doesn't exist, the user didn't interact with the browser — use only their terminal text.

## Phase 3 Usage — Architecture Options

Write one `.option` per approach (2–4 max), with a short tradeoff description in the body. Semantic filenames: `arch-options.html`, `arch-options-v2.html`.

After the user responds in terminal, read `$VISUAL_STATE_DIR/events` and merge with their text before proceeding to plan.

## Phase 6 Usage — Implementation Preview

After each implementor wave, read the changed files and write a wireframe preview. This is informational — push the screen, tell the user what changed, then continue without blocking on browser feedback. Semantic filenames: `wave-1-preview.html`, `wave-2-preview.html`.

Use `.mockup` containers with `.mock-nav`, `.mock-sidebar`, `.mock-content` to represent structure. Focus on layout changes, not pixel-perfect rendering.

## Graceful Fallback

If the server fails to start or dies mid-build, set `HAS_UI_CHANGES=false` and continue text-only. Never abort the build over visual companion failure.

## Cleaning Up

```bash
bash "${CLAUDE_PLUGIN_ROOT}/scripts/visual/stop-server.sh" "$VISUAL_SESSION_DIR"
```

Session files persist in `.qbuild/visual/` under the project directory.

## File Naming

Use semantic names (`arch-options.html`, `wave-1-preview.html`). Never reuse filenames — each screen is a new file. For iterations append a version suffix (`arch-options-v2.html`). Server serves newest file by modification time.
