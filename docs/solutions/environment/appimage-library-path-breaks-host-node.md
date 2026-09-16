---
title: AppImage library path breaks host Node and Pi enablement
date: 2026-09-16
component: Provider CLI process environment
tags: [AppImage, linux, pi, node, subprocess]
problem_type: environment
---

## Symptom

Pi enablement on `malix.lan` failed with `pi --version exited with non-zero
status`, although Pi 0.85.1 worked from an SSH login shell. Quill detected the
CLI and agent directory but persisted an error status. No Quill extension
had been installed.

## Cause

The running AppImage exported `LD_LIBRARY_PATH` containing its mounted
`usr/lib`. Pi's system Node loaded the AppImage's `libnghttp2.so.14` instead
of the host library. Host `libnode.so.127` required a symbol absent from the
bundled library:

```text
node: symbol lookup error: /usr/lib/aarch64-linux-gnu/libnode.so.127: undefined symbol: nghttp2_option_set_no_rfc9113_leading_and_trailing_ws_validation
```

This failed before Pi could print its version. PATH lookup, Pi's version
floor, and extension-directory permissions were not the problem.

## Verification

A read-only probe copied the running Quill process's environment from
`/proc/<pid>/environ` and launched the same `pi --version` executable:

- Inherited Quill environment: exit 127 and the symbol error above.
- Same environment with only `LD_LIBRARY_PATH` removed: exit 0, `0.85.1`.
- `ldd /usr/bin/node` confirmed bundled `libnghttp2.so.14` in the first case
  and `/usr/lib/aarch64-linux-gnu/libnghttp2.so.14` in the second.

No application restart, package replacement, or configuration edit was
needed for this reproduction.

## Prevention

Build host-tool processes with `config::external_command`, including the
login shells that resolve provider CLIs and package-manager prefixes. It
removes `LD_LIBRARY_PATH` from child commands on Linux AppImage launches.
Convert the standard command to Tokio for async callers. Preserve existing
PATH augmentation and command-specific environment restrictions.

Do not unset the variable globally: Quill and its updater relaunch may need
the bundled libraries. Do not patch just the Pi version probe: OAuth,
Claude, Codex, and MCP verification inherit the same environment otherwise.
Ordinary non-AppImage launches retain user-supplied library paths.
