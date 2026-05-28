#!/usr/bin/env bash
#
# Platform-aware Rust lint gate for pre-commit.
#
# `cargo clippy` only ever compiles for the HOST target triple, so
# platform-gated code (`#[cfg(target_os = "...")]`) is type-checked solely on a
# matching host. A macOS-only `libc::proc_pidpath` call therefore compiles clean
# on Linux (the block is cfg'd out) and only breaks on the macOS Release build.
#
# This wrapper detects the host OS and lints the code that host can actually
# compile, so cfg-gated regressions are caught by whichever platform's developer
# commits them instead of slipping through to the release pipeline. Mirrors the
# CI gate's `--all-targets -- -D warnings` so the local check is never laxer.
set -euo pipefail

cd "$(git rev-parse --show-toplevel)/src-tauri"

run_clippy() {
    echo "==> clippy ($1)"
    cargo clippy --all-targets -- -D warnings
}

case "$(uname -s)" in
Darwin)
    # Host is an Apple target: the native compile covers cfg(target_os = "macos").
    run_clippy "macOS native"
    ;;
Linux)
    # The native compile covers cfg(target_os = "linux").
    run_clippy "Linux native"
    # cfg(target_os = "macos") code cannot be compiled here: objc2's build
    # script compiles Objective-C with Apple-only clang flags that Linux cc
    # rejects. That code is verified by macOS committers and the Release build.
    echo "note: macOS-gated Rust is not lintable on Linux; covered on macOS / in Release."
    ;;
*)
    run_clippy "host"
    ;;
esac
