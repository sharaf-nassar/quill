---
name: setup
description: Configure the Quill widget connection and MCP server. Run this after installing the plugin to set the widget IP address and bearer secret.
---

You are configuring the Quill plugin. This plugin has two components:

1. **Usage hook** — reports per-turn token usage to the Quill desktop widget over HTTP
2. **MCP server** — lets you query session history, search past conversations, and analyze usage patterns

The widget server requires a bearer secret for authentication. The secret is stored at `~/.local/share/com.quilltoolkit.app/auth_secret` on the machine running the widget.

Follow these steps exactly:

## Part 1: Widget Connection

1. Use AskUserQuestion to ask the user for the widget address:
   - Question: "Where is the Quill widget running?"
   - Options:
     - "This machine" — description: "The widget app is running on this same machine (localhost)"
     - "Another machine on my network" — description: "The widget is running on a different machine — you'll provide the IP address"

2. If they choose "This machine":
   - Set the URL to `http://localhost:19876`.
   - Read the secret from `~/.local/share/com.quilltoolkit.app/auth_secret` using the Read tool.
   - If the secret file exists, display it to the user and tell them:
     "Save this secret — you'll need it when running `/quill-hook:setup` on any other machine that should report to this widget."
   - If the secret file doesn't exist, warn the user that the widget doesn't appear to have been launched yet. The config will be saved and will work once the widget creates the secret on first launch. They can re-run `/quill-hook:setup` afterward.

3. If they choose "Another machine on my network":
   - Use AskUserQuestion to ask:
     "What is the IP address (or hostname) of the machine running the widget?"
     Provide reasonable example options like "192.168.1.100" with descriptions, but they'll likely type their own.
   - Construct the URL as `http://<their-input>:19876`
   - Use AskUserQuestion to ask:
     "What is the bearer secret from the widget machine? (Run `cat ~/.local/share/com.quilltoolkit.app/auth_secret` on that machine to get it)"
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
   - If both succeed, tell the user the hook is connected and will report token usage after each turn.
   - If the health check fails, warn the user that the widget doesn't seem reachable at that address, but the config has been saved and will work once the widget is running.
   - If the health check passes but the auth test fails, warn the user that the secret may be incorrect. They can re-run `/quill-hook:setup` to fix it.

## Part 2: MCP Server Verification

7. Check that `uv` is installed:
   - Run: `uv --version`
   - If `uv` is not found, tell the user:
     "The Quill MCP server requires `uv` (Python package manager). Install it with: `curl -LsSf https://astral.sh/uv/install.sh | sh`
     Then re-run `/quill-hook:setup`."
   - If `uv` is found, proceed to step 8.

8. Verify the MCP server can start:
   - Run: `uv run --directory ${CLAUDE_PLUGIN_ROOT}/mcp python -c "from server import mcp; print('ok')"`
   - If it succeeds, tell the user: "The Quill MCP server is ready. It provides 12 tools for querying your session history, searching past conversations, and analyzing usage patterns. The MCP server starts automatically — no additional configuration needed."
   - If it fails, show the error and suggest: "Try running `uv sync --directory ${CLAUDE_PLUGIN_ROOT}/mcp` to install dependencies, then re-run `/quill-hook:setup`."

## Part 3: CLAUDE.md MCP Instructions

9. Add Quill MCP usage instructions to the user's global `~/.claude/CLAUDE.md` so Claude knows when and how to use the tools:
   - Read `~/.claude/CLAUDE.md` (create it if it doesn't exist).
   - Search for an existing `### Session History Search (Quill MCP)` section.
   - If the section already exists, **replace it entirely** (from the `###` heading up to but not including the next `###` or `##` heading) with the block below.
   - If the section does not exist, **append** the block below. Place it after any existing `###` subsections under `## Shortcuts` if that section exists, otherwise append at the end of the file.
   - The block to insert (do NOT modify this content):

   ~~~
   ### Session History Search (Quill MCP)

   - **Session History Tool**: Use the Quill MCP tools to search past Claude Code session history.
     Quill indexes all sessions into a searchable database with full-text search across conversation
     content, code changes, commands run, and tool usage.

   - **MCP Tools** (use these directly):
     - `mcp__plugin_quill-hook_quill__list_projects` — List all projects with session counts
     - `mcp__plugin_quill-hook_quill__list_sessions` — List sessions with metadata (filter by project, date)
     - `mcp__plugin_quill-hook_quill__get_session_overview` — Preview a session (first message, tools, files)
     - `mcp__plugin_quill-hook_quill__search_history` — Full-text search across all session history
     - `mcp__plugin_quill-hook_quill__get_session_context` — Get surrounding messages around a search hit
     - `mcp__plugin_quill-hook_quill__get_file_history` — All tool actions on a file across sessions
     - `mcp__plugin_quill-hook_quill__get_branch_activity` — Work done on a specific git branch
     - `mcp__plugin_quill-hook_quill__find_related_sessions` — Sessions that share files with a given session
     - `mcp__plugin_quill-hook_quill__get_token_usage` — Token usage analytics by period (1h/24h/7d/30d)
     - `mcp__plugin_quill-hook_quill__get_learned_rules` — Learned behavioral rules from past sessions
     - `mcp__plugin_quill-hook_quill__get_tool_details` — Full tool input/output for a specific message
     - `mcp__plugin_quill-hook_quill__get_index_status` — Index and database health stats

   - **Workflow**: Use progressive disclosure — browse (`list_projects`/`list_sessions`) → search
     (`search_history`) → cross-reference (`get_file_history`/`get_branch_activity`) → drill down
     (`get_session_context`/`get_tool_details`).

   - **Use When**: User asks about past sessions, previous work, conversation history, "what did we do",
     token usage/costs, or which sessions touched a specific file or branch.

   - **Do NOT**: Read raw JSONL session logs from `~/.claude/projects/` — use Quill instead.
   ~~~

   - Tell the user: "Added Quill MCP instructions to ~/.claude/CLAUDE.md — Claude will now know when and how to use the session history tools automatically."

## Summary

10. Print a summary:
   ```
   Setup complete!

   Hook:      Reports token usage to <url> as "<hostname>"
   MCP:       12 tools for querying session history (auto-starts with Claude Code)
   CLAUDE.md: MCP usage instructions added to ~/.claude/CLAUDE.md

   Available MCP tools:
   - list_projects / list_sessions / get_session_overview — browse sessions
   - search_history / get_session_context — search and drill into conversations
   - get_file_history / get_branch_activity / find_related_sessions — cross-reference
   - get_token_usage / get_learned_rules — analytics
   - get_tool_details — inspect full tool input/output
   - get_index_status — check search index health
   ```
