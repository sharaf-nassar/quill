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

First pass (2026-08-26): a test-runner check (`NODE_TEST_CONTEXT` or a
`.test.mjs` argv) moved test servers onto `<tmpdir>/quill-vite-test-cache`.
That closed the `npm test` half only, and the failure recurred (2026-09-01)
through callers the allowlist never named: agent-started screenshot servers
(`npx vite --port 5199`) and an ad-hoc `createServer()` script whose filename
did not end in `.test.mjs`.

Second pass, current: the rule is inverted in `vite.config.ts`
(`resolveCacheDir`). Only the bare `vite` CLI on its configured strict port —
the exact process `npm run dev` and `tauri dev`'s `beforeDevCommand` spawn —
defaults to the shared `node_modules/.vite`. Every other invocation
(programmatic `createServer()`/`build()` from tests or one-off scripts, and a
CLI run carrying a `--port` override) is routed to a private
`<tmpdir>/quill-vite-<pid>` cache. Because `strictPort` makes a second
no-override CLI die at bind, no second process can reach the shared cache
long enough to race the `deps/` rename. Detection reads `process.argv[1]`
(the `vite/bin/vite.js` entry or its `.bin/vite` shim, backslashes
normalized), so it needs no shell prefix and holds on Windows. Misclassifying
an unusual caller fails safe: a private cache only costs a re-optimize into
temp, never a poisoned dev server.

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

## A third trigger: editing vite.config.ts while dev runs

Saving `vite.config.ts` restarts the Vite server in-process and re-optimizes
the dependency cache. Two consequences, both observed 2026-09-01 while an
agent iterated on the config against a live `tauri dev`:

- An already-open window can race the re-optimization exactly like the
  `npm install` trigger above: it reloads into a graph whose CSS module
  request 404s, and renders unstyled while React keeps working.
- A restart that *fails* (config error, optimizer race that throws) exits the
  Vite process. Vite is the `beforeDevCommand`; when it dies the tauri CLI
  tears down the whole dev chain — what the operator experiences as "npm
  crashed".

Rules and mitigations:

- Process rule (now in `AGENTS.md`): land `vite.config.ts` edits, package
  installs, and anything else that forces a re-optimize while dev is
  *stopped*.
- Self-heal (dev builds only, `src/main.tsx`): after load, a window whose
  computed `--surface` token is empty reloads itself once — a sessionStorage
  latch prevents loops and clears on success. The unstyled-widget symptom now
  recovers in under a second instead of persisting until a manual reload.

## Why orphans existed at all: Ctrl+C does not kill the tauri dev chain

The stray servers were not operator carelessness. `tauri dev`'s cleanup
(tauri-apps/tauri #10343, #2794, #4262) kills its `beforeDevCommand` tree
recursively, but Ctrl+C fells the intermediate `npm`/`sh` links first, the
Vite process reparents to the user manager, and the walk finds a broken
chain. The orphan keeps port 8181; `strictPort` then blocks the next dev
run's own server while windows load from the stale one. Observed twice: an
orphan "started four days earlier" in the original incident, and an Aug 28
server still answering four days of sessions on 2026-09-01.

Repo fix in `vite.config.ts` (`tauriParentWatch`): when the server was
spawned by the tauri hook chain (`TAURI_ENV_PLATFORM` is exported to hook
commands), it polls `process.ppid` every 2s and shuts down once it differs
from the ppid recorded at startup — reparenting is exactly the orphan's
signature, and beats probing the original parent's liveness, which a
surviving `npx` wrapper would fool. Manual `npm run dev` outside tauri sets
no `TAURI_ENV_*`, so its lifecycle is untouched; in-process config-change
restarts keep the same ppid. Verified: a reparented TAURI-env server
self-terminates within one poll and logs
`tauri dev chain is gone (reparented)`, while a reparented plain server
stays up.

## Prevention

- `AGENTS.md` already limits the repo to one running Quill because of the fixed
  provider ports. The same single-writer rule now holds for Vite structurally:
  only the bare CLI on the strict port can own `node_modules/.vite`, so extra
  servers and scripts isolate themselves without anyone remembering a rule.
- Before debugging a dev-run anomaly, run
  `ps aux | grep vite` and confirm exactly one server owns the checkout.
- A missing `node_modules/.vite/deps` alongside many `deps_temp_*` directories
  is the signature of that race, not of a corrupt install.
- A new tool that starts Vite against this repo no longer needs its own
  `cacheDir` — `resolveCacheDir` assigns one — but must not pass a bare `vite`
  invocation without `--port` while dev runs (strictPort will refuse it).
- Restart `tauri dev` after any `npm install`; open windows cannot survive a
  dependency re-optimization.
- A window that renders as a flat background with no content is a caught-nothing
  render failure, not a CSS problem. Since the root error boundary landed it
  should be impossible — if it recurs, something mounted outside the boundary.
