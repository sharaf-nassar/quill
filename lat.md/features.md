# Features

Quill provides live usage monitoring, analytics, behavioral learning, session search, working-context preservation, plugin management, memory optimization, and restart orchestration.

## Live Usage View

Real-time provider-aware rate limit visualization in the main window's left pane.

Displays one row per live metric bucket (`UsageRow`) with three visualization modes controlled by a time mode selector: **pace marker** (vertical line showing current position), **dual bars** (time elapsed vs utilization), or **background fill** (color gradient). Colors indicate severity: green (<50%), yellow (50-80%), red (>=80%). Reset countdown shows time until each bucket resets. Data refreshes every 3 minutes via `fetch_usage_data()`.

Claude rows come from the Anthropic OAuth usage API, Codex rows come from `codex app-server` `account/rateLimits/read` (with transcript `token_count` data as a compatibility fallback), and MiniMax rows come from the MiniMax coding plan API at `api.minimax.io`. MiniMax is a service-only provider that requires an API key (stored in SQLite) rather than a local CLI. The live pane groups rows by provider when multiple are enabled, and it can keep rendering cached rows for a provider if live polling fails so other providers do not blank the entire view. Its in-memory usage cache is keyed only by provider identity and enabled state so transient detection churn does not dislodge a fresh snapshot. Claude reuses recently persisted snapshots across window reopens and app restarts, and a 429 response writes a short-lived silent cooldown so Quill does not retry the live endpoint on every remount or show rate-limit noise in the live pane. A 401 from the usage API is treated as a stale token and surfaces a muted "Paused" badge with cached rows still shown, not a login prompt. Claude login guidance ("Run: claude /login") is only surfaced for a confirmed logout — no local credentials AND `claude auth status` reporting `loggedIn: false` — see [[data-flow#Usage Bucket Fetching]].

Transport failures (DNS, connect refused, pre-response timeout) on either Claude or MiniMax write a per-provider network cooldown computed by [[src-tauri/src/lib.rs#compute_network_backoff]] — half-jitter exponential, 60-second base, 30-minute cap, doubled per consecutive failure. While the cooldown is active, the backend short-circuits both polling paths — the frontend `setInterval`-triggered IPC and the backend `tokio` loop — and serves cached rows without making a live HTTP request. (The frontend's `setInterval` keeps firing every 3 minutes regardless; it does not know about the cooldown, but the IPC roundtrip is cheap and returns immediately from cache.) on any successful fetch the cooldown timestamps and consecutive-failure counter clear. These transport errors are coalesced into a single "Offline — showing cached data" pill in [[src/components/UsageDisplay.tsx]] (slate palette, not red) instead of one red banner per provider. The fast offline signal itself comes from the shared [[src-tauri/src/config.rs#http_client]], which sets a 5-second connect timeout and a 15-second overall timeout so reqwest never hangs indefinitely on a dead network.

The top of the live pane now starts with a shared workload summary rail that renders once whenever at least one provider is enabled. It preserves the older `Sessions`, `Projects`, and range-scoped `Tokens` cards while aggregating those counts across the enabled providers instead of showing Codex-only activity.

That summary rail includes the same 1h/6h/12h/24h window selector and freshness indicator as the old Codex workload module, but its data now comes from provider-filtered token and session history fetched across Claude, Codex, or both.

Codex now uses the same quota-style live rows as Claude instead of a separate workload widget. The old active-session ledger is gone, and Codex reset countdowns are derived from the direct app-server rate-limit response, which also adds model-specific rows such as Codex Spark when the account exposes them. When the Codex account has a finite credit balance, it is displayed in the Codex provider header row (right-aligned next to "Usage limits").

The shared workload rail lives in [[src/components/live/LiveSummaryModule.tsx]], the provider row sections live in [[src/components/live/ProviderUsageModule.tsx]], and [[src/components/UsageDisplay.tsx]] now owns the top summary, provider grouping, provider errors, and the time-mode selector for detailed limit rows. When the container is wide enough (≥500px), provider sections display side by side via a CSS container query on `.usage-display`; below that threshold they stack vertically.

## Analytics Dashboard

Four-tab analytics view in the main window's right pane, powered by [[frontend#Custom Hooks]] and Recharts.

Most analytics data is aggregated across all LLM providers; provider-scoped controls appear only where the underlying data model can preserve reliable provider identity.

Each tab opens at its smallest available timeframe toggle by default: Now and Context default to `1h`, Trends defaults to `7d` (its smallest because week-over-week comparison requires at least a week), and Charts defaults to `1h` when no localStorage preference is saved (`quill-charts-range`).

### Now Tab

Real-time metrics dashboard with configurable time range (1h, 24h, 7d, 30d).

A two-column CSS Grid (`.insight-cards-row`) renders six insight cards in three row-pairs: row 1 pairs **LLM Runtime** (total active time across CC and Codex sessions — model generation, reasoning, and tool execution counted together; user-idle gaps over 5 minutes excluded; session count, turn count, avg per turn) with **Preserved** (context tokens written to local storage), row 2 pairs **Efficiency** (tokens-per-LOC ratio with trend and sparkline) with **Retrieved** (context tokens read back), row 3 pairs **Velocity** (LOC-per-hour with trend and sparkline) with **Routing cost** (transcript overhead from router/capture/search/MCP guidance). The grid uses a `2fr 1fr` template so the left column is twice the width of the right — left cards carry sparklines and need the room, while right cards only show a label, value, and short description. Cards are placed directly in the grid in interleaved source order (`L1, R1, L2, R2, L3, R3`) so the default `grid-auto-flow: row` plus implicit `align-items: stretch` makes both cards in each row share the height of the taller card — keeping the two visual columns aligned even though only the left column has sparklines. Right-column cards source values from [[src/hooks/useContextSavingsStats.ts]] (the same hook that powers the [[features#Analytics Dashboard#Context Tab]] minus Telemetry). All six cards carry a `?` help button that reveals an `.insight-card-tooltip` describing that metric — the tooltip is opt-in via [[src/components/analytics/InsightCard.tsx]]'s `description` prop, so a card without a `description` simply omits the button. Tooltip anchoring is position-based: `:nth-child(odd)` cards anchor `left: 0` (left column extends right), and the default `right: 0` covers the right column. The grid collapses to a single column at container width below 360px. Below the grid: a sortable breakdown panel switchable between sessions, projects, hosts, skills, and hooks (in that visual order, with Sessions selected by default since "what was I just doing" is the most common entry point). Hosts sort by `total_tokens DESC` (volume view); projects and sessions sort by `last_active DESC` (recency view), and skills default to recognized use count descending with a stable skill-name tie-break. Skills can also be re-sorted client-side by Skill, Uses, or Last used through a compact header row. The projects sort is enforced both in [[src-tauri/src/storage.rs#Storage#get_project_breakdown]]'s SQL `ORDER BY` and in the post-merge re-sort inside [[src-tauri/src/storage.rs#merge_project_subdirs]], since the subdir-folding step would otherwise override the SQL order. The panel renders the full active breakdown set in a flexing list that fills the remaining analytics pane height and scrolls internally when rows exceed that available space, avoiding separate pagination controls.

When a session row is selected, the breakdown stores both provider and session id, which keeps token history, compact token stats, and delete-session actions aligned with the right Claude or Codex transcript.

The Skills breakdown reads [[src-tauri/src/storage.rs#Storage#get_skill_breakdown]] via [[src/hooks/useBreakdownData.ts#useBreakdownData]] and shows one row per recognized skill. Recognition is intentionally conservative: session indexing stores a skill-use row only for read-like `SKILL.md` loads (Claude `Read` against a `SKILL.md` path, Claude `Skill` tool calls whose plugin prefix is stripped to match Codex's bare folder names, or Codex read commands such as `sed`/`cat`), and ambiguous mentions or skill-file edits are excluded. Skills default to all indexed history (the `∞ ALL TIME` chip is active on mount) because the skill set evolves slowly and the longer scope produces a more useful row count than the narrow Now window. Two controls live beneath the breakdown mode tabs: a left-aligned underline-indicator filter strip (`All / Codex / Claude`) that scopes counts by provider without touching the Now range, and a right-justified outlined uppercase `∞ ALL TIME` chip that toggles back to the active timeframe. A compact header row labels the Skill, Uses, and Last used columns; each title sorts the current client-side rows and toggles direction when clicked, while the default remains Uses descending. Provider breakdown is communicated through the filter strip rather than inline sub-counts, since one of `codex_count` or `claude_count` is typically zero and the dual labels added more noise than signal. Every skill row renders the disclosure affordance and lazily fetches its project drilldown when opened, including skills with only one recorded project; recognized uses with no captured `cwd` render as a `No project data` sub-row so expanded counts remain additive. The disclosure is a tiny borderless hairline caret shared by expandable breakdown rows.

The Hooks breakdown (feature 009) reads [[src-tauri/src/storage.rs#Storage#get_hook_breakdown]] via [[src/hooks/useBreakdownData.ts#useBreakdownData]] and shows one row per canonicalized hook identity. It mirrors the Skills controls — same `All / Codex / Claude` filter strip and `∞ ALL TIME` chip, independent state so they do not collide with the Skills filter — plus a `?` help affordance that explains the Claude/Codex tracking asymmetry. Claude rows are per-script: extracted from JSONL `attachment` records via [[src-tauri/src/sessions.rs#extract_hook_invocation_from_attachment]] and canonicalized via [[src-tauri/src/sessions.rs#canonicalize_hook_identity]] so Quill-managed paths collapse to `quill:<basename>`, `${CLAUDE_PLUGIN_ROOT}/<dir>/<file>` stays verbatim, other paths reduce to basename, and missing-command records fall back to `hookName`. Codex rows are per-event because Codex rollouts do not log hook executions — the deployed `hook-observe.cjs` observer captures every event firing via `POST /api/v1/hooks/observed` but cannot attribute individual sibling scripts since Codex registers multiple scripts per event. Rows backed by Quill-deployed scripts carry a green `QUILL` chip parallel to the purple `AGENT` chip the Sessions breakdown uses, so users see Quill telemetry overhead alongside their own hooks rather than hidden. The breakdown defaults to ALL TIME on mount because hook installations evolve slowly; provider scoping filters the visible rows so switching to `Claude` hides rows with `claude_count = 0` and renders the Claude-only count, the `Codex` chip mirrors that for `codex_count`, and `All` displays `total_count`. No per-project drilldown today; the schema preserves `cwd` and the `is_sidechain` / `agent_id` columns for future disclosure layers.

Sessions tab rows whose backend rollup reports `has_subagents = true` render the shared hairline disclosure control at the start of the row; ordinary session rows omit the disclosure slot so ids start at the normal row padding. Clicking the control expands an indented tree of sub-agent rows below the parent: 24px-per-depth indent, a `└─` unicode tree-line drawn in `breakdown-tree-guide`, a purple "AGENT" chip (`#c084fc`) to distinguish each child from the blue "CLAUDE" chip on the parent, the first 8 chars of the `agent_id`, and the same metric columns as the parent row (no status badge — sub-agents are point-in-time runs). Sub-agents are fetched lazily on first expand via [[src/hooks/useSessionSubagents.ts#useSessionSubagents]], which caches results per `(provider, session_id)` so collapse/re-expand never refetches. Rendering is recursive through [[src/components/analytics/BreakdownPanel.tsx#SubagentRow]] and depth-bounded by `SUBAGENT_MAX_DEPTH = 10` to defend against pathological `parent_uuid` cycles.

### Trends Tab

Week-over-week comparison charts for token usage trends, code velocity, and cache efficiency.

### Charts Tab

Composite Recharts visualization with three synchronized charts: tokens, code changes, and cache efficiency.

Crosshair context synchronizes tooltip position across chart components. Lazy-loaded with React Suspense to reduce initial bundle size.

### Context Tab

Context savings analytics show how much large working context Quill kept out of active LLM transcripts.

The tab appears in the Analytics tab bar only while context preservation is enabled or historical context-savings events exist. It stays available even before token snapshots exist, because context telemetry is independent of provider token polling.

`src/components/analytics/ContextSavingsTab.tsx` renders a compact 1h/24h/7d/30d view with a four-column stats strip whose semantics map to the event taxonomy: **Preserved** (tokens written to the context store, with a retention subtitle showing `X% reused · N/M sources`), **Retrieved** (tokens pulled back via `quill_get_context_source`), **Routing cost** (transcript tokens injected by router/capture guidance and search snippets), and **Telemetry** (count of hook observation events). Each headline card has a small `?` help button (`.context-stat-help`) absolutely positioned in its top-right corner; hovering or focusing the button reveals a sibling `<span class="context-stat-tooltip">` that explains that card's slice of the event taxonomy. The strip drops `overflow: hidden` so the tooltip can escape its rounded clip, and edge cards anchor the tooltip to `left: 0` / `right: 0` (with parallel `nth-child(odd|even)` rules in the 2-column container query) so they stay inside `analytics-content`'s horizontal clip. Visibility is driven by the CSS adjacent-sibling selector `.context-stat-help:hover ~ .context-stat-tooltip`, which works in every webview without `:has()`. The retention ratio is `sources_retrieved / sources_preserved` over distinct `source_ref` values within the selected window, clamped to `[0, 1]`, computed in [[src-tauri/src/storage.rs#apply_retention_metrics]] from the `CONTEXT_SAVINGS_RETENTION_SQL` CTE. The trend chart stacks the still-saved portion of preserved tokens against the already-returned portion in the same column so each bar's height represents the per-bucket preservation throughput rather than a sum that double-counts. The breakdown table places a category dot, the event-type label, an inline provider tag, the source name, and right-aligned numeric columns over a relative-magnitude background fill scaled to the largest event count, and a single-line event log where each entry shows time, provider, source, reason, a directional byte indicator (→ indexed, ← returned, · input), and the token estimate. The magnitude fill is implemented as a `::before` pseudo-element driven by `--bar-width` and `--bar-bg` custom properties, which keeps the bar from competing with grid items for track sizing. Confidence is hidden when the estimate is exact (the deterministic `ceil(bytes / 4)` case) and surfaced only when the source reports lower confidence. `src/hooks/useContextSavingsStats.ts` listens for the `context-savings-updated` event and invokes [[src-tauri/src/lib.rs#get_context_savings_analytics]].

## Learning System

AI-powered behavioral pattern discovery that analyzes tool-use observations, git history, and recent session history to extract reusable rules.

### How It Works

Multi-stream LLM analysis in [[src-tauri/src/learning.rs]] combining tool-use observations, git commit patterns, and session insights. All inference goes through [[src-tauri/src/cc_client.rs]] (`claude` CLI headless mode); no direct Anthropic API calls.

**Stream A** extracts patterns from provider-scoped tool-use observations collected by Claude or Codex hooks. **Stream B** analyzes git commit patterns via [[src-tauri/src/git_analysis.rs]]. **Stream C** ([[src-tauri/src/learning.rs#analyze_sessions_stream]]) derives session-insight patterns from Quill's own locally indexed session history — cross-project, provider-scoped, recency-capped, top-level sessions only, secret-redacted before inference — through the same `cc_client` path as Stream A/B (no external `claude /insights` command). All three streams emit the same `StreamFindings` shape, so any one alone can yield rules. A synthesis step combines findings and applies LLM verdicts on existing rules. Uses Sonnet 4.6 for extraction and, since feature 005 (US5 T060, H-7), the same pinned Sonnet 4.6 for synthesis (no rolling `sonnet` alias — single-model pipeline with stable cost attribution). Per-call structured metadata (tokens, model, durations, cost, cache stats) is captured into `learning_runs.inference_metadata` for every stream including `stream_c`; feature 005 (US5 T058, H-6) decodes that JSON tolerantly into a derived `RunInferenceSummary` rollup (total cost/duration, highest-cost `primary_model`, per-phase `calls`) surfaced on each `LearningRun` for the run-history UI — legacy/micro runs with no metadata decode to `None`, never an error.

The `streams` `RunPhase` records its `status` from per-stream outcomes rather than from "the code reached the next line": if zero streams produced findings AND at least one stream's inference metadata record carries `success=false` (any `failure_kind`: spawn, timeout, schema, …), the phase is `failed` (rendered as the red ✗ in [[src/components/learning/RunHistory.tsx#phaseStatusDot]]); otherwise it is `completed` (✓), even when `findings_count == 0` (streams ran but extracted nothing). When the all-empty branch triggers, the top-level `learning_runs.error` column distinguishes "claude subprocesses failed" from "extracted nothing": it is set to `"All N streams failed (claude subprocess error: <comma-joined failure_kinds>). See run logs for stderr."` when every dispatched stream failed at the subprocess level, to a partial-failure variant when ≥1 stream failed AND ≥1 returned empty, or to the legacy `"No streams produced findings"` only when streams all ran cleanly with zero extracted patterns. The intent: the run-history banner names the actionable cause (e.g. a too-restrictive sandbox policy that SIGILLs the launcher) instead of collapsing to a misleading "empty" message.

### Confidence Scoring

Wilson lower-bound confidence scoring with a 90-day half-life freshness decay.

States: **emerging** (new, low confidence), **confirmed** (high confidence, validated), **stale** (no recent observations), **invalidated** (contradicted by evidence). Anti-patterns are flagged separately.

[[src-tauri/src/storage.rs#evidence_weighted_score]] is the single source of truth (both `get_learned_rules` read sites and the promotion gate route through it). Feature 005 US3 (C-3/M-4/FR-014/FR-017): `compute_state` adds a strong-contradiction override — `beta >= alpha AND beta >= 5.0` ⇒ `invalidated` regardless of Wilson confidence, ordered after the stale check and before the confidence bands. LLM verdicts on existing rules are not silently dropped: `support`/`contradict` adjust α/β by strength, an `irrelevant` verdict calls [[src-tauri/src/storage.rs#Storage#decay_rule_freshness]] (one 90-day half-life backward, clamped monotone-backward), and any unknown verdict string is logged rather than discarded. Operator accept/reject/bad feedback (FR-029) is the **primary** signal: [[src-tauri/src/storage.rs#Storage#submit_rule_feedback]] upserts `operator_feedback` and the scorer folds it into α/β with a weight (`W_op=50`, `bad`=`100`) that strictly dominates any single LLM verdict (≤1.0) or the raw self-rating — `accept`→large α, `reject`→large β (recoverable), `bad`→largest β plus a durable `rule_tombstones` row (`tombstoned_by='operator_bad'`). The raw extracting-model `confidence` no longer gates anything.

### Trigger Modes

Analysis can run **on-demand** (manual) or **periodic** (every N minutes). Configurable via `LearningSettings`.

### UI

The Learning window has two tabs: **Rules** and **Memories** (memory optimization), plus a provider filter for combined, Claude-only, or Codex-only views.

The Rules tab splits rules into two sections: **Active Rules** (have `.md` files on disk) and **Discovered** (DB-only candidates). Both rules and runs show provider-scope badges so shared Claude-plus-Codex rules are distinct from provider-specific ones. A `StatusStrip` shows scoped observation counts and a "Run Analysis" button. A floating `RunHistory` OS window shows past runs with per-phase timing, provider scope, and real-time logs during active runs; its toggle state follows manual closes and Strict Mode remounts.

### Rule Storage

Rules are tracked in the `learned_rules` database table with `provider_scope` provenance and optionally written as `.md` files to provider-specific learned-rule directories.

Each row carries a persisted `lifecycle` (`candidate → awaiting_review → active`, plus `rejected`/`suppressed`/`tombstoned`) that is distinct from the read-derived quality `state`. Analysis NEVER writes a `.md` (feature 005 US2 T025, FR-007): extraction only persists DB `candidate` rows. A global `.md` is authored exclusively by the human-gated approval path. Claude-only rules live under `~/.claude/rules/learned/`, Codex-only rules under `~/.config/quill/learned-rules/codex/`, and shared rules under `~/.config/quill/learned-rules/shared/`. Suppression is durable via a name-keyed `rule_tombstones` table consulted at every name-addressed write/activation path, so a deleted pattern cannot be silently resurrected by re-extraction or reconcile.

### Rule Watcher

[[src-tauri/src/rule_watcher.rs]] monitors learned-rule directories for real-time filesystem changes using the `notify` crate.

On Create/Remove/Modify events for `.md` files, a debounced (300ms) reconciliation pass diffs the DB against the filesystem via [[src-tauri/src/storage.rs#Storage#reconcile_learned_rules]]: new files are INSERTed as `lifecycle='candidate'` with frontmatter-parsed metadata (`source = 'manual'`) so they route into the review queue (never auto-active), a deleted file durably tombstones its row (`beta += 5.0`, `lifecycle='tombstoned'`, `rule_tombstones` row `tombstoned_by='reconcile_delete'`), and modified files have their `content`/`content_hash` updated. Steps 3a/3c skip names with an active tombstone or terminal lifecycle so suppression is never overridden. Emits `learning-updated` for instant UI refresh.

### Rule Promotion

Approval via [[src-tauri/src/storage.rs#Storage#promote_learned_rule]] is the SOLE writer of a global learned-rule `.md` (feature 005 US2 T029, FR-007/FR-008).

The route from `candidate` to `awaiting_review` is evidence-grounded (feature 005 US3, C-3/H-1/H-2/M-3, FR-014..018). Every extracted/synthesized candidate carries machine-checkable `evidence_refs` (`{kind,id}`: Stream A injects the real `observations.id`, Stream B a git `%h` short-hash or the snapshot HEAD key, Stream C the indexed `session_id`). Before persistence [[src-tauri/src/storage.rs#Storage#resolve_evidence_refs]] resolves them; a candidate with no resolvable evidence is rejected and not stored (logged, skipped). [[src-tauri/src/storage.rs#Storage#persist_citations_and_advance_version]] writes a retention-proof `rule_evidence_citations` snapshot and repurposes the dead `confirmed_projects` column as the JSON array of distinct project paths among resolved observation citations. The ordering is strictly: `store_learned_rule` (α/β + content merge — always applied; it no longer bumps the version) → the new version's `rule_evidence_citations` are persisted → and ONLY THEN is the pending marker `current_version` advanced, all in one transaction via [[src-tauri/src/storage.rs#Storage#persist_citations_and_advance_version]] (feature 006 Follow-up B, R-B/C-B). This makes the invariant **`current_version` always resolves to a version that has its evidence citations** true by construction: a citation-write failure rolls the whole step back so neither the new rows nor the bump persist and the rule stays review-eligible on its prior snapshot (no transient concurrent-reader window and no permanently un-reviewable human-pending rule; merge-always is preserved because the merge committed separately). After that, [[src-tauri/src/storage.rs#Storage#eligible_for_review]] — one indexed point-read, no N+1, now always reading a cited version — sets `lifecycle='awaiting_review'` iff `evidence_weighted_score ≥ learning.min_eligibility` (Wilson scale, default **0.6**, legacy `learning.min_confidence` read as a fallback) AND `resolved_distinct_refs ≥ 3` AND `distinct_sources ≥ 1` AND `state != invalidated` AND not tombstoned. Each rule's *own* resolved-citation count drives its α/β (fixes the `observation_count=0` bug for Stream B/C-only rules). [[src-tauri/src/storage.rs#Storage#record_rule_reconciliation]] then deterministically supersedes duplicates (`lifecycle='superseded'` + `superseded_by`, survivor = higher evidence-weighted score, tie-broken by observation count then name) and flags conflicts (`lifecycle='conflict_flagged'` on both), so neither is independently surfaced.

Promotion preconditions `lifecycle='awaiting_review'`, an inactive tombstone, and the counterfactual gate (else `Err`). The regression gate (feature 005 US4 T053, C-4/FR-020) runs BEFORE any `.md`/`active` write: [[src-tauri/src/storage.rs#Storage#latest_eval_verdict]] (newest `evaluation_results` row, `(rule_name, evaluated_at DESC)`) is consulted — if it `regression`s AND no [[src-tauri/src/storage.rs#Storage#has_reviewer_override]] row exists for that `(rule_name, replay_set_version)`, promotion is hard-blocked (`"blocked: rule regresses the replay set; record an explicit reviewer override to approve"`). `judge_uncalibrated`/`replay_set_stale`/`inconclusive` (and regressing-but-overridden) are NOT blocked — promotion proceeds and the caller/UI surfaces the caution (warn-not-block). No `evaluation_results` rows at all is NOT a hard block (the rule is "unevaluated"; SC-007 expects the maintainer to run eval but promote must keep working). It then runs all DB mutations in ONE transaction — sets `file_path` and `lifecycle='active'`, appends an immutable [[src-tauri/src/storage.rs#Storage#rollback_rule|`rule_versions`]] row (`change_kind='promote'`), records provenance (`origin_run_id/origin_model/origin_at`) plus a retention-proof `rule_evidence_citations` snapshot — and commits FIRST; only after the commit does it materialize the redacted + injection-sanitized `.md` in the scope dir (path-traversal-guarded) via a temp-file + atomic `rename`, so a crash never leaves a torn or provenance-less orphan file (a post-commit write failure returns `Err` and the dangling DB-active row is self-healed by reconcile step 3b). Re-derivation of a queued rule UPSERTs content (α/β merged in place, never a 2nd row, never overwriting an active `.md`) and the pending `current_version` is bumped only after the new version's `rule_evidence_citations` are persisted, atomically (feature 006 Follow-up B), so a queued rule is never silently stranded with a citation-less `current_version`. [[src-tauri/src/storage.rs#Storage#rollback_rule]] restores an earlier version as a forward append-only `rule_versions` row (rewriting the `.md`, hash-touched so the watcher does not re-version it); [[src-tauri/src/storage.rs#Storage#reactivate_rule]] is the only path that clears a tombstone, returning the rule to `candidate`. A one-time sentinel-guarded legacy archive-then-wipe in the [[src-tauri/src/storage.rs#Storage#init]] chain copies any pre-existing on-disk rules to a read-only manifested archive, deletes the live files, and tombstones their rows before the watcher starts.

### Counterfactual Evaluation Harness

[[src-tauri/src/eval_harness.rs]] scores a candidate rule against a frozen, in-repo replay set (feature 005 US4, C-4/FR-019..023) so promotion can be gated on an evidence-based regression signal rather than the raw self-rating.

The replay set lives at `src-tauri/tests/fixtures/replay_set/` — a finalized `manifest.json` (`replay_set_version`, pinned `baseline_assistant_model`, `frozen_at`, `staleness_days`) plus ≥12 pre-redacted synthetic `case_NNN_<slug>.json` files with maintainer-authored `expected_judgment` labels spanning the R-4 archetypes (clear-positive, clear-regressing/negative-transfer, hallucinated-evidence, one-off/low-evidence, conflicting-pair, suppressed-then-rederived, empty/no-signal, plus normal friction). [[src-tauri/src/eval_harness.rs#load_replay_set]] parses the manifest and every case in order. For each case [[src-tauri/src/eval_harness.rs#run_evaluation_with_set]] substitutes the rule under test, then runs `N=3` paired WITH/WITHOUT [[src-tauri/src/cc_client.rs#invoke_typed]] arms pinned to `Model::Sonnet46`, taking the per-arm median and flagging high per-arm dispersion as `inconclusive`; `N=3` judge calls then emit a typed [[src-tauri/src/eval_harness.rs#EvalVerdict]] (`with_quality`/`without_quality`/`delta`/`regression`/`negative_transfer`/`rationale`), aggregated by strict majority. [[src-tauri/src/eval_harness.rs#is_regression]] is the deterministic dead-band rule: `delta < -EPSILON` (0.05) OR negative-transfer. [[src-tauri/src/eval_harness.rs#cohen_kappa]] scores the judge's coarse labels against the frozen labels; κ below `KAPPA_FLOOR` (0.6) sets `judge_uncalibrated` (verdicts advisory, non-blocking). [[src-tauri/src/eval_harness.rs#compute_staleness]] flags the set stale iff the judge model differs from the pinned baseline OR `age(frozen_at) > staleness_days` (disclosed, never auto-blocking). [[src-tauri/src/eval_harness.rs#run_evaluation]] returns a rich [[src-tauri/src/eval_harness.rs#EvalOutcome]] whose [[src-tauri/src/eval_harness.rs#EvalOutcome#to_row]] yields the scalar [[src-tauri/src/eval_harness.rs#EvalVerdictRow]]. The harness itself performs no storage I/O; the `run_rule_evaluation` IPC drives it and [[src-tauri/src/storage.rs#Storage#persist_evaluation_result]] (feature 005 US4 T052, FR-022) appends one `evaluation_results` row per evaluation (newest wins on the `(rule_name, evaluated_at DESC)` read — idempotent-friendly, no UPSERT; the scalar row is also stored self-describing in `per_case_json`). The promotion-coupling gate ([[src-tauri/src/storage.rs#Storage#latest_eval_verdict]] / [[src-tauri/src/storage.rs#Storage#has_reviewer_override]] / [[src-tauri/src/storage.rs#Storage#record_reviewer_override]]) is consumed by [[src-tauri/src/storage.rs#Storage#promote_learned_rule]] (see Rule Promotion). Tests use the `#[cfg(test)]` `cc_client` inference double (scripted JSON, no live `claude`).

## Session Search

Full-text search across Claude Code and Codex session transcripts, powered by Tantivy in [[src-tauri/src/sessions.rs]].

### Indexing

Opening Session Search triggers an mtime-based transcript sync, and hook endpoints can also ingest updates. Indexed messages include code_changes, commands_run, tool_details, and files_modified metadata.

### Search Interface

Search bar with filters for project, host, role, date range, and git branch.

Results show ranked hits with snippets, tools used, files modified, and code changes. A detail panel shows surrounding context (plus/minus 5 messages). Faceted search provides pre-aggregated project and host counts. Pagination with 20 results per page and load-more.

### Batch Code Stats

`useSessionCodeStats` hook lazily fetches LOC stats for visible search results using a ref-based cache to avoid redundant IPC calls.

## Working Context Preservation

Quill preserves large transient context as searchable refs so assistants can keep the conversation compact while still recovering details.

### Feature Toggle

Context preservation is controlled by a global default-off setting in Quill.

The [[features#Settings Window]] exposes a `Context` tab backed by `context_preservation.enabled` in the settings table. Enabling installs the local context scripts, context MCP tool, context-aware instruction templates, and hooks for currently enabled Claude Code and Codex providers; future Claude or Codex provider enables inherit the setting. Disabling redeploys only the base Quill integration for those providers, removing context hooks and local context assets while preserving historical context stores and analytics rows. Toggle sync runs when an enabled provider home exists, even if the provider CLI is temporarily unavailable, so disable cleanup can still remove local feature assets.

### Context MCP Tools

The Quill MCP server exposes context tools beside the single `search_history` session-history MCP tool.

Tools in [[src-tauri/claude-integration/mcp/tools/context.py]] can index text or files, fetch and cache web pages, run bounded commands, search indexed chunks, retrieve focused sources, record continuity events, create compact snapshots, inspect stats, and purge stored context. File-based tools resolve paths under the selected working directory before reading or preserving content.

The session-history surface in [[src-tauri/claude-integration/mcp/tools/search.py]] is intentionally narrow: only `search_history` remains, after a 30-day usage audit showed the discovery, analytics, and drill-down tools (`list_projects`, `list_sessions`, `get_session_overview`, `get_session_context`, `get_file_history`, `get_branch_activity`, `find_related_sessions`, `get_token_usage`, `get_learned_rules`, `get_tool_details`, `get_index_status`) were called ≤20 times across all sessions tracked. Trimming the surface keeps the tool listing legible and reduces low-value tool-selection noise.

Large execution and batch outputs are stored as `source:N` and `chunk:N` refs. Responses return previews and snippets by default, while [[src-tauri/claude-integration/mcp/tools/context.py#quill_get_context_source]] retrieves bounded chunks when the model needs exact details.

### Routing Hooks

Provider hooks steer high-volume operations toward Quill context tools before they flood the active transcript and track fetched-to-disk paths so the fetch-then-read bypass is closed.

`src-tauri/claude-integration/scripts/context-router.cjs` and `src-tauri/codex-integration/scripts/context-router.cjs` are byte-identical copies that block raw WebFetch or noisy `curl`/`wget` dumps, nudge broad Bash, Read, Grep, build, and test output toward `quill_*` MCP tools, and surface `mcp__quill__quill_execute` as the right alternative for `curl ... | jq` workflows (the previous deny message implicitly invited the workaround by suggesting `curl -sS -o path` as the only mitigation).

When the router denies a raw `curl`/`wget`, it parses the command line via the `extractFetchUrls` helper in `src-tauri/claude-integration/scripts/context-router.cjs` and embeds the first 1–2 distinct URLs into a ready-to-paste tool call inside the deny reason: `mcp__quill__quill_execute(command="curl -sS <URL> | jq .")` for API-shaped URLs (`/api/`, `.json`, `format=json`, or `api.` host) and `mcp__quill__quill_fetch_and_index(url=<URL>)` for HTML/docs/pages. The detection deliberately reads the heredoc-stripped command without stripping quoted args so `curl 'https://…'`, `fetch("https://…")`, and `requests.get("https://…")` all surface; the extractor trims at the first embedded quote, balances trailing `)`, and strips control whitespace (`\r`/`\n`/`\t`) so an attacker-authored URL with a literal newline cannot inject a fake instruction line into the prose deny message. The `looksLikeApiJson` heuristic bails out on binary-artifact extensions (`.tar.gz`, `.zip`, `.pdf`, images, fonts, media, `.wasm`, `.exe`, `.whl`, etc.) before the `api.` host check so binary downloads on `api.*` hosts route to `quill_fetch_and_index` instead of a `jq` pipeline that would mangle them. The previous generic deny gave the model the right tool names but not a copy-paste replacement; a 30-day audit showed only 0.7% of denied sessions ever followed up with a `quill_*` MCP call, so the actionable deny is the smallest change that closes that gap without adding new HTTP infrastructure.

When `curl`/`wget` does pass the network-dump check by writing quietly to a file (e.g. `curl -sS -o /tmp/x.json URL`), the router records the destination path under `~/.config/quill/context/markers/<provider>-<session>/tainted.json` and denies any subsequent Read, or Bash invocation of a pure-reader (`cat`, `bat`, `head`, `tail`, `less`, `more`, `view`, `od`, `xxd`, `strings`, `hexdump`, `sed`, `awk`, `grep`, `rg`, `ack`, `jq`, `yq`, `xq`, `xmllint`), targeting that path. Interpreter execution (`bash /tmp/x.sh`, `python /tmp/x.py`) and removal (`rm /tmp/x.json`) remain allowed so fetch-and-install flows are unaffected. The taint set is capped at 256 paths per session.

Per-session marker files under `~/.config/quill/context/markers/` keep guidance from repeating, and the scripts prune marker directories older than 30 days at most once per day. The taint state file lives inside each session's marker directory and is removed by the same 30-day cleanup. A standalone Node test suite next to the router (`src-tauri/claude-integration/scripts/context-router.test.cjs`) exercises the deny paths and the taint round-trip; run it with `node context-router.test.cjs` from its directory.

### Continuity Capture

Continuity hooks record small task and decision hints without writing to provider memory paths.

`src-tauri/claude-integration/scripts/context-capture.cjs` and `src-tauri/codex-integration/scripts/context-capture.cjs` write compact JSONL events under `~/.config/quill/context/continuity/`, capture prompts and simple decision/task hints, and store PreCompact or Stop snapshots when available. SessionStart guidance is scoped by provider and project key, where the project key is the nearest git root for the current `cwd` or the normalized `cwd` when no git root is found, so recent work from another project cannot leak into a new session. Continuity JSONL and per-session files are pruned to a 30-day retention window at most once per day.

The SessionStart `<quill_continuity>` directive only injects when at least one of `last_prompt`, `task_hints`, or `decision_hints` is non-empty for the scoped records (the `buildDirective` helper in `src-tauri/claude-integration/scripts/context-capture.cjs` returns `null` otherwise). Earlier the directive always rendered as long as any record existed, which produced empty injects (just `cwd:` + tool list + reminder line) that crowded the system prompt without carrying any actual continuity content.

### Context Savings Telemetry

Context savings telemetry forwards compact measurements to Quill without copying large context into the main analytics database.

The MCP tools and context hooks send best-effort batches to `/api/v1/context-savings/events` through `context-telemetry.cjs` or the Python telemetry helper in [[src-tauri/claude-integration/mcp/tools/context.py]]. Events record exact bytes when available, refs such as `source:N` or `snapshot:N`, and approximate token estimates using `ceil(bytes / 4)`. The local MCP context database remains the source of large stored content. The `feature.context_telemetry.enabled` flag (see [[features#Settings Window#Integration Features]]) gates whether `context-telemetry.cjs` is deployed at all; the router and capture scripts try to load it and fail open when it is absent so context preservation keeps working without any telemetry side effects.

Each event carries an explicit `category` (`preservation`, `retrieval`, `routing`, or `telemetry`) set at the call site by the producer. Token estimates are only auto-defaulted from byte counts for `preservation` and `retrieval` events; `routing` and `telemetry` events default `tokensSavedEst` and `tokensPreservedEst` to 0, and Rust ingestion normalizes those two fields back to 0 for any non-preservation/retrieval category so stale producers cannot inflate savings. The Rust ingestion layer derives `category` from `(eventType, decision)` only as a safety net for legacy callers via [[src-tauri/src/context_category.rs#derive_category]] and rejects unknown category strings outside the closed taxonomy. The Python `_attach_context_savings` wrapper in [[src-tauri/claude-integration/mcp/tools/context.py]] also gates its post-response `tokensSavedEst` recomputation loop on `category in ('preservation', 'retrieval')` so routing tools like `quill_search_context` never accumulate phantom savings from JSON response sizing.

## Brevity Profile

Single global toggle that injects a managed instruction block into every enabled Claude/Codex provider's agent file to compress assistant prose without altering code, paths, URLs, or other structural content.

### Feature Toggle

Brevity is one of the [[features#Settings Window#Integration Features]] flags (`feature.brevity.enabled`) surfaced inside the [[features#Settings Window]]'s Context tab.

[[src-tauri/src/integrations/manager.rs#set_brevity_enabled]] persists the flag and routes through `set_feature_flag`, which calls `apply_features_to_enabled_providers` to reinstall every enabled Claude/Codex provider and then runs `sync_brevity_blocks` to write or strip a `<!-- quill-managed:brevity:start --> ... <!-- quill-managed:brevity:end -->` block in each provider's primary agent file (`~/.claude/CLAUDE.md` for Claude Code, `~/.codex/AGENTS.md` for Codex). The block describes the caveman compression style and lists what the assistant must preserve verbatim: code blocks, inline code, URLs, file paths, command names, library and proper-noun names, numbers, env vars, and markdown structure. Disabling strips just the managed block while leaving the rest of the file intact. Newly-enabled providers inherit the current global setting through `confirm_enable_with_key`, which calls the same sync helper after install; disabling a provider strips that provider's block via `confirm_disable`.

### Migration

Existing installs that used per-provider brevity keys are migrated to the new global flag on first read of `IntegrationFeatures`.

[[src-tauri/src/integrations/manager.rs#load_integration_features]] calls `read_brevity_setting`, which unions the two legacy values (`provider.claude.brevity_enabled`, `provider.codex.brevity_enabled`) — if either was `true`, the new global flag is initialized `true` so the user does not silently lose the setting — then deletes the legacy keys.

### Symlink Awareness

The writer canonicalizes the target path before each write so a single underlying file is never edited twice.

When `AGENTS.md` is a symlink to `CLAUDE.md`, [[src-tauri/src/brevity.rs#apply_block]] takes the list of providers that should keep the block and uses canonical-path comparison so stripping one provider's block does not clobber a shared canonical file another still-enabled provider wants. MiniMax does not have a managed agent file; `apply_block` rejects it with an error before any disk write.

## Plugin Manager

Plugin installation, marketplace management, and update tracking via [[src-tauri/src/plugins.rs]].

### Plugin Lifecycle

Plugins are enumerated per enabled provider and normalized into one shared UI model with provider badges.

Claude plugins come from `~/.claude/plugins/installed_plugins.json` with blocklist-based enable/disable and CLI-backed install/remove/update actions. Codex plugins come from `codex app-server` plugin APIs and expose install/remove plus provider-native enabled state, but not separate enable/disable or versioned update actions.

### Marketplace System

Claude marketplaces are git repositories registered in `~/.claude/plugins/known_marketplaces.json`. Each marketplace exposes a manifest of available plugins, refresh pulls latest via git, and users can add or remove custom marketplace repos.

Codex marketplaces are discovered from `codex app-server` catalog responses. Quill can refresh the Codex catalog, but add/remove marketplace actions stay Claude-only because Codex does not expose that mutation surface.

### Update Checking

Claude plugin updates are polled every 4 hours.

Lenient semver comparison detects available updates, and bulk update with per-plugin progress events (`plugin-bulk-progress`) remains Claude-only because Codex does not expose versioned update metadata.

### Plugin UI

The plugin window stays shared but behaves per provider.

It switches between enabled providers and keeps one tab set: **Installed**, **Browse**, **Marketplaces**, and **Updates**. The Installed tab hides enable/disable controls for Codex, the Marketplaces tab disables add/remove for Codex, and the Updates tab shows Codex-specific guidance when only refresh is available. Operation result banners auto-dismiss after 5 seconds.

## Memory Optimizer

LLM-driven optimization of provider-aware memory and instruction files via [[src-tauri/src/memory_optimizer.rs]] (1,670 lines).

### Scanning

Recursively scans project directories for Quill memory files plus provider instruction files such as `CLAUDE.md` and `AGENTS.md`.

Filters out denylisted patterns, minified code, and compiled files. Dynamic budget allocation changes based on whether memory files and instruction files are both present.

### Analysis

Assembles an LLM prompt with memory content, provider-scoped instruction files, learned rules, and instinct sections.

Calls Sonnet 4.6 to generate provider-scoped optimization suggestions. Suggestion types: **Delete** (remove redundant), **Update** (improve content), **Merge** (combine related files), **Create** (add missing), **Flag** (needs human review).

### Suggestion Lifecycle

Suggestions follow a status flow: pending -> approved/denied, with backup for undo. Group operations allow batch approve/deny.

Approved suggestions are executed (file written/deleted/merged), with original content backed up. Denied suggestions can be un-denied. Executed suggestions can be undone (restores from backup). Malformed LLM output is filtered before storage so the UI only surfaces actionable suggestions, and `MEMORY.md` is treated as a special index file that can be updated directly but not merged as a source.

### UI

The Memories tab in the Learning window shows a project selector, provider filter, instruction and memory file browser with content preview, and suggestion cards with actions.

Supports custom project management, bulk operations, provider badges on files and suggestions, and approve/deny/undo per suggestion. The project selector opens on `All Projects` so the first view is the aggregated memory browser. The manage panel bulk delete acts on the current Memories selection, including aggregated deletion across `All Projects`, while still leaving instruction files untouched. Background learning refreshes update in place so the current project selection and expanded memory view do not snap back to the default project during polling. Bulk `Optimize All` runs keep the panel in a stable in-place state instead of flashing the all-projects browser as individual runs finish.

### Prose Compression

Optional caveman-compress pre-pass run from the Memories panel before the regular optimizer.

[[src-tauri/src/memory_optimizer.rs#run_prose_compression]] drives the orchestrator in [[src-tauri/src/compress_prose.rs]], which rewrites every eligible memory file via Sonnet 4.6, validates the rewrite preserves headings, code blocks, URLs, file paths, and bullets, retries up to twice on validation or LLM error, and either commits the rewrite (leaving a `<file>.original.md` backup next to the compressed file) or restores the original. Skip rules in `compress_prose/detect.rs` exclude instruction files, files over 500 KB, files on the secrets denylist (paths under `.ssh`/`.aws`/`.gnupg`/`.kube`/`.docker`, basenames such as `.netrc`/`authorized_keys`/`known_hosts`, basenames containing `secret`/`credential`/`apikey`/`privatekey`, and `.env*` prefixes), files with non-prose extensions (code, config, markup, lock files), and files that already have an `.original.md` backup so a second pass is a no-op. The `trigger_memory_optimization` Tauri command takes an optional `compress_prose: bool` flag plumbed from the Memories panel checkbox, and progress streams over the existing `memory-optimizer-log` event.

## Restart Orchestrator

Graceful restart of running Claude and Codex sessions via [[src-tauri/src/restart.rs]].

### Instance Discovery

Uses provider-specific discovery with a shared row model.

Claude instances come from Quill state files in `~/.cache/quill/claude-state/` plus process scanning. Codex instances come from process scanning and `~/.codex/sessions/` metadata queues per cwd so multiple same-directory sessions can still map to distinct restart rows.

### Restart Flow

Four-phase orchestration with real-time status events at each phase transition.

(1) Discover instances, (2) wait for idle where supported, (3) send SIGTERM and wait for exit, (4) resume with provider-specific commands. Claude uses `claude --resume`; Codex uses `codex resume`. Each phase emits `restart-status-changed` events.

Codex does not expose a reliable idle signal, so its rows stay `Unknown` before restart and Quill proceeds directly to termination/resume instead of pretending it observed an idle transition.

### Instance Status

Tracked as: Idle, Processing, Unknown, Restarting, Exited, or RestartFailed. The UI shows status indicators per instance with cancel support.

Force restart skips the idle-wait phase.

### Hook Installation

Restart hook actions are provider-aware.

Claude install writes Quill hook scripts into `~/.claude/settings.json` plus shell integration. Codex restart setup currently installs shell integration only, while Codex integration installs only telemetry/session hooks; the `qbuild-guard.sh` edit guard remains Claude-only because Codex hook coverage does not intercept `apply patch` edits. The shared shell integration is only removed when the last restart-capable provider is disabled, and the restart window groups instances by provider with setup banners per provider when integration is missing.

## Settings Window

Standalone Tauri window opened by the titlebar settings button (sliders icon) that exposes every user-configurable feature toggle in one comprehensive surface, replacing the previous inline `ProviderMenu` popover.

### Window Routing

Registered as `?view=settings` in [[src/main.tsx]] and listed in `src-tauri/capabilities/default.json`.

The window is intentionally NOT provider-gated so users can manage integrations and runtime preferences before any provider is enabled. The shell lives in [[src/windows/SettingsWindowView.tsx]] and follows the same custom-titlebar pattern as the Sessions and Plugins windows (transparent, decorations off, default and min 540x620 with min height 480). The default width matches the min width so the five top tabs always fit on a single row on first launch with a small buffer past the last tab, and the `.settings-tabs` flex container uses `nowrap` so tabs never collapse onto a second row even if the user pushes the window narrower.

### Tab Layout

Top-tabs navigation hosts five panels: General, Integrations, Context, Learning, and Performance.

| Tab | Panel | Settings |
|-----|-------|----------|
| General | [[src/components/settings/GeneralTab.tsx]] | Layout (stacked / side-by-side), time visualization mode, Live and Analytics panel visibility, always-on-top toggle, an Advanced section with the current-config summary and "Reset to defaults" button covering runtime, learning, and UI prefs, plus a bottom "Help improve Quill" toggle that drives the [[features#Crash Reporting]] opt-out |
| Integrations | [[src/components/settings/IntegrationsTab.tsx]] | Status provider selector, Rescan PATH, Activity tracking master toggle, per-provider enable/disable confirmations (with MiniMax API key prompt), in-place MiniMax API-key edit form |
| Context | [[src/components/settings/ContextTab.tsx]] | Working Context Preservation global toggle, Context savings telemetry sub-toggle (gated on context preservation), and the [[features#Brevity Profile]] global toggle (gated on having any provider enabled), each with descriptive copy explaining what gets installed |
| Learning | [[src/components/settings/LearningTab.tsx]] | Learning trigger mode, periodic enable, periodic interval, min observations, min confidence, plus the Rule Watcher master toggle |
| Performance | [[src/components/settings/PerformanceTab.tsx]] | Live-usage refresh enable + interval (60–600s), plugin update checker enable + interval (1–24h) |

### Integration Features

Four global feature flags decide which optional Quill assets get deployed into Claude Code and Codex when those providers are enabled, modeled by the [[src-tauri/src/models.rs#IntegrationFeatures]] struct.

`context_preservation` (default off), `activity_tracking` (default on), `context_telemetry` (default on, gated on `context_preservation`), and `brevity` (default off) are each persisted as `feature.<name>.enabled` keys in the SQLite settings table. The Settings window writes them via `set_context_preservation_enabled`, `set_activity_tracking_enabled`, `set_context_telemetry_enabled`, and `set_brevity_enabled` IPC commands; each setter saves the key, calls [[src-tauri/src/integrations/manager.rs#apply_features_to_enabled_providers]] to reinstall every currently-enabled provider with the merged feature set (and re-sync brevity blocks via `sync_brevity_blocks`), and emits `integration-features-updated` with the full struct so any open Settings window observes the resolved values without a re-fetch. Newly-enabled providers inherit the current feature set automatically — `confirm_enable_with_key` reads `IntegrationFeatures` from storage and threads it to the installer plus the brevity sync. Activity tracking gates the `observe.cjs` PreToolUse / PostToolUse hooks; context telemetry gates the `context-telemetry.cjs` script; context preservation gates the full context asset bundle (router, capture, MCP tool, full instruction template); brevity gates the `<!-- quill-managed:brevity -->` block in each enabled provider's primary agent file.

### Cross-Window UI Sync

UI preferences stored in `localStorage` (layout mode, time mode, Live/Analytics panel visibility) are shared across Tauri webviews but require an explicit notify so other windows update without reloading.

The [[src/hooks/useUiPrefs.ts#useUiPrefs]] hook writes localStorage and emits a `ui-prefs-updated` Tauri event. The main window's [[src/App.tsx]] subscribes to this event and re-applies layout, time mode, and panel visibility without a reload. The same event drives the "Reset to defaults" button in the Advanced section at the bottom of the General tab.

### Runtime Settings IPC

Always-on background tasks expose enable/interval toggles through a single `RuntimeSettings` IPC pair.

[[src-tauri/src/lib.rs#get_runtime_settings]] and [[src-tauri/src/lib.rs#set_runtime_settings]] persist `live_usage.enabled`, `live_usage.interval_seconds`, `plugin_updates.enabled`, `plugin_updates.interval_hours`, `rule_watcher.enabled`, `always_on_top`, and `crash_reporting.enabled` in the SQLite settings table. Live values are read on every iteration of the live-usage loop and the plugin-update checker so changes take effect on the next tick. The rule watcher reads its flag once at startup since `notify` holds an OS handle. Changing `always_on_top` calls `WebviewWindow::set_always_on_top` on the main window; toggling `crash_reporting.enabled` calls [[src-tauri/src/crash_reporting.rs#set_enabled]] which (re)initializes or drops the Sentry `ClientInitGuard` immediately; other runtime saves do not touch the main window's topmost/focus state. After every save the backend emits `runtime-settings-updated` so [[src/hooks/useRuntimeSettings.ts#useRuntimeSettings]] keeps any open Settings windows in sync.

### MiniMax API Key Update

The Integrations tab can update a stored MiniMax API key without disabling and re-enabling the integration.

[[src-tauri/src/lib.rs#set_minimax_api_key]] delegates to [[src-tauri/src/integrations/manager.rs#set_minimax_api_key]] which trims the key, persists it via [[src-tauri/src/integrations/minimax.rs#save_api_key]], refreshes provider statuses, and emits `integrations-updated`. The frontend renders an inline `Save` / `Cancel` form; the dialog-based first-enable flow stays unchanged.

## Crash Reporting

Default-on, user-opt-out crash reporter that ships scrubbed stack traces to Sentry without exposing any session content. Toggled via the "Help improve Quill" row at the bottom of the General settings tab.

### Deny-by-Default Scrubbing

Both surfaces wire a `before_send` hook that strips every dynamic field — messages, exception values, breadcrumbs, request data, user context, extras, and filenames are reduced to basenames — before any event leaves the process.

The threat model assumes the entire payload domain is sensitive: panic messages can contain prompts serialized across the Tauri IPC boundary, exception text can interpolate user data, and absolute file paths typically reveal the developer's `$HOME`. Rather than denylist known PII fields, both sides keep only stack-frame structure (function, module, line number) and an allowlist of tags (`release`, `environment`, `runtime`). Frontend session replay, browser-tracing, autoSessionTracking, default integrations, and HTTP context capture are all explicitly disabled — the only Sentry features in use are the global error handler and React error boundaries via `reactErrorHandler()`. Rust mirrors the policy with `auto_session_tracking: false`, `max_breadcrumbs: 0`, and a `before_breadcrumb` that drops every breadcrumb. Sentry server-side data scrubbing rules (IP, geolocation, user-agent) remain a follow-up configurable in the project's Sentry settings, not in code.

### Dual-Surface Wiring

Frontend [[src/lib/crashReporting.ts]] and Rust [[src-tauri/src/crash_reporting.rs]] share the same DSN and scrubbing policy.

The Rust side stores its `ClientInitGuard` in a `OnceLock<Mutex<Option<ClientInitGuard>>>` so [[src-tauri/src/crash_reporting.rs#set_enabled]] can drop the guard on opt-out (which flushes pending events and closes the transport) and re-init on opt-in. The frontend calls `Sentry.close()` and `Sentry.init()` for the same effect; one-shot initialization is gated on the `crash_reporting.enabled` value returned by the very first `get_runtime_settings` IPC call from [[src/main.tsx]], so the SDK never sends data before the user's preference is read.

### Toggle Lifecycle

Toggling the "Help improve Quill" row in [[src/components/settings/GeneralTab.tsx]] writes through the standard [[features#Settings Window#Runtime Settings IPC]] pipeline and applies immediately on both surfaces.

[[src-tauri/src/lib.rs#set_runtime_settings]] detects a `crash_reporting_enabled` delta and calls [[src-tauri/src/crash_reporting.rs#set_enabled]] directly on the Rust side, then emits `runtime-settings-updated` carrying the resolved `RuntimeSettings`. The frontend `crashReporting` module listens for that event and calls [[src/lib/crashReporting.ts#setCrashReportingEnabled]] so the React-side SDK opens or closes its transport in lock-step. Default is on; the user-facing copy never mentions Sentry and instead emphasises that session data is removed locally before transmission.
