# Features

Quill provides live usage monitoring, analytics, behavioral learning, session search, working-context preservation, memory optimization, and restart orchestration.

## Live Usage View

Live provider-aware rate-limit pressure, rendered as the widget's LIMITS band above everything else.

Each provider gets one identity row whose visible window cells evenly divide the fluid meter region and reflow at their legible minimum. Direct rows show their nearest reset; CPA rows place each visible canonical reset inside its matching meter cell. A CPA pool replaces that provider's direct row while present; without one, the direct row remains. Severity follows the 50/80 thresholds. Component detail lives in [[frontend#Frontend#Components#Widget Limits Band]].

When a bucket's `resets_at` has passed, the cell renders as stale — muted percentage and a neutral slate bar — so bygone utilization never reads as a live severity. Direct rows exclude it from their nearest reset; a dated CPA window keeps a neutral `now` reset while a missing reset renders a dash.

Without CPA configured, Claude rows come from the Anthropic OAuth usage API, Codex rows come from `codex app-server` `account/rateLimits/read` (with transcript `token_count` data as a compatibility fallback), and MiniMax rows come from the MiniMax coding plan API at `api.minimax.io`. A configured CPA connection becomes the exclusive live usage source, suppressing every native provider request until disconnected. MiniMax is a service-only provider that requires an API key (stored in SQLite) rather than a local CLI; its rows keep the plan-level bucket filter (M\*, coding-plan-search, coding-plan-vlm) because the per-model long tail does not fit a 360px row. The backend can keep serving cached rows for a provider whose live poll fails so other providers do not blank the band. Its in-memory usage cache is keyed only by provider identity and enabled state so transient detection churn does not dislodge a fresh snapshot. Claude reuses recently persisted snapshots across window reopens and app restarts, and a 429 response arms a short-lived rate-limit cooldown so Quill serves the last snapshot without retrying the live endpoint on every remount.

The right end of the widget's LIMITS header carries the manual [[src-tauri/src/lib.rs#refresh_usage_data]] freshness control. It bypasses the process and Claude persisted-snapshot freshness guards so an operator can request current live data, while preserving rate-limit and network cooldowns that protect providers from repeated failed calls.

Because a cached snapshot can go stale while the cooldown holds, degraded reads surface on the LIMITS-header sync control rather than on the rows: "Showing cached data" from the very first 429, "Paused" for a 401 stale token (never a login prompt), and a single consolidated "Offline — showing cached data" for transport failures. All three are slate variants — a degraded read is not an alarm — and offline wins if more than one applies. Claude login guidance ("Run: claude /login") is only surfaced for a confirmed logout — no local credentials AND `claude auth status` reporting `loggedIn: false` — see [[data-flow#Usage Bucket Fetching]].

Transport failures (DNS, connect refused, pre-response timeout) on either Claude or MiniMax write a per-provider network cooldown computed by [[src-tauri/src/lib.rs#compute_network_backoff]] — half-jitter exponential, 60-second base, 30-minute cap, doubled per consecutive failure. While the cooldown is active, the backend short-circuits both polling paths — the frontend `setInterval`-triggered IPC and the backend `tokio` loop — and serves cached rows without making a live HTTP request. (The frontend's `setInterval` keeps firing every 3 minutes regardless; it does not know about the cooldown, but the IPC roundtrip is cheap and returns immediately from cache.) On any successful fetch the cooldown timestamps and consecutive-failure counter clear. The fast offline signal itself comes from the shared [[src-tauri/src/config.rs#http_client]], which sets a 5-second connect timeout and a 15-second overall timeout so reqwest never hangs indefinitely on a dead network.

A provider with no live buckets still gets a row stating why: `SETUP` in amber when the failure is actionable (a `config`/`auth` provider error or an unfinished install) and `UNAVAILABLE` in slate otherwise. Codex reset countdowns are derived from the direct app-server rate-limit response, which also exposes model-specific windows such as Codex Spark; a finite Codex credit balance rides the same row.

### CPA Pool Aggregation

CPA pool rows derive mean account pressure from routing-usable account snapshots and are never persisted as independent facts.

[[src-tauri/src/cpa/aggregate.rs#compute_cpa_pools]] groups Claude and Codex accounts separately. Healthy means CPA's documented `active` or compatible `ready` status without disabled or unavailable flags; every account remains in the total denominator. Each window is normally the arithmetic mean of healthy accounts returning that window, so missing buckets are excluded rather than read as zero. When no account is routing-usable, quota-readable snapshots provide a fallback mean so the provider row retains quota data.

The widget renders each aggregate as that provider's sole top-level row while the pool exists: fixed provider identity, inline healthy/total count, mean window cells, and reset readouts for visible canonical windows. Each reset uses only its matching aggregate window's earliest contributing timestamp; missing timestamps show a dash and elapsed timestamps show neutral `now`. A semantic disclosure reveals at most six account rows plus a remainder count.

Claude always projects its canonical 5-hour and 7-day schema so missing data remains explicit. Fable and other `weekly_scoped` model windows join that shared schema when any pool or account returns them, keeping the aggregate and disclosed account cells aligned. Codex projects only durations returned by its pool or accounts; an absent 300-minute window therefore removes the 5-hour cell and reset from the pool and every disclosed account row.

#### Usable lifecycle compatibility

CPA v7 reports usable credentials as `active`; Quill also accepts the compatible `ready` form, while every other lifecycle state and either disabled or unavailable flag remains unhealthy.

#### Usable account mean

Each normalized window averages returned utilization across active or ready, non-disabled, available accounts that contain it; routing-unusable accounts cannot influence quota math.

Its reset is the earliest parseable contributing reset because that is when the displayed mean can first change.

#### Health denominator with unusable exclusions

Every account counts toward total, but while any account is active or ready, only active or ready, non-disabled, available accounts count as healthy or contribute buckets. A cooling sibling therefore cannot dilute active utilization.

#### All-cooling fallback

When every account is routing-unusable, all quota-readable snapshots with returned windows contribute to the provider mean while the healthy count remains zero.

#### Missing account buckets stay gaps

A healthy account without fetched windows remains in the health count without inventing utilization.

#### All healthy buckets missing

A provider whose healthy accounts all lack windows yields no numeric aggregate bucket.

#### Empty pool

An empty account inventory yields no provider aggregate rows.

#### Runtime-only accounts included

Runtime-only accounts participate in health counts and aggregate math exactly like file-backed accounts.

### CPA Poll Scheduling

CPA window polling is smoke-gated, capped, staggered, and exclusive of native provider usage polling while configured.

[[src-tauri/src/cpa/poll.rs#poll_account_snapshots]] maps the complete auth-file inventory before scheduling windows. Only persisted `true` provider smoke verdicts permit calls; the first 16 non-disabled, available accounts by `auth_index` launch 250ms apart with at most three requests active.

Each configured CPA phase logs `cpa_phase_ms` so its real-pool duration can be checked against the 3-minute cadence. Disconnecting CPA restores the enabled native provider polling path.

#### Native source exclusivity

A configured CPA connection yields no native polling candidates; without CPA, every enabled provider remains eligible for direct usage polling.

#### Quota readability scheduling

Quota scheduling depends on credential readability, not CPA routing health.

The poll mapper canonicalizes `active` and compatible `ready` to the frontend's ready state, but schedules every non-disabled, available Claude or Codex account when its provider smoke gate is open. Successful buckets render and enter pool pressure even when CPA reports a routing `error`.

#### Smoke verdict gate

An absent or false provider smoke verdict produces health-only accounts and schedules no window calls.

#### Bounded staggered fan-out

A 12-account stub transport verifies the scheduler stays below three concurrent calls and completes within its launch budget.

#### Deterministic account cap

Pools larger than 16 accounts schedule only the first 16 deterministic `auth_index` entries per cycle.

#### Unconfigured source null impact

Without both saved connection settings, CPA contributes no usage fields, request path, phase timing, or secret-bearing cache key.

### CPA Management Client Test Specs

Management protocol tests pin the researched CPA response shapes, local endpoint boundary, configuration gate, and display-safe failures.

#### Auth inventory fields

The `/auth-files` fixture preserves required identity, provider, health, runtime-only, and nested Codex account fields.

#### Auth inventory feature detection

Malformed JSON or entries missing required inventory fields must fail as invalid or unsupported without guessing a compatible shape.

#### API call envelope fields

The `api-call` request and response fixtures preserve `authIndex`, method, headers, status code, and nested response body semantics.

#### API call envelope rejection

An envelope missing status, headers, or body must fail as an invalid response rather than yielding partial quota data.

#### Loopback endpoint boundary

Only explicit HTTP(S) loopback hosts pass URL validation; alternate loopback ranges, remote hosts, userinfo, and query-bearing URLs fail closed.

#### Client configuration gate

Client construction requires both an allowed loopback endpoint and a nonblank management key before any request path becomes reachable.

#### Display-safe client errors

Client error strings must omit account email, status detail, management credentials, and auth-index identity even when malformed data contains them.

### CPA Quota Parser Test Specs

Quota tests pin the verified upstream headers and normalize only complete Claude and Codex window responses into CPA-sourced buckets.

#### Upstream request headers

Claude and Codex requests use CPA token substitution plus each upstream's required beta, account, and user-agent headers.

#### Claude windows fixture

The researched Anthropic response maps known and additional windows to account-qualified CPA buckets with normalized resets.

#### Claude malformed windows

Missing or invalid Anthropic utilization fields produce an account-scoped failure instead of numeric quota data.

#### Codex windows fixture

The researched Codex response maps primary and secondary rate-limit windows to duration-normalized, account-qualified CPA buckets and stable labels, regardless of field position.

#### Codex malformed windows

Missing or invalid Codex rate-limit windows produce an account-scoped failure instead of partial numeric quota data.

## Widget Views

Everything below LIMITS is one swappable view region: Usage (the default), Models, and Context, each a compact 360px surface in the same visual system.

The view name and shared 1H/6H/24H/7D range strip live in the region's band header, so switching views keeps the operator's range and there is only ever one control strip. `30d` is absent because a month is not a widget scope. A fresh profile defaults to 1H; the last valid selection persists locally across restarts, while missing, invalid, inaccessible, or unwritable storage degrades safely to 1H without breaking the current selection. The dropdown is a listbox, not a menu, because it has a value. Composition detail lives in [[frontend#Frontend#Components#Widget View Region]].

Most view data is aggregated across all LLM providers; provider-scoped controls appear only where the underlying data model can preserve reliable provider identity.

### Usage View

The default view and the product's core surface: hero chart, one computed insight line, a 3×2 metric readout grid, the switchable breakdown, and a totals footer.

The hero chart stacks one smoothed area per provider for the selected range, with the range's total tokens and an in-range momentum delta overlaid top-left. Its hover/focus legend carries per-provider totals at rest; pointer movement or focused arrow/Home/End keys select any aligned time bucket and replace those totals with its values while brightening its time and drawing a crosshair plus provider markers. Pointer leave, blur, or Escape restores the range summary and final endpoints. The delta compares the back half of the buckets against the front half rather than against the previous window, because a headline delta whose evidence the chart does not draw is not evidence.

The insight line states one computed fact per window — context savings, cached-token volume, or the per-provider split — chosen by a fixed priority that speaks last about anything the widget already draws. A candidate speaks only when its figure exists and is non-zero, so the line is never padded; with no eligible candidate it is simply not drawn.

The readout grid carries six metrics, each with a metric-hue swatch, a value, and a sparkline: **LLM Runtime** (total active time across Claude Code and Codex sessions — model generation, reasoning, and tool execution counted together, user-idle gaps over 5 minutes excluded), **Tokens per LOC**, **LOC per hour** (both over active LLM runtime, not wall clock), **Sessions**, **Projects**, and **Net lines**. Every band reads the region's selected range, so chart, delta, insight, all six readouts and the footer always describe the same window. The Projects readout reuses the selected Projects breakdown when available instead of mounting the same query twice.

The breakdown switches five modes over one row grammar — status dot, name, optional metadata, identity chip, primary value, and activity: Sessions (default), Projects, Hosts, Skills, and Hooks. A Session inserts retained agent count, bot icon, agent-only runtime, displayed root-turn count, and lifetime family runtime before provider. The displayed count adds one only while an open root turn is evidenced; backend analytics remain completed-only. Lifetime runtime turns subdued green under the same predicate as the bright green current-turn clock. Its right edge shows root current-turn runtime while live, a neutral em dash without root-tail evidence, or muted recency while inactive. Between backend snapshots, a one-second display clock extrapolates lifetime total at the additive family rate and current turn at one second per second only while its root accrual bit is true. The clock never polls. A second rail appears only for currently open short-model/runtime pairs, which wrap at the 320px width floor; sessions without open agents stay one 30px row even when lifetime agent totals exist. Positive lifetime totals remain after agents close, while zero or unavailable totals without an open list omit the main-row group. A current-process row shown before retained metrics uses em dashes unless open-root evidence supports the single active turn. Hosts sort by `total_tokens DESC`; projects and sessions by `last_active DESC`; skills and hooks by recognized count descending with a stable name tie-break. The projects sort is enforced both in [[src-tauri/src/storage.rs#Storage#get_project_breakdown]]'s SQL `ORDER BY` and in the post-merge re-sort inside [[src-tauri/src/storage.rs#merge_project_subdirs]], since the subdir-folding step would otherwise override the SQL order. The footer states In/Out/Cache totals and the ⌘M Manage entry.

Honesty disclosures keep the home that matches their data: the Hooks header carries the Claude/Codex tracking-asymmetry help, and the condensed retention line sits in Sessions — the only mode whose source retention actually prunes.

The Skills breakdown reads [[src-tauri/src/storage.rs#Storage#get_skill_breakdown]] via [[src/hooks/useBreakdownData.ts#useBreakdownData]] and shows one row per recognized skill inside the region's selected range. Recognition is intentionally conservative: session indexing stores a skill-use row only for read-like `SKILL.md` loads (Claude `Read` against a `SKILL.md` path, Claude `Skill` tool calls whose plugin prefix is stripped to match Codex's bare folder names, or Codex read commands such as `sed`/`cat`), and ambiguous mentions or skill-file edits are excluded.

The Hooks breakdown (feature 009) reads [[src-tauri/src/storage.rs#Storage#get_hook_breakdown]] via the same hook and shows one row per canonicalized hook identity. Claude rows are per-script: extracted from JSONL `attachment` records via [[src-tauri/src/sessions.rs#extract_hook_invocation_from_attachment]] and canonicalized via [[src-tauri/src/sessions.rs#canonicalize_hook_identity]] so Quill-managed paths collapse to `quill:<basename>`, `${CLAUDE_PLUGIN_ROOT}/<dir>/<file>` stays verbatim, other paths reduce to basename, and missing-command records fall back to `hookName`. Codex rows are per-event because Codex rollouts do not log hook executions — the deployed `hook-observe.cjs` observer captures every fire of eight observed events, including Stop and SessionEnd, via `POST /api/v1/hooks/observed` but cannot attribute individual sibling scripts since Codex registers multiple scripts per event. Rows backed by Quill-deployed scripts carry a `QUILL` chip, so users see Quill's own telemetry overhead alongside their hooks rather than hidden. The schema preserves `cwd` and the `is_sidechain` / `agent_id` columns for future disclosure layers.

Session rows key identity by provider, normalized hostname, and root session. `active_runtime_secs` totals lifetime active time across every native chain once runtime backfill is complete; `runtime_as_of_ms` timestamps that baseline and `active_runtime_rate` states its additive accrual rate. Nullable `agent_count` and `agent_runtime_secs` cover distinct retained sidechain chains only and never infer lifetime work from `observed_agents`; the UI uses only runtime-known active observations to advance a known agent-runtime baseline. `current_turn_runtime_secs` and `current_turn_runtime_active` expose only the root open turn. `turn_count` is lifetime, host-qualified, completed root prompt-response turns only. Candidate-scoped indexed joins read retained hourly totals and open tails without a global lifetime scan. `observed_agents` is nullable live-fold evidence preserving each open agent's id, exact model or type fallback, nullable lifetime runtime, and whether that runtime is actively accruing; null means the fold does not cover that row, while an empty list is a covered session with no open agents. An agent's model comes from its own transcript — a Claude assistant record's `message.model`, a Codex `turn_context` — so it resolves in the same pass that opens the agent and is never inferred from the root row. Each agent renders one identity: known model families shorten to Claude Opus → Sonnet → Haiku → Fable and Codex Sol → Terra → Luna order, an agent type stands in only when model is absent, and `?` appears when both are absent. Instant hover tooltips and accessible labels distinguish lifetime totals from currently open agents. `observed_only` marks active roots whose retained metrics are not available yet. An agent that died mid-turn stays open until the idle window closes it; the UI never claims verified process liveness.

### Models View

Provider-qualified transcript evidence: what is running now, and which models did the work.

A running-now strip pairs each provider's current model with when it took over and what it replaced; below it, the session-ranked model list shows the top five on one shared session scale with attributed tokens beside each id. A model that ran sessions but carries no token figures — the shape a provider that reports usage without per-observation counts takes — reads as an em dash rather than `0`, because a row that exists cannot honestly claim zero. Both bands read the region's range through one usage-overview snapshot. There is no inspect panel — session paging and chain history are deliberately absent from a widget.

Coverage is attributed tokens divided by all token-bearing model observations; carry-forward attribution inherits each chain's running model, so only token activity before any model evidence stays unattributed. It is stated whenever it falls short of 100%. Backfill status keeps recovered results usable while pending, partial, failed, or retrying, and only complete inventory plus final backend scope permits a definitive empty claim. See [[backend#Database#Schema#Model Analytics Evidence]].

Each accepted raw ID appears exactly as observed after surrounding-whitespace trimming and is qualified by provider; no model catalog, family parsing, alias, friendly name, or `unknown` row participates. Provider identity uses the fixed Claude orange, Codex blue, and MiniMax violet code, and each model renders as a rank-assigned shade of its provider's family ramp — the same shade in both bands, never a hue derived from the ID itself.

Data-changing model events invalidate the overview's process-cache key and join the widget's shared five-second-or-longer mounted refresh fan-out. A 60-second fallback poll uses the same path, and one accepted overview refreshes both bands. See [[data-flow#Model Observation Reconciliation]] and [[frontend#Frontend#Components#Widget View Region#Models View]].

### Context View

What the working-context store did with the selected range: preserved and retrieved token headlines, the shared savings line, and what routing cost.

The view is deliberately chartless. Its summary totals and its per-bucket series are computed from different token columns, so plotting the series beneath these headlines would put two disagreeing numbers in one band. The single visualization is a split bar assembled from the exact three figures printed around it: how the range's accounted context tokens divide between preservation, retrieval, and routing.

Only category-scoped totals are read, never the legacy `tokens*Est` columns, because those counted telemetry as savings. A backend that does not categorize therefore reads as zero, and the view says which nothing it is looking at — "context events recorded, none carrying token categories" is a different fact from "no context events in this range". The retention ratio behind the preserved headline is `sources_retrieved / sources_preserved` over distinct `source_ref` values within the window, clamped to `[0, 1]`, computed in [[src-tauri/src/storage.rs#apply_retention_metrics]] from the `CONTEXT_SAVINGS_RETENTION_SQL` CTE. `src/hooks/useContextSavingsStats.ts` listens for the `context-savings-updated` event and invokes [[src-tauri/src/lib.rs#get_context_savings_analytics]].

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

The Learning section (in the [[frontend#Manage Workspace]]) has two tabs — **Rules** and **Memories** (memory optimization) — a provider filter for combined, Claude-only, or Codex-only views, and a Runs toggle that opens run history inline.

The Rules tab splits rules into two sections: **Active Rules** (have `.md` files on disk) and **Discovered** (DB-only candidates). Both rules and runs show provider-scope badges so shared Claude-plus-Codex rules are distinct from provider-specific ones. A `StatusStrip` shows scoped observation counts and a "Run Analysis" button. A `RunHistory` panel docks inline as a right-side overlay over the content (toggled by the toolbar Runs button) and shows past runs with per-phase timing, provider scope, and real-time logs during active runs.

### Rule Storage

Rules are tracked in the `learned_rules` database table with `provider_scope` provenance and optionally written as `.md` files to provider-specific learned-rule directories.

Each row carries a persisted `lifecycle` (`candidate → awaiting_review → active`, plus `rejected`/`suppressed`/`tombstoned`) that is distinct from the read-derived quality `state`. Analysis NEVER writes a `.md` (feature 005 US2 T025, FR-007): extraction only persists DB `candidate` rows. A global `.md` is authored exclusively by the human-gated approval path. Claude-only rules live under `~/.claude/rules/learned/`, Codex-only rules under `~/.config/quill/learned-rules/codex/`, and shared rules under `~/.config/quill/learned-rules/shared/`. Suppression is durable via a name-keyed `rule_tombstones` table consulted at every name-addressed write/activation path, so a deleted pattern cannot be silently resurrected by re-extraction or reconcile.

### Rule Watcher

[[src-tauri/src/rule_watcher.rs]] monitors learned-rule directories for real-time filesystem changes using the `notify` crate.

On Create/Remove/Modify events for `.md` files, a debounced (300ms) reconciliation pass diffs the DB against the filesystem via [[src-tauri/src/storage.rs#Storage#reconcile_learned_rules]]: new files are INSERTed as `lifecycle='candidate'` with frontmatter-parsed metadata (`source = 'manual'`) so they route into the review queue (never auto-active), a deleted file durably tombstones its row (`beta += 5.0`, `lifecycle='tombstoned'`, `rule_tombstones` row `tombstoned_by='reconcile_delete'`), and modified files have their `content`/`content_hash` updated. Steps 3a/3c skip names with an active tombstone or terminal lifecycle so suppression is never overridden. Emits `learning-updated` for instant UI refresh.

### Rule Promotion

Approval via [[src-tauri/src/storage.rs#Storage#promote_learned_rule]] is the SOLE writer of a global learned-rule `.md` (feature 005 US2 T029, FR-007/FR-008).

The route from `candidate` to `awaiting_review` is evidence-grounded (feature 005 US3, C-3/H-1/H-2/M-3, FR-014..018). Every extracted/synthesized candidate carries machine-checkable `evidence_refs` (`{kind,id}`: Stream A injects the real `observations.id`, Stream B a git `%h` short-hash or the snapshot HEAD key, Stream C the indexed `session_id`). Before persistence [[src-tauri/src/storage.rs#Storage#resolve_evidence_refs]] resolves them; a candidate with no resolvable evidence is rejected and not stored (logged, skipped). [[src-tauri/src/storage.rs#Storage#persist_citations_and_advance_version]] writes a retention-proof `rule_evidence_citations` snapshot and repurposes the dead `confirmed_projects` column as the JSON array of distinct project paths among resolved observation citations. The ordering is strictly: `store_learned_rule` (α/β + content merge — always applied; it no longer bumps the version) → the new version's `rule_evidence_citations` are persisted → and ONLY THEN is the pending marker `current_version` advanced, all in one transaction via [[src-tauri/src/storage.rs#Storage#persist_citations_and_advance_version]] (feature 006 Follow-up B, R-B/C-B). This makes the invariant **`current_version` always resolves to a version that has its evidence citations** true by construction: a citation-write failure rolls the whole step back so neither the new rows nor the bump persist and the rule stays review-eligible on its prior snapshot (no transient concurrent-reader window and no permanently un-reviewable human-pending rule; merge-always is preserved because the merge committed separately). After that, [[src-tauri/src/storage.rs#Storage#eligible_for_review]] — one indexed point-read, no N+1, now always reading a cited version — sets `lifecycle='awaiting_review'` iff `evidence_weighted_score ≥ learning.min_eligibility` (Wilson scale, default **0.6**, legacy `learning.min_confidence` read as a fallback) AND `resolved_distinct_refs ≥ 3` AND `distinct_sources ≥ 1` AND `state != invalidated` AND not tombstoned. Each rule's *own* resolved-citation count drives its α/β (fixes the `observation_count=0` bug for Stream B/C-only rules). [[src-tauri/src/storage.rs#Storage#record_rule_reconciliation]] then deterministically supersedes duplicates (`lifecycle='superseded'` + `superseded_by`, survivor = higher evidence-weighted score, tie-broken by observation count then name) and flags conflicts (`lifecycle='conflict_flagged'` on both), so neither is independently surfaced.

Promotion preconditions `lifecycle='awaiting_review'` and an inactive tombstone (else `Err`); the former eval-based regression gate was removed with the counterfactual harness, so promotion no longer consults evaluation results. It runs all DB mutations in ONE transaction — sets `file_path` and `lifecycle='active'`, appends an immutable `rule_versions` row (`change_kind='promote'`), records provenance (`origin_run_id/origin_model/origin_at`) plus a retention-proof `rule_evidence_citations` snapshot — and commits FIRST; only after the commit does it materialize the redacted + injection-sanitized `.md` in the scope dir (path-traversal-guarded) via a temp-file + atomic `rename`, so a crash never leaves a torn or provenance-less orphan file (a post-commit write failure returns `Err` and the dangling DB-active row is self-healed by reconcile step 3b). Re-derivation of a queued rule UPSERTs content (α/β merged in place, never a 2nd row, never overwriting an active `.md`) and the pending `current_version` is bumped only after the new version's `rule_evidence_citations` are persisted, atomically (feature 006 Follow-up B), so a queued rule is never silently stranded with a citation-less `current_version`. A one-time sentinel-guarded legacy archive-then-wipe in the [[src-tauri/src/storage.rs#Storage#init]] chain copies any pre-existing on-disk rules to a read-only manifested archive, deletes the live files, and tombstones their rows before the watcher starts.

## Session Search

Full-text search across Claude Code and Codex session transcripts, powered by Tantivy in [[src-tauri/src/sessions.rs]].

### Indexing

Opening Session Search syncs transcripts using nanosecond mtime plus file size, including removal of search documents for transcripts proven absent by complete source discovery. Metadata failures remain uncached for retry.

Incomplete discovery preserves indexed data. Hook endpoints can also ingest updates; indexed messages include code_changes, commands_run, tool_details, and files_modified metadata. Parent and child chains keep distinct provider-native identities, so result context resolves the exact retained transcript without crossing providers or choosing an ambiguous source.

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

Tools in [[src-tauri/claude-integration/mcp/tools/context.py]] can index text or files, fetch and cache web pages, run bounded commands, search indexed chunks, retrieve focused sources, inspect stats, and purge stored context. File-based tools resolve paths under the selected working directory before reading or preserving content.

The session-history surface in [[src-tauri/claude-integration/mcp/tools/search.py]] is intentionally narrow: only `search_history` remains, after a 30-day usage audit showed the discovery, analytics, and drill-down tools (`list_projects`, `list_sessions`, `get_session_overview`, `get_session_context`, `get_file_history`, `get_branch_activity`, `find_related_sessions`, `get_token_usage`, `get_learned_rules`, `get_tool_details`, `get_index_status`) were called ≤20 times across all sessions tracked. Trimming the surface keeps the tool listing legible and reduces low-value tool-selection noise.

Large execution and batch outputs are stored as `source:N` and `chunk:N` refs. Responses return previews and snippets by default, while [[src-tauri/claude-integration/mcp/tools/context.py#quill_get_context_source]] retrieves bounded chunks when the model needs exact details.

### Routing Hooks

Provider hooks steer high-volume operations toward Quill context tools before they flood the active transcript and track fetched-to-disk paths so the fetch-then-read bypass is closed.

`src-tauri/claude-integration/scripts/context-router.cjs` is the shared bundled router deployed to both Claude and Codex. It blocks raw WebFetch or noisy `curl`/`wget` dumps, nudges broad Bash, Read, Grep, build, and test output toward `quill_*` MCP tools, and surfaces `mcp__quill__quill_execute` as the right alternative for `curl ... | jq` workflows (the previous deny message implicitly invited the workaround by suggesting `curl -sS -o path` as the only mitigation).

When the router denies a raw `curl`/`wget`, it parses the command line via the `extractFetchUrls` helper in `src-tauri/claude-integration/scripts/context-router.cjs` and embeds the first 1–2 distinct URLs into a ready-to-paste tool call inside the deny reason: `mcp__quill__quill_execute(command="curl -sS <URL> | jq .")` for API-shaped URLs (`/api/`, `.json`, `format=json`, or `api.` host) and `mcp__quill__quill_fetch_and_index(url=<URL>)` for HTML/docs/pages. The detection deliberately reads the heredoc-stripped command without stripping quoted args so `curl 'https://…'`, `fetch("https://…")`, and `requests.get("https://…")` all surface; the extractor trims at the first embedded quote, balances trailing `)`, and strips control whitespace (`\r`/`\n`/`\t`) so an attacker-authored URL with a literal newline cannot inject a fake instruction line into the prose deny message. The `looksLikeApiJson` heuristic bails out on binary-artifact extensions (`.tar.gz`, `.zip`, `.pdf`, images, fonts, media, `.wasm`, `.exe`, `.whl`, etc.) before the `api.` host check so binary downloads on `api.*` hosts route to `quill_fetch_and_index` instead of a `jq` pipeline that would mangle them. The previous generic deny gave the model the right tool names but not a copy-paste replacement; a 30-day audit showed only 0.7% of denied sessions ever followed up with a `quill_*` MCP call, so the actionable deny is the smallest change that closes that gap without adding new HTTP infrastructure.

When `curl`/`wget` does pass the network-dump check by writing quietly to a file (e.g. `curl -sS -o /tmp/x.json URL`), the router records the destination path under `~/.config/quill/context/markers/<provider>-<session>/tainted.json` and denies any subsequent Read, or Bash invocation of a pure-reader (`cat`, `bat`, `head`, `tail`, `less`, `more`, `view`, `od`, `xxd`, `strings`, `hexdump`, `sed`, `awk`, `grep`, `rg`, `ack`, `jq`, `yq`, `xq`, `xmllint`), targeting that path. Interpreter execution (`bash /tmp/x.sh`, `python /tmp/x.py`) and removal (`rm /tmp/x.json`) remain allowed so fetch-and-install flows are unaffected. The taint set is capped at 256 paths per session.

`extractFetchOutputPaths` detects fetch commands on the quote-stripped segment (so a `curl -o …` quoted inside a commit message or `echo` string is data, not a fetch, and records nothing) but captures targets from the heredoc-stripped, quote-preserving segment so quoted output paths survive intact: it captures quoted (single or double, spaces allowed) or bare tokens after `-o`, `--output`, and `--output-document` (accepting both `--flag value` and `--flag=value`), plus `>`/`>>` redirect targets, and unquotes each before recording. `-O` is treated as argument-taking only for `wget` segments; `curl`'s `-O`/`--remote-name` takes no argument, so `curl -O URL` records nothing instead of tainting the URL. Command segmentation on `&&`/`||`/`;` uses a quote-tracking splitter so a quoted separator inside an output path does not split the segment. A shared `isDegenerateTaint` guard rejects empty, whitespace-only, and quote-residue tokens (a bare quote pair like `''`/`""`, or a quotes-only basename like `/cwd/''`): `recordTainted` never stores them (raw or resolved) and `loadTainted` filters them out of previously written state files, so `tainted.json` files poisoned by the old bug self-heal on the next script deployment with no state migration. That degenerate-token guard is what fixes a concrete false-denial regression: the old extractor read the quote-stripped command, so a quoted `-o '/tmp/x.json'` collapsed to `-o ''` and was recorded as a bare quote-pair (`''`, plus its cwd-resolved twin `/cwd/''`), which then matched any later reader command containing any quoted argument and produced persistent false `blocked Bash on ''` denials for the rest of the session.

`commandReadsTaintedPath` gates on reader verbs in the quote-stripped command only (a `cat` that appears solely inside quoted data does not open the gate), then matches each tainted path against both the quote-stripped command and an unquote-preserving normalization (`unquoteCommand`), keeping the previous word-boundary anchoring, so a read written as `cat '/tmp/x.json'` is caught instead of collapsing to `cat ''` and escaping the guard while `echo 'next: cat /tmp/x.json'` stays allowed.

Per-session marker files under `~/.config/quill/context/markers/` keep guidance from repeating, and the scripts prune marker directories older than 30 days at most once per day. The taint state file lives inside each session's marker directory and is removed by the same 30-day cleanup. A standalone Node test suite next to the router (`src-tauri/claude-integration/scripts/context-router.test.cjs`) exercises the deny paths and the taint round-trip; run it with `node context-router.test.cjs` from its directory.

### Context Savings Telemetry

Context savings telemetry forwards compact measurements to Quill without copying large context into the main analytics database.

The MCP tools and routing hook send best-effort batches to `/api/v1/context-savings/events` through `context-telemetry.cjs` or the Python telemetry helper in [[src-tauri/claude-integration/mcp/tools/context.py]]. Events record exact bytes when available, refs such as `source:N`, and approximate token estimates using `ceil(bytes / 4)`. The local MCP context database remains the source of large stored content. The `feature.context_telemetry.enabled` flag (see [[features#Settings Window#Integration Features]]) gates whether `context-telemetry.cjs` is deployed at all; the router fails open when it is absent so context preservation keeps working without telemetry side effects.

Each event carries an explicit `category` (`preservation`, `retrieval`, or `routing`) set at the call site by the producer. Token estimates are only auto-defaulted from byte counts for `preservation` and `retrieval` events; routing events default `tokensSavedEst` and `tokensPreservedEst` to 0, and Rust ingestion normalizes those fields back to 0 outside preservation/retrieval so stale producers cannot inflate savings. The Rust ingestion layer derives `category` from `(eventType, decision)` only as a safety net via [[src-tauri/src/context_category.rs#derive_category]] and rejects unknown category strings outside the closed taxonomy. The Python `_attach_context_savings` wrapper in [[src-tauri/claude-integration/mcp/tools/context.py]] also gates its post-response `tokensSavedEst` recomputation on `category in ('preservation', 'retrieval')` so routing tools like `quill_search_context` never accumulate phantom savings from response sizing.

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

## Memory Optimizer

LLM-driven optimization of provider-aware memory and instruction files via [[src-tauri/src/memory_optimizer.rs]].

### Scanning

Recursively scans project directories for Quill memory files plus provider instruction files such as `CLAUDE.md` and `AGENTS.md`.

Filters out denylisted patterns, minified code, and compiled files. Dynamic budget allocation changes based on whether memory files and instruction files are both present.

### Analysis

Assembles an LLM prompt with memory content, provider-scoped instruction files, learned rules, and instinct sections.

Calls Sonnet 4.6 to generate provider-scoped optimization suggestions. Suggestion types: **Delete** (remove redundant), **Update** (improve content), **Merge** (combine related files), **Create** (add missing), **Flag** (needs human review).

### Suggestion Lifecycle

Suggestions follow a status flow: pending -> approved/denied, with backup for undo. Group operations allow batch approve/deny.

Approved suggestions are executed (file written/deleted/merged), with original content backed up. Denied suggestions can be un-denied. Executed suggestions can be undone (restores from backup). Provider instruction-file execution and undo share the integration mutation guard with provider installers from staleness checks through filesystem and status writes; grouped execution holds the guard when any target is an instruction file. Undo of an instruction update restores the original only when live content still exactly matches the applied proposed content, rejecting stale undo rather than overwriting newer changes. Malformed LLM output is filtered before storage so the UI only surfaces actionable suggestions, and `MEMORY.md` is treated as a special index file that can be updated directly but not merged as a source.

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

Claude instances come from Quill state files in `~/.cache/quill/claude-state/` plus process scanning. The restart CJS hook writes `processing` on `UserPromptSubmit`/`PreToolUse`, `idle` on `Stop`/`StopFailure`, and `exited` on `SessionEnd`. Codex instances come from process scanning and `<Codex home>/sessions/` metadata per cwd. Quill emits a Codex restart row only when the cwd has exactly one process and one distinct metadata candidate; ambiguous process or session mappings are omitted.

### Restart Flow

Four-phase orchestration with real-time status events at each phase transition.

(1) Discover instances, (2) wait for idle where supported, (3) send SIGTERM and wait for exit, (4) resume with provider-specific commands. Claude uses `claude --resume`; Codex uses `codex resume`. Each phase emits `restart-status-changed` events.

Codex does not expose a reliable idle signal, so its rows stay `Unknown` before restart and Quill proceeds directly to termination/resume instead of pretending it observed an idle transition.

### Instance Status

Tracked as: Idle, Processing, Unknown, Restarting, Exited, or RestartFailed. The UI shows status indicators per instance with cancel support.

Force restart skips the idle-wait phase.

### Hook Installation

Restart hook actions are provider-aware.

Claude restart setup is on-demand and uses the same pinned [[src-tauri/src/claude_setup.rs#ClaudePaths]] as provider setup. It registers one non-executable `claude-restart-hook.cjs` Node exec-form handler on `UserPromptSubmit`, `PreToolUse`, `Stop`, `StopFailure`, and `SessionEnd`, each with a 2-second timeout. Codex restart setup installs only shared shell integration; provider telemetry/session hooks remain separate.

Restart install, repair, and uninstall snapshot Claude settings, shared and restart ownership state, hook assets, the shell script, and every touched `.bashrc`, `.bash_profile`, or `.zshrc` before mutation. Rollback restores all snapshots, and [[src-tauri/src/restart.rs#startup_cleanup]] recovers an interrupted transaction on app startup. Component flags keep pinned Claude path state until both main and restart ownership are gone, so an uninstall failure remains retryable.

Shell setup owns a bounded `# quill-managed:restart:start` / `# quill-managed:restart:end` block containing one exact source line. Repair removes prior bounded blocks and migrates only the exact legacy `# quill-shell-integration` plus source-line pair; unrelated lines that merely mention Quill survive. Verification requires current script contents, exactly one block in every recorded RC file, and exact hook commands/args/timeouts rather than substring markers.

The shared shell integration is removed only when the last restart-capable provider is disabled. The restart window groups instances by provider and shows setup banners when exact provider verification fails.

## Settings Window

The Settings section of the [[frontend#Manage Workspace]], opened by the titlebar cog (sliders icon), exposes every user-configurable feature toggle in one comprehensive surface, replacing the previous inline `ProviderMenu` popover.

### Window Routing

Rendered as the `settings` section of the Manage workspace ([[src/windows/ManageWindowView.tsx]]); the titlebar cog opens `manage` at that section (via a `manage:navigate` event when it is already open). The former standalone `?view=settings` window was retired.

Settings is always reachable (the Manage workspace never gates it) so users can manage integrations and runtime preferences before any provider is enabled. The shell lives in [[src/windows/SettingsWindowView.tsx]]; its `.settings-tabs` flex container uses `nowrap` so the five top tabs never collapse onto a second row, and its own window chrome (titlebar/close) is suppressed via `manage.css` when embedded in the Manage content pane.

### Tab Layout

Top-tabs navigation hosts five panels: General, Integrations, Context, Learning, and Performance.

| Tab | Panel | Settings |
|-----|-------|----------|
| General | [[src/components/settings/GeneralTab.tsx]] | Always-on-top toggle, an Advanced section with the current-config summary and "Reset to defaults" button covering runtime and learning settings, a "Help improve Quill" toggle that drives the [[features#Crash Reporting]] opt-out, and an About section described in [[features#Settings Window#Version and Release Notes]] |
| Integrations | [[src/components/settings/IntegrationsTab.tsx]] | Status provider selector, Rescan PATH, Activity tracking master toggle, per-provider enable/disable confirmations (with MiniMax API key prompt), in-place MiniMax API-key edit form, and the CPA connection lifecycle |
| Context | [[src/components/settings/ContextTab.tsx]] | Working Context Preservation global toggle, Context savings telemetry sub-toggle (gated on context preservation), and the [[features#Brevity Profile]] global toggle (gated on having any provider enabled), each with descriptive copy explaining what gets installed |
| Learning | [[src/components/settings/LearningTab.tsx]] | Learning trigger mode, periodic enable, periodic interval, min observations, min confidence, plus the Rule Watcher master toggle |
| Performance | [[src/components/settings/PerformanceTab.tsx]] | Live-usage refresh enable + interval (60–600s), model-index rebuild and committed progress from [[frontend#Frontend#Components#Model Rollup Maintenance]], manual database compaction, and the manual retention prune control described in [[frontend#Frontend#Components#Retention Control]] |

### Version and Release Notes

The General tab's About section is the settings surface that reports the running app version and opens the release-notes window.

[[src/components/settings/GeneralTab.tsx]] reads the version once on mount through the Tauri `getVersion()` API and renders it in the About row; a failed read is logged and the row falls back to "Version unavailable" rather than disappearing, so the adjacent "What's new" button always works. That button focuses an existing `release-notes` webview when one is open and otherwise creates it at `/?view=release-notes` ([[src/windows/ReleaseNotesWindow.tsx]]), which keeps the release-notes viewer reachable from settings independently of the main-window chrome.

### Database Compaction

The Performance tab exposes a manual compaction control so operators can reclaim
SQLite space without an automatic maintenance job interrupting active work.

[[src/components/settings/PerformanceTab.tsx#PerformanceTab]] invokes
`compact_database`, disables itself while progress events arrive, and reports the
completed size change or a skipped preflight result inline. Browser fixtures emit
the same event sequence so the settings surface remains inspectable outside Tauri.

### Integration Features

Four global feature flags decide which optional Quill assets get deployed into enabled CLI providers, modeled by the [[src-tauri/src/models.rs#IntegrationFeatures]] struct.

`context_preservation` (default off), `activity_tracking` (default on), `context_telemetry` (default on, gated on `context_preservation`), and `brevity` (default off) are each persisted as `feature.<name>.enabled` keys in the SQLite settings table. The Settings window writes them via `set_context_preservation_enabled`, `set_activity_tracking_enabled`, `set_context_telemetry_enabled`, and `set_brevity_enabled` IPC commands; each setter saves the key, calls [[src-tauri/src/integrations/manager.rs#apply_features_to_enabled_providers]] to reinstall every currently-enabled CLI provider with the merged feature set (and re-sync brevity blocks via `sync_brevity_blocks`), and emits `integration-features-updated` with the full struct so any open Settings window observes the resolved values without a re-fetch. Newly-enabled providers inherit the current feature set automatically. Activity tracking gates the Claude/Codex observation hooks; context telemetry and context preservation gate their context assets. Pi participates in lifecycle sync so later extension payloads receive the same flags, while its lifecycle-only no-op payload has no feature-dependent behavior. Brevity remains limited to Claude and Codex in Pi v1.

### Cross-Window UI Sync

There is no cross-window UI-preference channel any more: the preferences that needed one were deleted with the split-pane main window.

The former [[frontend#Frontend#Custom Hooks#Settings Hooks|useUiPrefs]] hook wrote layout mode, usage-row time mode, and Live/Analytics panel visibility to `localStorage` and emitted a frontend-side `ui-prefs-updated` Tauri event so other webviews re-applied them without a reload. The widget has no layout to choose, no time-visualization modes, and no panels to hide, so the hook, the `UiPrefs` type, the event, the Settings→General controls, and their branch of "Reset to defaults" were all removed rather than kept as settings that configure nothing. The preferences that genuinely cross windows — always-on-top among them — travel on the backend-owned [[features#Settings Window#Runtime Settings IPC]] path instead, which is what keeps the tray checkitem, the Settings toggle, and the widget titlebar one state.

### Runtime Settings IPC

Always-on background tasks expose enable/interval toggles through a single `RuntimeSettings` IPC pair.

[[src-tauri/src/lib.rs#get_runtime_settings]] reads `live_usage.enabled`, `live_usage.interval_seconds`, `rule_watcher.enabled`, `always_on_top`, and `crash_reporting.enabled` from SQLite. [[src-tauri/src/lib.rs#set_runtime_settings]] and the tray's Always-on-Top item both enter [[src-tauri/src/lib.rs#apply_runtime_settings]], whose nonblocking gate admits one writer, requires a live main window for a changed topmost value, applies that native request first, saves every runtime field atomically, synchronizes the tray checkmark, and emits `runtime-settings-updated` so [[src/hooks/useRuntimeSettings.ts#useRuntimeSettings]] mirrors the committed result. Concurrent IPC returns a busy error; a concurrent tray action restores its auto-toggled checkmark from committed settings without blocking the event-loop thread. The tray item's checked value is authoritative intent because it auto-toggles before its event; the backend does not invert a lagging native getter. A reported failure restores prior native, persisted, and checkitem state and emits no desired result. Tauri success does not prove compositor acknowledgement, so a platform window manager may still delay or ignore the request. Toggling `crash_reporting.enabled` calls [[src-tauri/src/crash_reporting.rs#set_enabled]] immediately; live-usage values are reread each loop, and the rule-watcher flag still takes effect at next launch because `notify` holds an OS handle.

### MiniMax API Key Update

The Integrations tab can update a stored MiniMax API key without disabling and re-enabling the integration.

[[src-tauri/src/lib.rs#set_minimax_api_key]] delegates to [[src-tauri/src/integrations/manager.rs#set_minimax_api_key]] which trims the key, persists it via [[src-tauri/src/integrations/minimax.rs#save_api_key]], refreshes provider statuses, and emits `integrations-updated`. The frontend renders an inline `Save` / `Cancel` form; the dialog-based first-enable flow stays unchanged.

### CPA Connection Lifecycle

CPA is an opt-in cross-provider usage source configured from the Integrations tab without becoming a provider status row.

The form defaults to `http://127.0.0.1:8317`, accepts only HTTP(S) loopback URLs, and sends the management key across Tauri only for an explicit connect attempt. [[src-tauri/src/integrations/cpa.rs#validate_connection]] checks `/v0/management/auth-files`, reports typed invalid-URL, hashed-key, unreachable, unauthorized, unsupported-version, and unexpected-response failures, then runs one Claude and Codex quota smoke check when each provider is present. A valid management connection persists `integration.cpa.base_url`, `integration.cpa.management_key`, and boolean `usage.cpa.window_smoke.{claude,codex}` gates; a failed provider smoke check keeps the connection but leaves that provider in health-only mode.

#### Exact plaintext key bytes

Nonblank management keys retain every byte from Settings through validation, HTTP authentication, and local persistence because CPA hashes and verifies the exact plaintext rather than a trimmed form.

#### One-way hash rejection

An exact bcrypt hash shape is rejected before any CPA request with safe recovery copy because CPA replaces configured plaintext with a one-way hash that cannot authenticate as the original key.

[[src-tauri/src/lib.rs#get_cpa_connection_status]] returns only the saved URL and configured state, never the management key. [[src-tauri/src/lib.rs#clear_cpa_connection]] runs the guarded manager purge, deletes both connection settings, every `usage.cpa.*` runtime row, raw CPA snapshots and `usage_hourly` keys under `cpa/%`, then clears the usage cache and advances its epoch so an older in-flight refresh cannot restore disconnected rows. Direct provider snapshots remain intact.

While CPA is configured, LIMITS uses CPA exclusively and suppresses direct provider usage polling. Disconnecting CPA restores polling for every enabled native provider.

#### Ready account smoke selection

Connection smoke checks prefer a ready, available account over a degraded account for each supported provider.

#### Typed safe connect failures

Invalid URL, unreachable, unauthorized, unsupported-version, and unexpected-response failures keep distinct user-safe codes and messages without credential names.

## Crash Reporting

Default-on, user-opt-out crash reporter that ships scrubbed stack traces to Sentry without exposing any session content. Toggled via the "Help improve Quill" row at the bottom of the General settings tab.

### Deny-by-Default Scrubbing

Both surfaces wire a `before_send` hook that strips every dynamic field — messages, exception values, breadcrumbs, request data, user context, extras, and full file paths — before any event leaves the process.

The threat model assumes the entire payload domain is sensitive: panic messages can contain prompts serialized across the Tauri IPC boundary, exception text can interpolate user data, and absolute file paths typically reveal the developer's `$HOME`. Rather than denylist known PII fields, both sides keep only stack-frame structure and the `release`, `environment`, and `runtime` tag allowlist. Rust also clears the hostname-derived `server_name` after SDK integrations run and keeps only `os`, `device`, `rust`, and `app` context keys; Sentry serializes the `rust` entry with context type `runtime`, while release and environment remain top-level SDK metadata. Frontend session replay, browser-tracing, autoSessionTracking, default integrations, and HTTP context capture are all explicitly disabled — the only Sentry features in use are the global error handler and React error boundaries via `reactErrorHandler()`. Rust mirrors the policy with `auto_session_tracking: false`, `max_breadcrumbs: 0`, and a `before_breadcrumb` that drops every breadcrumb. Sentry server-side data scrubbing rules (IP, geolocation, user-agent) remain a follow-up configurable in the project's Sentry settings, not in code.

### Dual-Surface Wiring

Frontend [[src/lib/crashReporting.ts]] and Rust [[src-tauri/src/crash_reporting.rs]] share the same DSN and scrubbing policy.

The Rust side stores its `ClientInitGuard` in a `OnceLock<Mutex<Option<ClientInitGuard>>>` so [[src-tauri/src/crash_reporting.rs#set_enabled]] can drop the guard on opt-out (which flushes pending events and closes the transport) and re-init on opt-in. The frontend calls `Sentry.close()` and `Sentry.init()` for the same effect; one-shot initialization is gated on the `crash_reporting.enabled` value returned by the very first `get_runtime_settings` IPC call from [[src/main.tsx]], so the SDK never sends data before the user's preference is read.

### Toggle Lifecycle

Toggling the "Help improve Quill" row in [[src/components/settings/GeneralTab.tsx]] writes through the standard [[features#Settings Window#Runtime Settings IPC]] pipeline and applies immediately on both surfaces.

[[src-tauri/src/lib.rs#set_runtime_settings]] detects a `crash_reporting_enabled` delta and calls [[src-tauri/src/crash_reporting.rs#set_enabled]] directly on the Rust side, then emits `runtime-settings-updated` carrying the resolved `RuntimeSettings`. The frontend `crashReporting` module listens for that event and calls [[src/lib/crashReporting.ts#setCrashReportingEnabled]] so the React-side SDK opens or closes its transport in lock-step. Default is on; the user-facing copy never mentions Sentry and instead emphasises that session data is removed locally before transmission.

## AppImage Desktop Integration

On Linux the AppImage is the only build and has no desktop presence by default. On first launch Quill offers to add itself to the applications menu, with a Settings control to re-run it. Inert on non-AppImage builds.

Detection uses the `APPIMAGE` env var ([[src-tauri/src/appimage_integration.rs#running_as_appimage]]); the pure [[src-tauri/src/appimage_integration.rs#should_prompt]] gate fires the one-time prompt only when running as an AppImage with no decision yet recorded.

### First-run prompt

[[src-tauri/src/lib.rs#maybe_prompt_appimage_integration]] runs async from `.setup()` so it never blocks startup (mirroring the tray update check).

It shows a native `tauri-plugin-dialog` confirmation. **Add** runs the shared integration routine then an info dialog noting the original download can be deleted; **Not now** persists a `declined` decision so the prompt never returns.

### Integration routine

[[src-tauri/src/appimage_integration.rs#integrate]] backs both the prompt and the Settings control, doing all work in user space (no privilege escalation).

It copies `$APPIMAGE` to `~/Applications/Quill.AppImage` (executable), writes `~/.local/share/applications/quill.desktop`, installs an icon extracted from the running AppImage's `$APPDIR` to `~/.local/share/icons/hicolor/256x256/apps/quill.png`, and best-effort refreshes the desktop/icon caches. It is copy-not-move (the running session stays valid, no relaunch), idempotent, and replaces the AppImage through a same-directory temporary file plus atomic rename. State (`appimage.integration` = `done`/`declined` and `appimage.integration_path`) is persisted only after the filesystem work succeeds; a hidden `.Quill.AppImage.version` sidecar beside the integrated copy records its semantic version.

The module is compiled on every target (its two IPC commands are always registered), so the one platform-specific call — setting the executable bit — is `#[cfg(unix)]`-gated to keep the Windows build compiling; integration itself only ever runs on Linux.

### Automatic version refresh

[[src-tauri/src/appimage_integration.rs#refresh_integrated_appimage]] keeps an existing applications-menu install on the newest AppImage the user launches.

Before single-instance handoff, an integrated AppImage records its running package version when `$APPIMAGE` resolves to `~/Applications/Quill.AppImage`. A loose AppImage replaces that target only when its semantic version is newer; equal or older versions do nothing. This ordering also works while an older Quill process is already running: atomic rename changes the next launcher target without changing that process. The first release carrying the sidecar treats a differing legacy target with no version as predating the feature and refreshes it once; byte-identical legacy copies only seed metadata. Malformed or unreadable metadata fails closed.

### Settings control

The General settings tab ([[src/components/settings/GeneralTab.tsx]]) renders an "Install to applications menu" row only when running as an AppImage, via [[src/hooks/useAppImageIntegration.ts]].

The hook calls [[src-tauri/src/appimage_integration.rs#get_appimage_integration_status]] (reports `is_appimage` + `integrated`, never errors) and [[src-tauri/src/appimage_integration.rs#integrate_appimage]]. The row shows an active install button when not integrated and a disabled "Installed ✓" once done — the path back for users who declined the prompt.

### Install script

The `install.sh` script (repo root) is a `curl | sh` one-liner that handles the *pre-launch* step the app cannot — browsers save downloads non-executable.

It resolves the latest `*_linux_amd64.AppImage` from the GitHub releases API, downloads it to `~/Applications/Quill.AppImage`, marks it executable, and launches it; first-run integration then adds the menu entry. Because it lands the AppImage directly at the integration target, [[src-tauri/src/appimage_integration.rs#copy_appimage]] skips the copy when the source already resolves to the destination; other installs use an atomic replacement.

### Updater interaction

The updater is unchanged: because the menu launches the integrated `~/Applications/Quill.AppImage`, future updates replace that copy in place.

The pre-launch step (a freshly downloaded file lacks an execute bit) is outside the reach of a not-yet-running app; the install script above covers it.
