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

# Contract: `get_skill_project_breakdown`

## Purpose

Return per-(project, hostname) skill usage counts for a single skill, used by the Skills expand drilldown in the analytics breakdown.

## Request

```json
{
  "skillName": "speckit-specify",
  "days": 7,
  "provider": "codex",
  "allTime": false,
  "limit": 50
}
```

## Request Fields

- `skillName`: Parent skill whose project rows are being fetched.
- `days`: Active analytics timeframe in days. Ignored when `allTime` is true.
- `provider`: Optional provider filter. Valid values are `claude`, `codex`, or `null` for All.
- `allTime`: When true, count across all indexed history.
- `limit`: Optional maximum row count after subdir merge. Backend clamps to a safe range.

## Response

```json
[
  {
    "skill_name": "speckit-specify",
    "project": "/home/user/work/quill",
    "hostname": "laptop",
    "total_count": 9,
    "claude_count": 3,
    "codex_count": 6,
    "last_used": "2026-05-12T23:10:00Z"
  }
]
```

## Response Fields

- `skill_name`: Echoes the requested skill name.
- `project`: Resolved project root after subdir merge.
- `hostname`: Machine that produced the rows; null when not captured.
- `total_count`: Count for the active provider scope after merge.
- `claude_count`: Claude Code count in the active time scope after merge.
- `codex_count`: Codex count in the active time scope after merge.
- `last_used`: Most recent recognized skill use timestamp in the active scope after merge.

## Behavior

- Filters honor the parent `get_skill_breakdown` query's time-scope and provider-filter semantics.
- Rows are keyed by `(cwd, hostname)`; `cwd IS NULL` rows are excluded so pre-reingest skill uses do not appear.
- Subdir merge folds `/a/b/c` into `/a/b` exactly like the Projects breakdown, summing counts and taking `MAX(last_used)` across folded rows.
- Post-merge rows sort by `total_count` descending, then `last_used` descending, then `project` ascending, and the `limit` cap is applied after merge.
- An empty result is valid; it means the skill has no captured cwd rows in scope.
