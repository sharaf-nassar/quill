# Contract: Model Observation Ingest

This contract defines the shared normalized output and lifecycle used by initial
backfill, startup reconciliation, and live transcript notifications.

## Supported inputs

Only locally retained session transcript records already within Quill's session
discovery scope are eligible. Provider adapters recognize source record shapes,
not specific model identifier values.

### Claude adapter

A recognized `assistant` record emits a `turn` observation when session, chain,
and timestamp metadata are usable.

- `message.model` is accepted only through shared raw-ID validation.
- `message.usage` dimensions are copied independently when valid.
- Missing/rejected model produces `rawModelId: null` and a turn gap.
- The root transcript uses the session ID as chain ID. A subagent transcript uses
  its `agentId` as chain and the parent session ID as analytics session.

### Codex adapter

A recognized `turn_context` record emits a tokenless `turn` observation.

- `payload.model` is accepted only through shared raw-ID validation.
- `payload.turn_id`, when present, is retained but is not required.
- Missing/rejected model produces `rawModelId: null` and a turn gap.
- `session_meta` emits native thread and `parent_thread_id` metadata; the
  reconciliation coordinator resolves `analyticsSessionId` and cross-source root
  ownership.

A recognized `event_msg` token-count record emits a `token` observation.

- Cumulative totals become deterministic per-dimension deltas:
  1. The first valid value contributes its full current value from zero.
  2. A later value at or above its previous valid baseline contributes the
     difference.
  3. A later value below its baseline starts a new segment for that dimension and
     contributes its full current value from zero with a bounded local warning.
  4. A missing/invalid dimension contributes null and leaves its baseline intact.
  5. A token row is emitted only when at least one delta is positive; valid zero
     siblings remain explicit when another dimension changes.
- Any cumulative-total object excludes `last_token_usage` from addition. A
  last-only value whose uniqueness cannot be proven leaves dimensions absent.
- The record remains `rawModelId: null`; `turn_context.model` is never carried
  forward or joined by proximity.
- Token rows contribute to coverage but not model-turn adjacency.

Quota labels, configured defaults, provider family names, and direct
`/sessions/messages` payloads without co-located transcript evidence emit no model
identity.

## Normalized observation

```ts
type NormalizedModelObservation = {
  provider: string;
  sourceKey: string;
  sourceRecordKey: string;
  sourceOrdinal: number;
  observationKind: "turn" | "token";
  sourceSessionId: string;
  analyticsSessionId: string;
  chainId: string;
  parentChainId: string | null;
  isSidechain: boolean;
  agentId: string | null;
  turnId: string | null;
  observedAtMs: number;
  rawModelId: string | null;
  inputTokens: number | null;
  outputTokens: number | null;
  cacheCreationTokens: number | null;
  cacheReadTokens: number | null;
  modelEvidence: "explicit" | "missing" | "invalid";
  tokenEvidence: "direct" | "cumulative_delta" | "unavailable";
  cwd: string | null;
  hostname: string | null;
};
```

No field derives a model family, friendly name, pricing identity, or lifecycle
status.

## Processing entry points

### Initial and retry backfill

1. Enumerate all configured Claude and Codex transcript roots.
2. Record stable owning root key, canonical path provenance, and root completion
   without interpreting transcript-native session metadata.
3. Compare source fast fingerprint, then content hash when needed.
4. Let each provider adapter parse native session, parent, chain, and observation
   metadata from the complete changed source.
5. Resolve cross-source root graphs in the reconciliation coordinator; path-layout
   hints never override conflicting transcript-native metadata.
6. Replace only that source in a short transaction.
7. Mark undiscovered sources removed only after complete root enumeration.

### Startup reconciliation

`SessionIndex::startup_scan` invokes the same fingerprint and source-replacement
path for changed transcript files. It does not clear or rebuild unrelated search,
tool, skill, or token data.

### Live notification

After request validation, the server admits every retained transcript path to a
model-reconciliation queue keyed by `(provider, canonical source_key)` before the
existing provider/session-keyed search queue can coalesce payloads. Only repeated
notifications for the same canonical source may coalesce. Parent and subagent
paths sharing a session therefore remain independent. Search coalescing remains
unchanged, and model work runs even when extracted search messages are empty.

Direct message ingestion without a retained transcript path cannot satisfy this
contract and creates no model observation.

## Idempotency

- Identical fast and content fingerprints perform no observation writes.
- Replaying identical content produces identical normalized rows.
- A changed source replaces its full prior row set in one transaction.
- A failed read preserves the last committed row set.
- Each cumulative dimension resets independently on decrease and contributes its
  current value from zero; missing dimensions retain their prior baseline. Records
  whose computed deltas are all zero emit no token row. Reset warnings remain
  bounded local diagnostics and do not make a source fail.
- Source removal deletes its rows only after complete discovery proves absence.

## Live visibility

After a successful source transaction, the backend emits
`model-analytics-updated`. The event is advisory; clients refetch authoritative
SQLite aggregates. Events may coalesce, and a fallback refresh covers a missed
notification.

## Failure behavior

Malformed JSON, unsupported shapes, missing timestamps, and invalid source,
session, or chain identity are skipped per record. Missing/invalid token
dimensions are downgraded independently so valid siblings survive. Rejected model
identity becomes a null-model turn where the record is otherwise a recognized
turn. Every case adds only bounded diagnostics and processing continues with
later records in the same source.

A source-level read failure marks that source stale/failed, retains prior valid
rows, increments failed history, and allows the rest of the run to continue.
Unexpected storage errors bubble to the runner and set a terminal partial or
failed state rather than being converted to empty evidence.
