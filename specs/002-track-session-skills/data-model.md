# Data Model: Skills Breakdown Tab

## SkillUsage

A durable record of one recognized skill use extracted from an indexed session transcript.

**Fields**:

- `id`: Stable storage identifier.
- `provider`: Provider that produced the session, limited to Claude Code or Codex for this feature.
- `session_id`: Provider-local session identifier.
- `message_id`: Message or synthetic tool-message identifier that contained the recognized access.
- `skill_name`: Display name derived from the parent directory of `SKILL.md`.
- `skill_path`: Full path or transcript path fragment that identified the skill file.
- `timestamp`: Timestamp of the tool action or message that accessed the skill file.
- `tool_name`: Tool/action name that produced the access, when available.
- `created_at`: Storage insertion timestamp.

**Validation rules**:

- `provider`, `session_id`, `skill_name`, and `timestamp` are required.
- `skill_name` must not be empty after trimming.
- Only read-like `SKILL.md` loads are stored; skill-file edits or patches are excluded.
- Re-indexing the same session must replace prior rows for that `(provider, session_id)` pair before inserting current extracted rows.

## SkillAggregate

A row returned to the analytics UI for a single skill after applying time scope and provider filter.

**Fields**:

- `skill_name`: Display name shown in the Skills breakdown.
- `total_count`: Total recognized uses in the active scope.
- `claude_count`: Recognized Claude Code uses in the active scope.
- `codex_count`: Recognized Codex uses in the active scope.
- `last_used`: Most recent recognized use timestamp in the active scope.

**Validation rules**:

- `total_count` equals the selected provider count for provider-filtered requests.
- `total_count` equals `claude_count + codex_count` for All requests.
- Rows sort by `total_count` descending, then `skill_name` ascending.

## SkillTimeScope

The user-selected time scope for the Skills breakdown.

**States**:

- `timeframe`: Counts only recognized skill uses within the active analytics range.
- `all_time`: Counts recognized skill uses across all indexed history.

**Transitions**:

- Default state is `timeframe`.
- Toggling all-time changes only the Skills breakdown query scope.
- Leaving and returning to the Skills tab preserves the current Skills controls while the panel remains mounted.

## SkillProviderFilter

The provider badge state for the Skills breakdown.

**States**:

- `all`: Combine Claude Code and Codex rows.
- `codex`: Count Codex rows only.
- `claude`: Count Claude Code rows only.

**Transitions**:

- Default state is `all`.
- Selecting a badge immediately refreshes or reuses the aggregate for that provider scope.
- Empty states include the selected provider scope.
