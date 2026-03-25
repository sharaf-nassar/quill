#!/usr/bin/env bash
# Install the Quill widget hook into Claude Code.
#
# Preferred: Use the plugin instead:
#   /plugin marketplace add sharaf-nassar/quill
#   /plugin install quill-hook@sharaf-nassar/quill
#   /quill-hook:setup
#
# Manual install (this script):
#   curl -fsSL https://raw.githubusercontent.com/sharaf-nassar/quill/main/hooks/install.sh | bash
#
# With options:
#   ... | bash -s -- --url http://<widget-ip>:19876 --hostname my-server --secret <bearer-secret>

set -euo pipefail

HOOK_URL="https://raw.githubusercontent.com/sharaf-nassar/quill/main/hooks/quill-hook.sh"
INSTALL_DIR="${HOME}/.claude/hooks"
HOOK_PATH="${INSTALL_DIR}/quill-hook.sh"
SETTINGS_FILE="${HOME}/.claude/settings.json"
CONFIG_DIR="${HOME}/.config/quill"
CONFIG_FILE="${CONFIG_DIR}/config.json"
USAGE_URL=""
HOSTNAME_LABEL=""
SECRET=""
if [ "$(uname)" = "Darwin" ]; then
    SECRET_FILE="${HOME}/Library/Application Support/com.quilltoolkit.app/auth_secret"
else
    SECRET_FILE="${HOME}/.local/share/com.quilltoolkit.app/auth_secret"
fi

while [[ $# -gt 0 ]]; do
    case $1 in
        --url) USAGE_URL="$2"; shift 2 ;;
        --hostname) HOSTNAME_LABEL="$2"; shift 2 ;;
        --secret) SECRET="$2"; shift 2 ;;
        *) echo "Unknown option: $1"; exit 1 ;;
    esac
done

# Auto-detect secret for localhost URLs if not explicitly provided
if [ -z "$SECRET" ]; then
    RESOLVED_URL="${USAGE_URL:-http://localhost:19876}"
    if echo "$RESOLVED_URL" | grep -qE '(localhost|127\.0\.0\.1)'; then
        if [ -f "$SECRET_FILE" ]; then
            SECRET=$(cat "$SECRET_FILE" 2>/dev/null || true)
            if [ -n "$SECRET" ]; then
                echo "  Auto-detected auth secret from local widget installation"
            fi
        fi
    fi
fi

echo "Installing Quill hook..."

# Detect if running from the repo (local install) vs piped from GitHub
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" 2>/dev/null && pwd)" || SCRIPT_DIR=""
REPO_ROOT="${SCRIPT_DIR:+$(dirname "$SCRIPT_DIR")}"

mkdir -p "$INSTALL_DIR"

# Copy or download hook script
if [ -n "$REPO_ROOT" ] && [ -f "$REPO_ROOT/hooks/quill-hook.sh" ]; then
    cp "$REPO_ROOT/hooks/quill-hook.sh" "$HOOK_PATH"
    echo "  Copied hook from local repo to $HOOK_PATH"
else
    curl -fsSL "$HOOK_URL" -o "$HOOK_PATH"
    echo "  Downloaded hook to $HOOK_PATH"
fi
chmod +x "$HOOK_PATH"

# Write config file
mkdir -p "$CONFIG_DIR"
python3 - "$CONFIG_FILE" "${USAGE_URL:-http://localhost:19876}" "${HOSTNAME_LABEL:-$(hostname -s 2>/dev/null || echo local)}" "$SECRET" <<'PYEOF'
import json, sys

config_path = sys.argv[1]
url = sys.argv[2]
hostname = sys.argv[3]
secret = sys.argv[4]

config = {"url": url, "hostname": hostname}
if secret:
    config["secret"] = secret

import os
fd = os.open(config_path, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
with os.fdopen(fd, "w") as f:
    json.dump(config, f, indent=2)
    f.write("\n")

print(f"  Config written to {config_path}")
print(f"    url: {url}")
print(f"    hostname: {hostname}")
if secret:
    print(f"    secret: {'*' * 8}...{secret[-4:]}")
else:
    print("    secret: (none — requests will be unauthenticated)")
PYEOF

# Copy or download observe and session-end-learn scripts
OBSERVE_PATH="${INSTALL_DIR}/quill-observe.js"
SESSION_END_PATH="${INSTALL_DIR}/quill-session-end-learn.js"

if [ -n "$REPO_ROOT" ] && [ -f "$REPO_ROOT/plugin/scripts/observe.js" ]; then
    cp "$REPO_ROOT/plugin/scripts/observe.js" "$OBSERVE_PATH"
    echo "  Copied observe hook from local repo"
    cp "$REPO_ROOT/plugin/scripts/session-end-learn.js" "$SESSION_END_PATH"
    echo "  Copied session-end-learn hook from local repo"
else
    OBSERVE_URL="https://raw.githubusercontent.com/sharaf-nassar/quill/main/plugin/scripts/observe.js"
    SESSION_END_URL="https://raw.githubusercontent.com/sharaf-nassar/quill/main/plugin/scripts/session-end-learn.js"
    curl -fsSL "$OBSERVE_URL" -o "$OBSERVE_PATH"
    echo "  Downloaded observe hook to $OBSERVE_PATH"
    curl -fsSL "$SESSION_END_URL" -o "$SESSION_END_PATH"
    echo "  Downloaded session-end-learn hook to $SESSION_END_PATH"
fi

# Merge hooks into settings.json
python3 - "$SETTINGS_FILE" "$HOOK_PATH" "$OBSERVE_PATH" "$SESSION_END_PATH" <<'PYEOF'
import json, sys, os

settings_path = sys.argv[1]
token_hook_cmd = sys.argv[2]
observe_cmd = f"node {sys.argv[3]}"
session_end_cmd = f"node {sys.argv[4]}"

if os.path.exists(settings_path):
    with open(settings_path) as f:
        settings = json.load(f)
else:
    os.makedirs(os.path.dirname(settings_path), exist_ok=True)
    settings = {}

hooks = settings.setdefault("hooks", {})

# Stop hook: token reporting + session-end learning
stop_hooks = hooks.setdefault("Stop", [])
token_found = False
session_end_found = False
for entry in stop_hooks:
    for h in entry.get("hooks", []):
        if "quill-hook" in h.get("command", ""):
            h["command"] = token_hook_cmd
            token_found = True
        if "session-end-learn" in h.get("command", ""):
            h["command"] = session_end_cmd
            session_end_found = True

if not token_found:
    stop_hooks.append({"matcher": "", "hooks": [{"type": "command", "command": token_hook_cmd}]})
if not session_end_found:
    added = False
    for entry in stop_hooks:
        for h in entry.get("hooks", []):
            if "quill" in h.get("command", ""):
                entry["hooks"].append({"type": "command", "command": session_end_cmd, "timeout": 5})
                added = True
                break
        if added:
            break
    if not added:
        stop_hooks.append({"matcher": "", "hooks": [{"type": "command", "command": session_end_cmd, "timeout": 5}]})

print("  Configured Stop hooks (token reporting + session-end learning)")

# PreToolUse + PostToolUse: observation hooks (synchronous — stdin required)
for event in ["PreToolUse", "PostToolUse"]:
    event_hooks = hooks.setdefault(event, [])
    found = False
    for entry in event_hooks:
        for h in entry.get("hooks", []):
            if "quill" in h.get("command", ""):
                h["command"] = observe_cmd
                h.pop("async", None)
                h["timeout"] = 3
                found = True
    if not found:
        event_hooks.append({
            "matcher": "*",
            "hooks": [{"type": "command", "command": observe_cmd, "timeout": 3}]
        })
    print(f"  Configured {event} hook (tool observation)")

with open(settings_path, "w") as f:
    json.dump(settings, f, indent=2)
    f.write("\n")
PYEOF

echo ""
echo "Done! Hooks installed:"
echo "  - Token reporting (Stop hook)"
echo "  - Tool observation (PreToolUse + PostToolUse hooks)"
echo "  - Session-end learning trigger (Stop hook)"
echo ""
echo "To verify: curl ${USAGE_URL:-http://localhost:19876}/api/v1/health"
echo "To reconfigure: re-run this script with --url and --hostname"
echo "To uninstall: rm $HOOK_PATH $OBSERVE_PATH $SESSION_END_PATH $CONFIG_FILE && edit $SETTINGS_FILE"
