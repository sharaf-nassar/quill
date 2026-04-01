#!/usr/bin/env bash
# Reports token usage from the latest Codex turn to the Quill widget.

set -euo pipefail

CONFIG_FILE="${HOME}/.config/quill/config.json"

if [ ! -f "$CONFIG_FILE" ]; then
    cat >/dev/null
    exit 0
fi

CONFIG_VALUES=$(python3 -c "
import json, sys
with open(sys.argv[1]) as f:
    c = json.load(f)
print(c.get('url', ''))
print(c.get('hostname', ''))
print(c.get('secret', ''))
" "$CONFIG_FILE" 2>/dev/null || true)

USAGE_URL=$(echo "$CONFIG_VALUES" | sed -n '1p')
HOSTNAME_ID=$(echo "$CONFIG_VALUES" | sed -n '2p')
SECRET=$(echo "$CONFIG_VALUES" | sed -n '3p')

if [ -z "$USAGE_URL" ] || [ -z "$SECRET" ]; then
    cat >/dev/null
    exit 0
fi

HOSTNAME_ID="${HOSTNAME_ID:-$(hostname -s 2>/dev/null || echo local)}"

HOOK_INPUT=$(cat)

HOOK_FIELDS=$(echo "$HOOK_INPUT" | python3 -c "
import sys, json
data = json.load(sys.stdin)
print(data.get('stop_hook_active', False))
print(data.get('session_id', ''))
print(data.get('transcript_path', ''))
print(data.get('cwd') or '')
" 2>/dev/null) || exit 0

IS_ACTIVE=$(echo "$HOOK_FIELDS" | sed -n '1p')
SESSION_ID=$(echo "$HOOK_FIELDS" | sed -n '2p')
TRANSCRIPT_PATH=$(echo "$HOOK_FIELDS" | sed -n '3p')
CWD=$(echo "$HOOK_FIELDS" | sed -n '4p')

if [ "$IS_ACTIVE" = "True" ]; then
    exit 0
fi

if [ -z "$SESSION_ID" ] || [ -z "$TRANSCRIPT_PATH" ] || [ ! -f "$TRANSCRIPT_PATH" ]; then
    exit 0
fi

USAGE_JSON=$(tac "$TRANSCRIPT_PATH" | python3 -c "
import sys, json

for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    try:
        msg = json.loads(line)
    except json.JSONDecodeError:
        continue

    if msg.get('type') != 'event_msg':
        continue

    payload = msg.get('payload') or {}
    if payload.get('type') != 'token_count':
        continue

    info = payload.get('info') or {}
    usage = info.get('last_token_usage') or info.get('total_token_usage')
    if not usage:
        continue

    result = {
        'input_tokens': usage.get('input_tokens', 0),
        'output_tokens': usage.get('output_tokens', 0),
        'cache_creation_input_tokens': 0,
        'cache_read_input_tokens': usage.get('cached_input_tokens', 0),
    }
    print(json.dumps(result))
    break
" 2>/dev/null || true)

if [ -z "$USAGE_JSON" ]; then
    exit 0
fi

PAYLOAD=$(python3 -c "
import sys, json
usage = json.loads(sys.argv[1])
payload = {
    'provider': 'codex',
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

curl -s -m 2 \
    -X POST \
    -H 'Content-Type: application/json' \
    -H "Authorization: Bearer $SECRET" \
    -d "$PAYLOAD" \
    "${USAGE_URL}/api/v1/tokens" \
    >/dev/null 2>&1 || true
