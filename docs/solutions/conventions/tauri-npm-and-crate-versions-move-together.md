---
title: A caret range on @tauri-apps/api silently breaks tauri dev
date: 2026-08-27
component: build
tags: [tauri, npm, cargo, versions, dev-run]
problem_type: conventions
---

# A caret range on @tauri-apps/api silently breaks tauri dev

## Problem

`npm run tauri -- dev` refused to start:

```text
Error Found version mismatched Tauri packages. Make sure the NPM package and
Rust crate versions are on the same major/minor releases:
tauri (v2.10.3) : @tauri-apps/api (v2.11.1)
```

No Tauri upgrade had been requested. The only action was `npm install` of two
plugin bindings.

## Root cause

`package.json` carried `"@tauri-apps/api": "^2.0.0"`. Installing
`@tauri-apps/plugin-opener`, whose own dependency is `@tauri-apps/api ^2.11.0`,
let npm resolve the API from 2.10.1 to 2.11.1. `Cargo.lock` stayed at
`tauri 2.10.3`, and Tauri checks that the two agree on major and minor.

A caret range is the trap. The npm API and the Rust crate are two halves of one
runtime and must move together, so any range wide enough to drift will
eventually drift — during an unrelated install, with an error that names the
symptom and not the cause.

Confirmed by diffing the lockfile against `HEAD`:

```bash
git show HEAD:package-lock.json | \
  python3 -c "import json,sys; d=json.load(sys.stdin); \
  print(d['packages']['node_modules/@tauri-apps/api']['version'])"
# 2.10.1   (2.11.1 after the plugin install)
```

## What didn't work

- `cargo update -p tauri` reported `Locking 0 packages`: the range in
  `Cargo.toml` is `"2"`, but plain `update` would not cross the minor on its
  own. `--precise` was required.
- Pinning `@tauri-apps/api` back to `~2.10.1` would have conflicted with
  `plugin-opener`'s `^2.11.0` dependency, so the fix had to go forward.

## Fix

Move all three to the same minor, then pin the npm halves:

```bash
cd src-tauri && cargo update -p tauri --precise 2.11.5
cd .. && npm i -D @tauri-apps/cli@^2.11
```

```json
"@tauri-apps/api": "~2.11.1",
"@tauri-apps/cli": "~2.11.4",
```

`npx tauri info` then reports `tauri 2.11.5`, `@tauri-apps/api 2.11.1`,
`@tauri-apps/cli 2.11.4` with no mismatch. The crate bump also carried
`tauri-build`, `tauri-runtime`, `wry`, and `tao`; `cargo clippy -D warnings`
and all 558 Rust tests passed unchanged.

## Prevention

- Keep `@tauri-apps/api` and `@tauri-apps/cli` on `~` (minor-pinned), never `^`.
- Upgrading Tauri is one deliberate operation: `cargo update -p tauri
  --precise <version>`, then move both npm pins to the matching minor.
- Before adding any `@tauri-apps/plugin-*` binding, read its
  `dependencies["@tauri-apps/api"]` — it can force a crate upgrade.
- A version-mismatch error after an unrelated `npm install` is this, not a
  corrupt install.
