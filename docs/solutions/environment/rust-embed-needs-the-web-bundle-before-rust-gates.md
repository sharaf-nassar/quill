---
title: rust-embed needs the ignored web bundle before Rust gates
date: 2026-08-24
last_updated: 2026-08-24
component: web-ui-server
tags: [rust-embed, vite, pre-commit, clippy, worktree, build-order]
problem_type: environment
---

# rust-embed needs the ignored web bundle before Rust gates

## Problem

While integrating `quill-j76f.9`, primary-checkout `pre-commit run --all-files`
failed in clippy even though the worker's release and debug asset tests passed:

```text
error: #[derive(RustEmbed)] folder '/home/mamba/work/quill/src-tauri/../dist-web' does not exist
```

The derive is declared at `src-tauri/src/web_server/assets.rs:21-28`. The same
missing folder also made `WebBundle::get` appear absent because the derive had
not generated its implementation.

## Root cause

`dist-web/` is intentionally generated and ignored. The package and CI build
order creates it before compiling Rust, but the local pre-commit clippy hook
runs Cargo directly. A clean primary checkout therefore has no folder for
`rust-embed` to inspect at compile time.

Debug serving does not remove this compile-time requirement. Debug requests read
files from disk at runtime, but the derive is still expanded while compiling the
crate.

## What didn't work

- Re-running clippy cannot create the missing Vite output.
- Treating the generated folder as source would commit build artifacts and break
  the isolated-bundle contract.
- Removing the derive in debug builds would make release and debug compile
  different asset definitions and weaken the clean-build check.

## Fix

Build the web entry before any Rust gate that compiles
`src-tauri/src/web_server/assets.rs`:

```bash
npm run build:web
pre-commit run --all-files
```

Run `run-20260824T005217.f1pGpn` then integrated `quill-j76f.9` as
`a52cf69a38b942534059acb8f37427c018baba46`. The rerun passed clippy and the
integration gate while `dist-web/` remained ignored.

## Prevention

- After asset embedding lands, treat `npm run build:web` as a prerequisite for
  local clippy, Cargo test, and pre-commit runs from a clean checkout.
- Keep CI and Tauri build commands ordered web bundle first, Rust second.
- If clippy reports both a missing `rust-embed` folder and a missing generated
  `get` method, fix the build order rather than editing Rust imports or traits.
- Do not commit `dist-web/`; regenerate it whenever the browser entry changes.
