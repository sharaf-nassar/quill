# Research: Skills Breakdown Tab

## Decision: Persist Recognized Skill Uses During Session Indexing

Skill usage will be extracted during transcript indexing and stored as first-class rows. Aggregation queries will read those stored rows instead of scanning free-form transcript text or shell command strings at display time.

**Rationale**: The breakdown panel needs fast timeframe and all-time counts. Persisted rows also make the reliability boundary explicit: only extracted events with a provider and skill identity can count.

**Alternatives considered**:

- Query `tool_actions` directly every time. Rejected because reliable skill identity requires parsing multiple tool input shapes and command text, which would be slow and harder to validate in an aggregate query.
- Infer usage from assistant text. Rejected because prose may mention a skill without using it.
- Count skills listed in injected instructions. Rejected because available skills are not used skills.

## Decision: Recognize Skill Use From Explicit Skill File Access

The first implementation will count a skill use only when an indexed tool action explicitly reads a `SKILL.md` file under a known skill directory shape. The skill name is derived from the parent directory of `SKILL.md`.

**Rationale**: Current Codex and Claude Code workflows both require reading the skill body before following it. A concrete `SKILL.md` load is a stronger signal than natural-language statements, command names, provider prompt metadata, or skill-file maintenance edits.

**Alternatives considered**:

- Add heuristic matching for assistant phrases such as "Using X". Rejected because it is not reliable enough for analytics.
- Count every path under a skill root. Rejected because only read-like `SKILL.md` loads establish that the skill body was loaded.
- Detect only Claude Code-specific skill metadata. Rejected because the feature must cover Codex and Claude Code in one provider-aware model.

## Decision: Keep Ambiguous Activity Out of Named Totals

Rows without a recognized skill name, provider, session id, and timestamp will not be stored in the skill-use table and will not contribute to counts.

**Rationale**: The user asked for reliable counts. Showing lower but defensible totals is better than inflating counts with guessed skill names.

**Alternatives considered**:

- Add an "Unknown" skill row. Rejected because it would mix unavailable telemetry with actual skill usage and make provider comparison less useful.
- Backfill by scanning assistant prose. Rejected for the same reliability reason.

## Decision: Aggregate Provider Counts in One Command

The backend will expose one `get_skill_breakdown` command with `days`, `provider`, `all_time`, and optional `limit` inputs. Rows will include total count plus provider-specific counts.

**Rationale**: The UI can switch All/Codex/Claude Code badges without needing separate command shapes, and All view can still explain combined totals if needed later.

**Alternatives considered**:

- Separate commands for all-time and timeframe data. Rejected because scope is a filter, not a separate resource.
- Fetch raw skill-use events to the frontend. Rejected because the breakdown only needs aggregate counts.

## Decision: Refresh Skills on Session Index Updates

The Skills breakdown will refresh on the existing session-index update signal and timeframe/provider/all-time state changes.

**Rationale**: Skill-use rows are derived from indexed sessions, not live token bucket updates. This matches the existing indexing pipeline and avoids unnecessary refreshes.

**Alternatives considered**:

- Refresh only on manual tab selection. Rejected because the rest of the breakdown panel already stays live as data changes.
- Refresh on every token update only. Rejected because token events do not guarantee skill extraction changes.
