# Quickstart: Skills Breakdown Tab

## Validate Locally

1. Start Quill in development mode:

   ```bash
   pnpm tauri dev
   ```

2. Open Analytics and use the Now view.

3. In the breakdown selector, choose Skills.

4. Verify the default view:

   - Skills appears alongside Sessions, Projects, and Hosts.
   - Rows show skill names and total recognized use counts.
   - Counts are scoped to the active analytics timeframe.
   - Rows sort highest count first.

5. Toggle all-time mode.

   - Counts include recognized skill use across all indexed history.
   - Other analytics timeframe controls remain unchanged.

6. Switch provider badges.

   - All combines Claude Code and Codex recognized uses.
   - Codex shows Codex recognized uses only.
   - Claude Code shows Claude Code recognized uses only.

7. Re-index or load a session with no recognized skill usage.

   - Skills shows a scoped empty state rather than stale rows.

## Suggested Verification Commands

```bash
pnpm typecheck
cargo test --manifest-path src-tauri/Cargo.toml
lat check
```
