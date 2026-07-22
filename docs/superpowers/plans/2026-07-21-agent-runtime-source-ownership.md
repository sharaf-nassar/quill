# Agent Runtime Source Ownership Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Accurately retain and sum Claude and Codex parent/sub-agent runtime by making each canonical transcript source own one atomic five-table analytics snapshot.

**Architecture:** Add source-aware SQLite storage plus a provider-neutral transcript analytics reconciler built on retained-source inventory and a shared chain-root graph. Parse each source into memory, resolve its root, atomically replace only that source, and keep search and analytics scheduling independent. Durable registry, origin mapping, suppression, complete-root pruning, and resumable backfill preserve lifecycle correctness.

**Tech Stack:** Rust, Tauri 2, rusqlite/SQLite partial indexes, serde_json JSONL parsing, SHA-256 fingerprints, React/TypeScript, lat.md.

**Testing note:** Project policy forbids new automated test code without explicit user request. Each task uses existing tests, disposable transcript/database fixtures, schema queries, and build/static checks.

---

## File Structure

- Create `src-tauri/src/transcript_identity.rs`: shared provider-native chain metadata, Codex two-pass identity resolution, and provider-neutral root graph.
- Create `src-tauri/src/transcript_analytics.rs`: in-memory snapshot parsing, fingerprint reconciliation, startup/live scheduling, descendant restamping, and complete-root prune coordination.
- Modify `src-tauri/src/model_usage.rs`: consume shared identity/root APIs instead of private Codex and graph implementations.
- Modify `src-tauri/src/sessions.rs`: expose retained inventory, produce source-owned snapshots, keep Tantivy indexing independent, and remove five-table session-wide mutation.
- Modify `src-tauri/src/storage.rs`: migration 30, registry/origin data types, atomic snapshot replacement, prune/suppress/delete, live writes, and chain-based runtime query.
- Modify `src-tauri/src/lib.rs`: register modules and own startup/live transcript-analytics runner state.
- Modify `src-tauri/src/server.rs`: source-key analytics admission/retry plus transactional source-less message/hook writes.
- Modify `src/components/analytics/NowTab.tsx`: explain additive parent/sub-agent runtime.
- Modify `lat.md/backend.md`, `lat.md/data-flow.md`, and `lat.md/features.md`: document schema, reconciliation, deletion, and UI semantics.

## Commit Protocol

Before every task commit, run that task's exact `git add` command, then `pre-commit run --all-files`. Expected: `Passed` for every hook. If formatting changes files, re-stage and rerun. Use exactly one bare `git commit` command with literal `-m` text; no shell wrappers or compound commands.

---

### Task 1: Add source-aware schema and durable registries

**Files:** Modify `src-tauri/src/storage.rs` (`Storage::init`, after migration 29; source analytics input/storage structs near `SessionEventInput`).

- [ ] **Step 1: Add migration 30 as one transaction**

Rebuild `session_events`, `response_times`, `tool_actions`, `skill_usages`, and `hook_invocations`. Preserve every existing payload/attribution column, add nullable `source_key`, required non-empty `chain_id`, nullable `parent_chain_id`, and keep `session_id` required as resolved root. Add required `event_key` to `session_events` and `action_key` to `tool_actions`. Add `CHECK` constraints:

```sql
CHECK (length(session_id) > 0),
CHECK (length(chain_id) > 0),
CHECK (source_key IS NULL OR length(source_key) > 0)
```

Clear legacy rows from the first four tables and Claude `hook_invocations`. Copy legacy Codex hooks with `source_key=NULL`, `chain_id=session_id`, and `parent_chain_id=NULL`. Create `transcript_analytics_sources` with `(provider, source_key)` primary key and the approved root, path, fingerprint, last-good identity/origin, generation, status, timestamps, error, and suppression columns. Create `live_analytics_sessions(provider, session_id, project, cwd, hostname, updated_at)` with the composite primary key. Set `transcript_analytics_reingest_pending=1` before committing schema version `30`.

- [ ] **Step 2: Add exact partial identities and source lookup indexes**

```sql
CREATE UNIQUE INDEX uidx_se_owned ON session_events(provider,source_key,event_key) WHERE source_key IS NOT NULL;
CREATE UNIQUE INDEX uidx_se_live ON session_events(provider,session_id,event_key) WHERE source_key IS NULL;
CREATE UNIQUE INDEX uidx_rt_owned ON response_times(provider,source_key,chain_id,timestamp) WHERE source_key IS NOT NULL;
CREATE UNIQUE INDEX uidx_rt_live ON response_times(provider,session_id,chain_id,timestamp) WHERE source_key IS NULL;
CREATE UNIQUE INDEX uidx_ta_owned ON tool_actions(provider,source_key,action_key) WHERE source_key IS NOT NULL;
CREATE UNIQUE INDEX uidx_ta_live ON tool_actions(provider,session_id,action_key) WHERE source_key IS NULL;
CREATE UNIQUE INDEX uidx_su_owned ON skill_usages(provider,source_key,message_id,skill_name,skill_path,timestamp) WHERE source_key IS NOT NULL;
CREATE UNIQUE INDEX uidx_su_live ON skill_usages(provider,session_id,message_id,skill_name,skill_path,timestamp) WHERE source_key IS NULL;
CREATE UNIQUE INDEX uidx_hi_owned ON hook_invocations(provider,source_key,chain_id,timestamp,hook_identity) WHERE source_key IS NOT NULL;
CREATE UNIQUE INDEX uidx_hi_live ON hook_invocations(provider,session_id,chain_id,timestamp,hook_identity) WHERE source_key IS NULL;
```

Also create `idx_session_events_provider_source`, `idx_response_times_provider_source`, `idx_tool_actions_provider_source`, `idx_skill_usages_provider_source`, and `idx_hook_invocations_provider_source` on `(provider, source_key)`. Add registry indexes `idx_tas_root_generation(provider,source_root_key,seen_generation)`, `idx_tas_session(provider,analytics_session_id)`, `idx_tas_project(project)`, `idx_tas_cwd(cwd)`, and `idx_tas_host(hostname)`.

- [ ] **Step 3: Verify migration on a disposable database**

Run: `cargo check --manifest-path src-tauri/Cargo.toml --lib`
Expected: `Finished` with no errors.

Run the existing storage test target: `cargo test --manifest-path src-tauri/Cargo.toml --lib storage::tests`
Expected: `test result: ok.`

Create a disposable directory and back up the current database with Python's
built-in SQLite module (the `sqlite3` CLI is not installed):

```bash
export AGENT_RUNTIME_TMP
AGENT_RUNTIME_TMP="$(mktemp -d /tmp/quill-agent-runtime-migration.XXXXXX)"
python3 -c 'import os, sqlite3; source=os.path.expanduser("~/.local/share/com.quilltoolkit.app/usage.db"); target=os.path.join(os.environ["AGENT_RUNTIME_TMP"], "usage.db"); src=sqlite3.connect(f"file:{source}?mode=ro", uri=True); dst=sqlite3.connect(target); src.backup(dst); dst.close(); src.close()'
QUILL_DEMO_MODE=1 QUILL_DATA_DIR="$AGENT_RUNTIME_TMP" npm run tauri dev
```

After startup completes, stop it and inspect the disposable database:

```bash
python3 -c 'import os, sqlite3; path=os.path.join(os.environ["AGENT_RUNTIME_TMP"], "usage.db"); conn=sqlite3.connect(path); print(conn.execute("SELECT sql FROM sqlite_master WHERE name = ?", ("transcript_analytics_sources",)).fetchone()[0]); print(conn.execute("PRAGMA user_version").fetchone()[0])'
```

Expected: registry schema prints and version is `30`; retained Codex hooks have `source_key IS NULL AND chain_id=session_id`.

- [ ] **Step 4: Commit**

```bash
git add src-tauri/src/storage.rs
pre-commit run --all-files
git commit -m "feat: add source-owned analytics schema" -m "Rebuild transcript-derived tables with source and chain identity.
Add durable source, origin, suppression, and backfill state so retained
history can be reconciled safely after migration."
```

### Task 2: Share native identity resolution and parse complete snapshots

**Files:** Create `src-tauri/src/transcript_identity.rs`, `src-tauri/src/transcript_analytics.rs`; modify `src-tauri/src/lib.rs`, `src-tauri/src/model_usage.rs`, `src-tauri/src/sessions.rs` (`extract_messages_from_jsonl`, Claude/Codex extractors, `ExtractedEvent`, `ToolAction`).

- [ ] **Step 1: Extract shared identity contracts from model analytics**

Move the proven Codex two-pass metadata rules and `SourceRootGraph` into `transcript_identity.rs` with these interfaces:

```rust
pub(crate) struct NativeChainIdentity {
    pub provider: IntegrationProvider,
    pub source_session_id: String,
    pub chain_id: String,
    pub parent_chain_id: Option<String>,
    pub is_sidechain: bool,
    pub agent_id: Option<String>,
    pub cwd: Option<PathBuf>,
}
pub(crate) fn resolve_codex_native_identity(records: &[JsonlRecord])
    -> Result<NativeChainIdentity, IdentityError>;
pub(crate) struct SourceRootGraph { /* provider-qualified nodes */ }
impl SourceRootGraph {
    pub(crate) fn from_metadata(items: impl IntoIterator<Item = NativeChainIdentity>) -> Self;
    pub(crate) fn resolve(&self, provider: IntegrationProvider, chain_id: &str)
        -> Result<String, RootGraphResolutionError>;
}
```

Expose record ordinal plus parsed JSON as `JsonlRecord`. Make `model_usage::parse_codex_model_usage_jsonl` call this resolver and remove its duplicate identity walk/root graph. Claude identity remains `agentId` for sidechains, `sessionId` for parents, with parent session lineage.

- [ ] **Step 2: Define a complete in-memory snapshot**

```rust
pub(crate) struct TranscriptAnalyticsSnapshot {
    pub source: TranscriptAnalyticsSourceState,
    pub session_events: Vec<OwnedSessionEvent>,
    pub response_times: Vec<OwnedResponseTime>,
    pub tool_actions: Vec<OwnedToolAction>,
    pub skill_usages: Vec<OwnedSkillUsage>,
    pub hook_invocations: Vec<OwnedHookInvocation>,
}
pub(crate) fn parse_transcript_analytics_source(
    source: &DiscoveredRetainedJsonlSource,
    hostname: &str,
) -> Result<ParsedTranscriptAnalyticsSource, TranscriptAnalyticsError>;
pub(crate) fn stamp_analytics_root(
    parsed: ParsedTranscriptAnalyticsSource,
    root_session_id: &str,
) -> Result<TranscriptAnalyticsSnapshot, TranscriptAnalyticsError>;
```

Compute response/idle pairs inside one source/chain. Valid zero-row snapshots remain valid. Generate stable event keys from native ids plus emitted ordinal, else `record_ordinal:event_ordinal`; tool `action_key` uses call/tool-use id, else `message_id:block_ordinal`.

- [ ] **Step 3: Complete Codex runtime coverage**

Map `event_msg.user_message` to `user_text`, `event_msg.agent_message` and assistant output text to `asst_text`, reasoning items to `asst_thinking`, `function_call`/`custom_tool_call` to `asst_tool_use`, and both output variants to `user_tool_result`. Stamp every row with child `chain_id`, native parent, resolved root `session_id`, and `is_sidechain`; never let repeated ancestor `session_meta` replace child identity. Skip unrelated metadata and reject conflicting unrelated identity.

- [ ] **Step 4: Verify parsers with disposable retained fixtures**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib model_usage::tests`
Expected: existing tests pass.

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib sessions::tests`
Expected: existing tests pass.

Use temporary Claude parent + two sibling JSONLs and a depth-three Codex rollout, run the app's startup scan against copied roots, and inspect parser diagnostics.
Expected: 3 Claude chains and 3 Codex chains retain distinct native ids; each Codex call/output pair emits `asst_tool_use`/`user_tool_result`; ancestor metadata does not rewrite the child.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/transcript_identity.rs src-tauri/src/transcript_analytics.rs src-tauri/src/lib.rs src-tauri/src/model_usage.rs src-tauri/src/sessions.rs
pre-commit run --all-files
git commit -m "feat: parse provider-neutral analytics snapshots" -m "Share Claude and Codex chain identity with model analytics and resolve
native ancestry without overwriting child identity. Emit complete
in-memory snapshots including Codex reasoning and tool-loop events."
```

### Task 3: Replace all five tables atomically by source

**Files:** Modify `src-tauri/src/storage.rs` (`insert_tool_actions`, existing table ingest helpers, new registry methods); modify `src-tauri/src/sessions.rs` (`SessionIndex::startup_scan` old SQLite block).

- [ ] **Step 1: Add the sole retained-source storage entry point**

```rust
pub(crate) fn replace_transcript_analytics_snapshot(
    &self,
    snapshot: &TranscriptAnalyticsSnapshot,
) -> Result<TranscriptAnalyticsReplacement, String>;
```

Within one `rusqlite::Transaction`, check current suppression, delete `(provider, source_key)` from all five tables, insert all snapshot vectors through transaction-only helpers, upsert last-good registry fingerprint/identity/root/origin/status/counts, then commit once. Return `SuppressedUnchanged` when suppression blocks content. Empty vectors still execute deletes. No helper may lock `self.conn`, open/commit a transaction, delete rows, or update registry.

- [ ] **Step 2: Remove retained session-wide replacement**

Remove calls to `delete_*_for_session`, `store_*_for_messages`, `ingest_response_times`, and `ingest_session_events` from transcript startup/live paths. Keep source-less APIs only for direct HTTP ingestion. Make Tantivy delete/reinsert remain search-only and independent.

- [ ] **Step 3: Verify atomicity and sibling coexistence**

Against a disposable database, ingest parent and two sibling snapshots; replace one sibling with an empty valid snapshot; force an insert constraint failure before commit.
Expected SQL invariants: parent and untouched sibling counts remain unchanged; empty replacement removes only selected source; forced failure preserves all five previous source row sets and last-good registry values.

Run: `cargo check --manifest-path src-tauri/Cargo.toml --lib`
Expected: `Finished` with no errors.

- [ ] **Step 4: Commit**

```bash
git add src-tauri/src/storage.rs src-tauri/src/sessions.rs
pre-commit run --all-files
git commit -m "fix: replace transcript analytics by source" -m "Write all five transcript-derived tables and source registry state in
one transaction. Reprocessing one parent or agent can no longer erase
sibling data, and failed replacement keeps the last-good snapshot."
```

### Task 4: Add resumable startup inventory, root restamping, and prune

**Files:** Modify `src-tauri/src/transcript_analytics.rs`, `src-tauri/src/storage.rs`, `src-tauri/src/sessions.rs` (`enumerate_retained_jsonl_sources`, `SessionIndex::startup_scan`), `src-tauri/src/lib.rs` (startup runner).

- [ ] **Step 1: Implement startup reconciliation APIs**

```rust
pub(crate) fn prepare_transcript_analytics_reconciliation(
    storage: &Storage,
    roots: &[ProviderSourceRoot],
    hostname: &str,
) -> Result<PreparedTranscriptAnalyticsReconciliation, String>;
pub(crate) fn commit_next_transcript_source(
    storage: &Storage,
    plan: &mut PreparedTranscriptAnalyticsReconciliation,
) -> Result<Option<TranscriptSourceResult>, String>;
pub(crate) fn prune_completed_transcript_root(
    storage: &Storage,
    proof: &CompletedTranscriptSourceRoot,
) -> Result<usize, String>;
```

Allocate durable generation; mark every present source seen; stage fast/content fingerprints and native identities before root resolution; seed graph with registry last-good identity. Replace new/changed sources and force-reparse descendants whose root moved. Preserve fast-unchanged snapshots. Record failures without changing last-good fields.

- [ ] **Step 2: Gate prune and backfill completion**

Only a `ProviderRootEnumerationOutcome::Complete` produces prune proof. Prune stale active registry rows and their five-table children transactionally; retain suppressed tombstones. While `transcript_analytics_reingest_pending` exists, bypass fast mtime skip and replay committed sources safely. Clear marker only after every available source and required root completes; incomplete/unavailable roots keep it set.

- [ ] **Step 3: Verify restart and failure semantics**

Run with copied roots; stop after several committed sources; restart.
Expected: marker remains after interruption, completed sources deduplicate, remaining sources continue, and marker clears only after complete inventory/prune. Make one source unreadable and one root unavailable.
Expected: last-good rows survive both; unavailable root is not pruned; a confirmed missing source under a complete root is pruned.

- [ ] **Step 4: Commit**

```bash
git add src-tauri/src/transcript_analytics.rs src-tauri/src/storage.rs src-tauri/src/sessions.rs src-tauri/src/lib.rs
pre-commit run --all-files
git commit -m "feat: reconcile retained analytics at startup" -m "Inventory complete provider roots before resolving chain ancestry and
replacing source snapshots. Resume interrupted historical backfill,
restamp descendants, and prune only from complete inventory proof."
```

### Task 5: Separate live analytics scheduling from search coalescing

**Files:** Modify `src-tauri/src/lib.rs` (runner state and enqueue API), `src-tauri/src/server.rs` (`post_session_notify`, `queue_session_notify`, retry state), `src-tauri/src/transcript_analytics.rs` (scoped reconciliation).

- [ ] **Step 1: Add source-key live queue**

```rust
#[derive(Clone, Eq, Hash, PartialEq)]
struct TranscriptAnalyticsLiveSourceKey {
    provider: IntegrationProvider,
    source_key: String,
}
pub(crate) fn enqueue_transcript_analytics_live_source(
    app: &tauri::AppHandle,
    source: DiscoveredRetainedJsonlSource,
) -> Result<(), String>;
```

Coalesce only by `(provider, source_key)`, process bounded batches, use registry graph, and enqueue every persisted descendant whose resolved root changes. Do not reuse `session_notify_key`; search remains keyed/coalesced exactly as today.

- [ ] **Step 2: Add bounded validation retry**

In `post_session_notify`, successful retained-source validation admits the canonical source to analytics and separately queues original payload for search. `Unavailable` enters a bounded retry map keyed by `(provider, candidate_path)` with existing backoff constants; promote only after validation succeeds. `Invalid` never enters analytics. Search/analytics failures log independently and never cancel the other queue.

- [ ] **Step 3: Verify simultaneous siblings**

Send rapid notifications for parent and two sibling paths, including repeated notifications for one sibling.
Expected: three analytics keys run; repeated same-file work coalesces; all sibling rows survive; Tantivy search still refreshes. Temporarily deny validation access.
Expected: candidate stays untrusted until retry validates it; no invented source key or analytics deletion occurs.

- [ ] **Step 4: Commit**

```bash
git add src-tauri/src/lib.rs src-tauri/src/server.rs src-tauri/src/transcript_analytics.rs
pre-commit run --all-files
git commit -m "fix: queue live analytics by transcript source" -m "Keep source-key analytics reconciliation independent from session-keyed
search coalescing. Retry temporarily unverifiable paths without
inventing source identity or dropping simultaneous sibling updates."
```

### Task 6: Make source-less origin and explicit deletion authoritative

**Files:** Modify `src-tauri/src/storage.rs` (`store_codex_hook_observation`, source-less message ingest, `delete_session_data`, `delete_project_data`, `delete_host_data`); modify `src-tauri/src/server.rs` (`post_session_messages`, hook background storage).

- [ ] **Step 1: Store source-less analytics and origin atomically**

Add `store_live_session_analytics(provider, session_id, origin, rows)` transaction entry point. `/sessions/messages` supplies `project` and `host`; `/hooks/observed` supplies `cwd`. Upsert `live_analytics_sessions` with `COALESCE(excluded.field, live.field)` so null never erases known origin. Source-less rows use `source_key=NULL`, `chain_id=session_id`, null parent, and stable event identity `message_uuid:event_ordinal`; reject runtime input without stable incoming identity.

- [ ] **Step 2: Rewrite root-wide deletion**

For direct session deletion, resolve all registry sources whose root equals requested `(provider, session_id)`, delete their five-table rows, and durably suppress each source in one transaction; also delete source-less rows and mapping for that exact session. For project/host deletion, capture authoritative session pairs from `live_analytics_sessions` before deletion and combine them with matching registry project/cwd/hostname sources. Delete all five tables, suppress retained sources, and delete matching mappings in the same transaction. Never infer absent project/host fields.

- [ ] **Step 3: Verify suppression and origin boundaries**

Use disposable retained and live rows. Delete a root session and rescan unchanged/changed retained files.
Expected: all parent/descendant rows stay absent and suppression remains. Delete by project and host.
Expected: only registry matches and live mappings with recorded matching fields disappear; hook-only sessions lacking hostname survive host deletion but remain removable directly.

- [ ] **Step 4: Commit**

```bash
git add src-tauri/src/storage.rs src-tauri/src/server.rs
pre-commit run --all-files
git commit -m "fix: preserve analytics deletion boundaries" -m "Record live session origin beside source-less rows and use only that
authoritative mapping for project and host deletion. Suppress every
retained source under explicit root-wide deletion."
```

### Task 7: Aggregate runtime by chain and explain additive semantics

**Files:** Modify `src-tauri/src/storage.rs` (`Storage::get_llm_runtime_stats`), `src/components/analytics/NowTab.tsx` (LLM Runtime `InsightCard`).

- [ ] **Step 1: Change runtime walker identity**

Select/order by `provider, chain_id, timestamp`; use `(provider, chain_id)` as walker key. Keep `session_id` only for distinct resolved-root session count. Preserve current 5-minute idle split, 6-hour tool-wait clamp, range, sparkline, and IPC response shape. `parent_only` remains exactly `is_sidechain = 0`. Sum finalized durations for every chain without wall-clock union.

- [ ] **Step 2: Update tooltip**

Set the description to: `Total parent and sub-agent LLM work in this window. Concurrent chains are additive; model generation, reasoning, and tool execution count together. User-idle gaps over 5 minutes are excluded, while tool waits are capped.`

- [ ] **Step 3: Verify additive result and frontend**

Insert disposable events for four distinct chains, each spanning five minutes concurrently, then invoke `get_llm_runtime_stats` for the covering range.
Expected: `total_runtime_secs=1200`, one resolved session, four turns; `parent_only` includes only parent chain.

Run: `npm run typecheck`
Expected: exit 0.

Run: `npm run lint`
Expected: exit 0.

Run: `npm run build`
Expected: Vite build completes.

- [ ] **Step 4: Commit**

```bash
git add src-tauri/src/storage.rs src/components/analytics/NowTab.tsx
pre-commit run --all-files
git commit -m "fix: sum runtime across native agent chains" -m "Group active intervals by provider-native chain while retaining root
session counts and parent-only filtering. Clarify that simultaneous
parent and sub-agent work contributes additive runtime."
```

### Task 8: Synchronize architecture docs and run full verification

**Files:** Modify `lat.md/backend.md`, `lat.md/data-flow.md`, `lat.md/features.md`; verify all implementation files from Tasks 1-7.

- [ ] **Step 1: Update lat.md design truth**

Document five-table source ownership and partial unique identities under `backend#Database#Schema`; registry, root graph, atomic replacement, resumable backfill, complete-root prune, suppression, and independent live queue under `data-flow#Session Indexing Pipeline`; additive chain runtime and tooltip under `features#Features`. Replace statements claiming Codex lacks sub-agent attribution or SQLite replacement is `(provider, session_id)`. Add focused `@lat:` references beside new migration, snapshot replacement, reconciler, and runtime query.

- [ ] **Step 2: Run backend and frontend verification**

Run: `cargo fmt --manifest-path src-tauri/Cargo.toml -- --check`
Expected: exit 0.

Run: `cargo check --manifest-path src-tauri/Cargo.toml`
Expected: `Finished` with no errors.

Run: `cargo test --manifest-path src-tauri/Cargo.toml`
Expected: `test result: ok.` for all existing targets.

Run: `npm run typecheck`
Expected: exit 0.

Run: `npm run lint`
Expected: exit 0.

Run: `npm run build`
Expected: Vite build completes.

- [ ] **Step 3: Run final fixture/database matrix**

Repeat parent + sibling Claude, depth-three Codex, one-sibling reindex, malformed/unreadable source, interrupted backfill/restart, complete-root deletion, failed-root non-prune, suppression, source-owned/source-less deduplication, live origin, and late-parent restamping scenarios. Query all five tables and registries.
Expected: no cross-source erasure or duplicate partial identities; every owned row has source/chain/root; every runtime event has stable non-empty event key; malformed/unavailable inputs retain last-good rows; four concurrent five-minute chains total 20 minutes.

- [ ] **Step 4: Validate docs and whitespace**

Run: `lat check`
Expected: all wiki links, source refs, leading paragraphs, and required code mentions pass.

Run: `git diff --check`
Expected: no output and exit 0.

- [ ] **Step 5: Commit**

```bash
git add lat.md/backend.md lat.md/data-flow.md lat.md/features.md
pre-commit run --all-files
git commit -m "docs: describe source-owned runtime analytics" -m "Record transcript lifecycle ownership, atomic reconciliation, durable
suppression, live origin mapping, and additive Claude and Codex agent
runtime semantics in the project knowledge graph."
```
