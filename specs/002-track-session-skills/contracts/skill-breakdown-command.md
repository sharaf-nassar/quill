# Contract: `get_skill_breakdown`

## Purpose

Return aggregate skill usage counts for the analytics Skills breakdown.

## Request

```json
{
  "days": 7,
  "provider": "codex",
  "allTime": false,
  "limit": 100
}
```

## Request Fields

- `days`: Active analytics timeframe in days. Ignored when `allTime` is true.
- `provider`: Optional provider filter. Valid values are `claude`, `codex`, or `null` for All.
- `allTime`: When true, count across all indexed history.
- `limit`: Optional maximum row count. Backend clamps to a safe range.

## Response

```json
[
  {
    "skill_name": "speckit-specify",
    "total_count": 12,
    "claude_count": 4,
    "codex_count": 8,
    "last_used": "2026-05-12T23:10:00Z"
  }
]
```

## Response Fields

- `skill_name`: Skill display name.
- `total_count`: Count for the active provider scope.
- `claude_count`: Claude Code count in the active time scope.
- `codex_count`: Codex count in the active time scope.
- `last_used`: Most recent recognized skill use timestamp in the active scope.

## Behavior

- Rows are sorted by `total_count` descending, then `skill_name` ascending.
- If `provider` is `codex`, `total_count` equals `codex_count`.
- If `provider` is `claude`, `total_count` equals `claude_count`.
- If `provider` is `null`, `total_count` equals `claude_count + codex_count`.
- Ambiguous or unidentified skill activity is excluded.
- Empty scopes return an empty array.
