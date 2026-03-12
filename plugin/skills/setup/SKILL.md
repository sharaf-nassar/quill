---
name: setup
description: Configure the Quill widget connection. Run this after installing the plugin to set the widget IP address and bearer secret.
---

You are configuring the Quill widget hook. This hook reports token usage from each Claude Code turn to the Quill desktop widget over HTTP.

The widget server requires a bearer secret for authentication. The secret is stored at `~/.local/share/io.quill.toolkit/auth_secret` on the machine running the widget.

Follow these steps exactly:

1. Use AskUserQuestion to ask the user for the widget address:
   - Question: "Where is the Quill widget running?"
   - Options:
     - "This machine" — description: "The widget app is running on this same machine (localhost)"
     - "Another machine on my network" — description: "The widget is running on a different machine — you'll provide the IP address"

2. If they choose "This machine":
   - Set the URL to `http://localhost:19876`.
   - Read the secret from `~/.local/share/io.quill.toolkit/auth_secret` using the Read tool.
   - If the secret file exists, display it to the user and tell them:
     "Save this secret — you'll need it when running `/quill-hook:setup` on any other machine that should report to this widget."
   - If the secret file doesn't exist, warn the user that the widget doesn't appear to have been launched yet. The config will be saved and will work once the widget creates the secret on first launch. They can re-run `/quill-hook:setup` afterward.

3. If they choose "Another machine on my network":
   - Use AskUserQuestion to ask:
     "What is the IP address (or hostname) of the machine running the widget?"
     Provide reasonable example options like "192.168.1.100" with descriptions, but they'll likely type their own.
   - Construct the URL as `http://<their-input>:19876`
   - Use AskUserQuestion to ask:
     "What is the bearer secret from the widget machine? (Run `cat ~/.local/share/io.quill.toolkit/auth_secret` on that machine to get it)"
     Provide a single option "I don't have it yet" with description "Skip for now — the hook will fail until a valid secret is configured. Re-run /quill-hook:setup when you have it."
   - If they provide a secret, use it. If they choose "I don't have it yet", set secret to empty string and warn them.

4. Then ask for an optional hostname label:
   - "What name should this machine report as in the widget?"
   - Options:
     - Use the system hostname (run `hostname -s` via Bash to get it and show it as the option label)
     - "Custom name" — description: "Choose a custom label for this machine"

5. Write the config file to `~/.config/quill/config.json` with this structure:
   ```json
   {
     "url": "http://<address>:19876",
     "hostname": "<hostname>",
     "secret": "<secret>"
   }
   ```
   Create the `~/.config/quill/` directory if it doesn't exist.
   If secret is empty, omit the `"secret"` field.

6. Verify connectivity:
   - First, run a health check: `curl -s -m 3 <url>/api/v1/health`
   - If the health check returns "ok" AND a secret was configured, run an authenticated test:
     `curl -s -m 3 -X POST -H 'Content-Type: application/json' -H 'Authorization: Bearer <secret>' -d '{"session_id":"setup-test","hostname":"setup-test","input_tokens":0,"output_tokens":0,"cache_creation_input_tokens":0,"cache_read_input_tokens":0}' <url>/api/v1/tokens`
   - If both succeed, tell the user setup is complete and the hook will now report token usage after each turn.
   - If the health check fails, warn the user that the widget doesn't seem reachable at that address, but the config has been saved and will work once the widget is running.
   - If the health check passes but the auth test fails, warn the user that the secret may be incorrect. They can re-run `/quill-hook:setup` to fix it.
