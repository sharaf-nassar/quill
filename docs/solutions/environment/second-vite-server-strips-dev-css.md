---
title: A second Vite server on the same checkout strips all CSS from tauri dev
date: 2026-08-26
component: dev-run
tags: [vite, tauri-dev, node_modules, optimize-deps, css]
problem_type: environment
---

# A second Vite server on the same checkout strips all CSS from tauri dev

## Problem

`npm run tauri -- dev` rendered the widget as unstyled HTML: default UA button
chrome, default sans-serif type, no tokens, no layout. React mounted, IPC
resolved, and the chart drew real data, so only the stylesheet was missing.
Edits appeared not to reload.

## Root cause

Two Vite dev servers were running with `cwd=/home/mamba/work/quill`:

```text
2386320  vite                      # spawned by tauri dev, 00:36 today
2394575  vite --port 8182          # orphaned, started four days earlier
```

The stray had been reparented to `systemd --user` after its shell exited, and
port 8182 appears nowhere in the repository — it was a leftover manual run.

Both servers share one `node_modules/.vite`. Dependency optimization writes to
`deps_temp_<hash>/` and then atomically renames it to `deps/`. Two servers
racing that rename leave the temporaries behind and no `deps/` at all:

```bash
$ ls node_modules/.vite/deps
ls: cannot access 'node_modules/.vite/deps': No such file or directory
$ ls node_modules/.vite | wc -l
29          # all deps_temp_*, several written in the last two minutes
```

Vite serves project sources directly and only pre-bundled dependencies from
`deps/`, which is why the app still ran. In dev, `import "./styles/index.css"`
is served as a JS module that injects a `<style>` tag; that request resolved
against the missing cache, so every rule silently vanished while the component
tree kept rendering.

`npm test` compounded this. Eleven test files call `createServer()` and one
calls `build()`; each loaded `vite.config.ts` and each defaulted `cacheDir` to
the same `node_modules/.vite`. Their config hashes differ from the dev server's,
so every run re-optimized the shared cache — hence the recurring
`Re-optimizing dependencies because vite config has changed`. Measured: one test
file added exactly one orphaned `deps_temp_*` to the dev server's cache; a full
run left twelve and no `deps/`.

## What didn't work

- Reading the CSS for a syntax error: brace depth balanced, and
  `npx vite build` emitted `index-*.css` at 48.77 kB.
- Checking `src/main.tsx` for a dropped `import "./styles/index.css"`: present
  and unchanged.

Both were symptom-level checks; neither could explain CSS-only failure with a
working module graph.

## Fix

```bash
kill 2394575                 # stop the orphaned server
rm -rf node_modules/.vite    # drop the poisoned cache
```

Then restart `npm run tauri -- dev`, which re-optimizes into a clean `deps/`.

## Repository fix

The `npm test` half is now closed in `vite.config.ts`: a test-runner check moves
`cacheDir` to `<tmpdir>/quill-vite-test-cache`, so test servers cannot touch
`node_modules/.vite`.

```ts
const isTestRunner =
  Boolean(process.env.NODE_TEST_CONTEXT) ||
  Boolean(process.argv[1]?.endsWith(".test.mjs"));
```

Node sets `NODE_TEST_CONTEXT` in every `node --test` child, and the filename
check is a second repo-owned signal in case that variable changes. Neither needs
a shell prefix, so it holds on Windows. The temp dir is used rather than another
`node_modules` folder because these servers set `optimizeDeps.noDiscovery` and
never commit a `deps/` — they leave ~39MB of temporaries per run that nothing
reads twice. Verified: `node_modules/.vite/deps` holds 54 entries before and
after a full `npm test`, with no repo growth and no change in suite runtime.

## A second trigger: adding a dependency mid-session

Installing an npm package while `tauri dev` runs makes Vite discover it and
re-optimize. Windows already open still hold module URLs from the previous
browser hash, so a lazily-imported chunk can fail to load. Adding
`@tauri-apps/plugin-clipboard-manager` this way left the Settings window
entirely blank.

Blank, not broken-looking: React unmounts the whole tree when nothing catches
the error, and the root had only a `Suspense` boundary, which answers pending
rather than rejected. The window painted the page background and nothing else.
Editing any file in the failed chunk forced a re-transform and it recovered,
which is what made it look intermittent.

`src/components/RootErrorBoundary.tsx` now wraps the routed view and renders the
error text plus a Reload action, so this class of failure is legible in one
glance instead of an hour. The trigger itself is inherent to Vite: **restart the
dev server after installing a package.**

## Prevention

- `AGENTS.md` already limits the repo to one running Quill because of the fixed
  provider ports. The same single-writer rule still applies to Vite by hand: one
  dev server per checkout, because `node_modules/.vite` has no cross-process
  locking. The fix above only covers the automated collisions.
- Before debugging a dev-run anomaly, run
  `ps aux | grep vite` and confirm exactly one server owns the checkout.
- A missing `node_modules/.vite/deps` alongside many `deps_temp_*` directories
  is the signature of that race, not of a corrupt install.
- Any new tool that starts Vite against this repo must set its own `cacheDir`.
- Restart `tauri dev` after any `npm install`; open windows cannot survive a
  dependency re-optimization.
- A window that renders as a flat background with no content is a caught-nothing
  render failure, not a CSS problem. Since the root error boundary landed it
  should be impossible — if it recurs, something mounted outside the boundary.
