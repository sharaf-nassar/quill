#!/bin/bash
# qbuild-guard.sh — PreToolUse hook for Edit|Write|MultiEdit|NotebookEdit
# Blocks file modifications to the original project directory when a qbuild
# session is active (indicated by .qbuild-lock.* files in the repo root).
# Edits inside a worktree (sibling directory) are allowed through.

INPUT=$(cat)

FILE_PATH=$(echo "$INPUT" | jq -r '.tool_input.file_path // empty' 2>/dev/null | tr -d '\n') || exit 0
if [ -z "$FILE_PATH" ]; then
  exit 0
fi

CWD=$(echo "$INPUT" | jq -r '.cwd // empty' 2>/dev/null | tr -d '\n') || exit 0
if [ -z "$CWD" ] || [ ! -d "$CWD" ]; then
  exit 0
fi

# Resolve the main repo root from cwd (works from both main repo and worktrees).
# --git-common-dir always returns the shared .git dir even from a worktree;
# its parent is the main repo root. --show-toplevel would return the worktree
# root instead, which is why we use --git-common-dir here.
COMMON_GIT=$(cd "$CWD" && git rev-parse --git-common-dir 2>/dev/null) || exit 0
if [[ "$COMMON_GIT" = /* ]]; then
  MAIN_REPO_ROOT=$(cd "$COMMON_GIT/.." && pwd)
else
  MAIN_REPO_ROOT=$(cd "$CWD/$COMMON_GIT/.." && pwd)
fi

# Check for qbuild lock files in the main repo root
shopt -s nullglob
LOCKS=("$MAIN_REPO_ROOT"/.qbuild-lock.*)
shopt -u nullglob
if [ ${#LOCKS[@]} -eq 0 ]; then
  exit 0
fi

# qbuild is active — block if the file is inside the main repo (not a worktree)
# Resolve relative paths against CWD; realpath -m is GNU-only (unavailable on macOS)
case "$FILE_PATH" in
  /*) RESOLVED_PATH="$FILE_PATH" ;;
  *)  RESOLVED_PATH="$CWD/$FILE_PATH" ;;
esac

case "$RESOLVED_PATH" in
  "$MAIN_REPO_ROOT"/*)
    jq -n '{
      "hookSpecificOutput": {
        "hookEventName": "PreToolUse",
        "permissionDecision": "deny",
        "permissionDecisionReason": "BLOCKED: qbuild is active — all file modifications must happen inside the worktree, not the original project directory. Use WORKTREE_PATH for all edits."
      }
    }'
    exit 0
    ;;
esac

exit 0
