---
title: Partial worktree node_modules makes Knip report used binaries as unused
date: 2026-08-18
component: implement-ready
tags: [worktree, node_modules, knip, dependencies, qualification]
problem_type: environment
---

# Partial worktree node_modules makes Knip report used binaries as unused

## Problem

Final Pi qualification ran `npm run knip` in the rail worktree and reported two
unused dev dependencies:

```text
@tauri-apps/cli
eslint
```

The same command on the primary checkout returned no findings. Both packages
are used as command-line binaries by package scripts, so one tree could not be
correct while the other was wrong on unchanged source.

## Root cause

The qualification worktree had a real `node_modules/` directory containing only
Vite and Jiti caches. It did not contain Knip or the package metadata Knip uses
to resolve script binaries. Pi's inherited `PATH` still found the primary
checkout's Knip executable, so the command ran, but dependency resolution used
the incomplete worktree directory and classified `eslint` and
`@tauri-apps/cli` as unused.

Per this investigation, `node -p
"require('./node_modules/knip/package.json').version"` returned `6.31.0` on the
primary checkout and `MODULE_NOT_FOUND` in the worktree. `package.json` itself
was identical in both trees.

## What didn't work

- Editing `knip.json` or adding an ignore list would have hidden an environment
  defect and weakened the absolute dead-code gate documented in
  `lat.md/infrastructure.md:186-195`.
- `--no-gitignore` produced the same false findings because Git ignore handling
  was not the cause.
- Treating the output as repository debt would have removed packages that the
  build and lint scripts need.

## Fix

The run removed the cache-only worktree directory and replaced it with a symlink
to the fully installed primary dependency tree:

```bash
rm -rf <worktree>/node_modules
ln -s <repo>/node_modules <worktree>/node_modules
```

`npm run knip` then passed in the worktree without source or configuration
changes. No repository fix was required.

## Prevention

- Before Node-based gates, prove the worktree dependency path resolves the same
  Knip package as the primary checkout.
- If a generated cache directory appears before worktree dependency setup,
  replace it with the intended symlink rather than installing or editing Knip
  configuration.
- A tool executable found through `PATH` does not prove its dependency graph is
  resolvable from the current working directory.
