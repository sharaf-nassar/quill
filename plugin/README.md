# quill-hook

Claude Code plugin that reports per-turn token usage (input, output, cache creation, cache read) to the [Quill](https://github.com/sharaf-nassar/quill) desktop widget.

## Install

```
/plugin marketplace add sharaf-nassar/quill
/plugin install quill-hook@sharaf-nassar/quill
/quill-hook:setup
```

The setup skill will ask:
1. Whether the widget is on this machine or a remote IP
2. What hostname label this machine should report as

Configuration is saved to `~/.config/quill/config.json`.

## Remote setup

When the widget runs on a different machine, the setup skill will prompt for the IP address. You can also edit the config directly:

```json
{
  "url": "http://192.168.1.50:19876",
  "hostname": "my-server"
}
```

## Requirements

- `python3` and `curl` available on PATH
- The Quill widget running (provides the HTTP server on port 19876)

## How it works

The plugin registers a Stop hook that fires after every Claude Code turn. It:
1. Reads the JSONL transcript to find the last assistant message's `usage` block
2. Extracts input_tokens, output_tokens, cache_creation_input_tokens, cache_read_input_tokens
3. POSTs the data to the widget's HTTP server (2s timeout, fails silently)

No data is sent until you run `/quill-hook:setup`.
