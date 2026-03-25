#!/usr/bin/env bash
# Claude Code Stop hook — extracts token usage from the last assistant turn
# and reports it to the Quill widget HTTP server.
#
# Configuration is read from ~/.config/quill/config.json (written by
# the plugin setup skill). Falls back to env vars, then defaults.
#
# Install via plugin:
#   /plugin marketplace add sharaf-nassar/quill
#   /plugin install quill-hook@sharaf-nassar/quill
#   /quill-hook:setup

set -euo pipefail

CONFIG_FILE="${HOME}/.config/quill/config.json"

# No config file = not configured yet, skip silently
if [ ! -f "$CONFIG_FILE" ]; then
    cat > /dev/null  # drain stdin
    exit 0
fi

# Read URL, hostname, and secret from config file
USAGE_URL=$(python3 -c "
import json, sys
with open(sys.argv[1]) as f:
    c = json.load(f)
print(c.get('url', ''))
" "$CONFIG_FILE" 2>/dev/null || true)
HOSTNAME_ID=$(python3 -c "
import json, sys
with open(sys.argv[1]) as f:
    c = json.load(f)
print(c.get('hostname', ''))
" "$CONFIG_FILE" 2>/dev/null || true)
SECRET=$(python3 -c "
import json, sys
with open(sys.argv[1]) as f:
    c = json.load(f)
print(c.get('secret', ''))
" "$CONFIG_FILE" 2>/dev/null || true)

# If config exists but URL is empty/missing, skip
if [ -z "$USAGE_URL" ]; then
    cat > /dev/null
    exit 0
fi

HOSTNAME_ID="${HOSTNAME_ID:-$(hostname -s 2>/dev/null || echo local)}"

# Read hook payload from stdin
HOOK_INPUT=$(cat)

# Exit early if this is a re-entry from a previous stop hook
IS_ACTIVE=$(echo "$HOOK_INPUT" | python3 -c "
import sys, json
data = json.load(sys.stdin)
print(data.get('stop_hook_active', False))
" 2>/dev/null || echo "False")

if [ "$IS_ACTIVE" = "True" ]; then
    exit 0
fi

SESSION_ID=$(echo "$HOOK_INPUT" | python3 -c "
import sys, json
data = json.load(sys.stdin)
print(data.get('session_id', ''))
" 2>/dev/null || true)

TRANSCRIPT_PATH=$(echo "$HOOK_INPUT" | python3 -c "
import sys, json
data = json.load(sys.stdin)
print(data.get('transcript_path', ''))
" 2>/dev/null || true)

CWD=$(echo "$HOOK_INPUT" | python3 -c "
import sys, json
data = json.load(sys.stdin)
print(data.get('cwd') or '')
" 2>/dev/null || true)

# Resolve to git repository root so subdirectories are grouped under one project
if [ -n "$CWD" ] && [ -d "$CWD" ]; then
    GIT_ROOT=$(git -C "$CWD" rev-parse --show-toplevel 2>/dev/null || true)
    if [ -n "$GIT_ROOT" ]; then
        CWD="$GIT_ROOT"
    fi
fi

if [ -z "$SESSION_ID" ] || [ -z "$TRANSCRIPT_PATH" ] || [ ! -f "$TRANSCRIPT_PATH" ]; then
    exit 0
fi

# Find the last assistant message with usage data in the JSONL transcript
# Read file in reverse within Python (tac is not available on macOS)
USAGE_JSON=$(python3 -c "
import sys, json

with open(sys.argv[1]) as f:
    lines = f.readlines()

for line in reversed(lines):
    line = line.strip()
    if not line:
        continue
    try:
        msg = json.loads(line)
    except json.JSONDecodeError:
        continue

    if msg.get('type') != 'assistant':
        continue

    usage = msg.get('message', {}).get('usage')
    if usage is None:
        continue

    result = {
        'input_tokens': usage.get('input_tokens', 0),
        'output_tokens': usage.get('output_tokens', 0),
        'cache_creation_input_tokens': usage.get('cache_creation_input_tokens', 0),
        'cache_read_input_tokens': usage.get('cache_read_input_tokens', 0),
    }
    print(json.dumps(result))
    break
" "$TRANSCRIPT_PATH" 2>/dev/null || true)

if [ -z "$USAGE_JSON" ]; then
    exit 0
fi

# Build the full payload
PAYLOAD=$(python3 -c "
import sys, json
usage = json.loads(sys.argv[1])
payload = {
    'session_id': sys.argv[2],
    'hostname': sys.argv[3],
    'input_tokens': usage['input_tokens'],
    'output_tokens': usage['output_tokens'],
    'cache_creation_input_tokens': usage['cache_creation_input_tokens'],
    'cache_read_input_tokens': usage['cache_read_input_tokens'],
}
cwd = sys.argv[4]
if cwd:
    payload['cwd'] = cwd
print(json.dumps(payload))
" "$USAGE_JSON" "$SESSION_ID" "$HOSTNAME_ID" "$CWD" 2>/dev/null || true)

if [ -z "$PAYLOAD" ]; then
    exit 0
fi

# POST to the widget server (fire-and-forget, 2s timeout)
# Skip if no secret is configured — the server requires auth
if [ -z "$SECRET" ]; then
    exit 0
fi

curl -s -m 2 \
    -X POST \
    -H 'Content-Type: application/json' \
    -H "Authorization: Bearer $SECRET" \
    -d "$PAYLOAD" \
    "${USAGE_URL}/api/v1/tokens" \
    >/dev/null 2>&1 || true
