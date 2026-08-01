// Shared TypeScript interfaces matching Rust models in src-tauri/src/models.rs

export interface UsageBucket {
  provider: IntegrationProvider;
  key: string;
  label: string;
  utilization: number;
  resets_at: string | null;
  sort_order?: number;
}

// Mirrors `ProviderErrorKind` in src-tauri/src/models.rs. "network" means the
// provider's API was unreachable (DNS / connect / timeout) and the poller is
// in offline cooldown; the UI collapses these into a single offline pill.
export type ProviderErrorKind =
  | "network"
  | "config"
  | "auth"
  | "rate_limit"
  | "server"
  // Live polling is paused for a transient, non-failure reason (a stale Claude
  // access token returned 401 while still logged in). Rendered as a muted
  // "Paused" badge by src/components/UsageDisplay.tsx, never a red prompt.
  | "paused"
  // Rows are being served from the last-persisted snapshot during a rate-limit
  // cooldown (a 429 armed it, or one just landed), so the values may be out of
  // date. Rendered as a single muted "Showing cached data" pill (slate, never
  // red) by src/components/UsageDisplay.tsx; the offline pill wins when both
  // network and stale errors are present.
  | "stale";

export interface UsageProviderError {
  provider: IntegrationProvider;
  kind: ProviderErrorKind;
  message: string;
}

export interface ProviderCredits {
  provider: IntegrationProvider;
  balance: string | null;
}

export interface UsageData {
  buckets: UsageBucket[];
  provider_errors: UsageProviderError[];
  provider_credits: ProviderCredits[];
  error: string | null;
}

export interface DataPoint {
  timestamp: string;
  utilization: number;
}

export interface IndicatorMetric {
  provider: IntegrationProvider;
  key: string;
  label: string;
  modelLabel: string | null;
  utilization: number;
  resetsAt: string | null;
  displayResetTime: string | null;
}

export interface StatusIndicatorState {
  configuredPrimaryProvider: IntegrationProvider | null;
  resolvedPrimaryProvider: IntegrationProvider | null;
  status: "ready" | "degraded" | "unavailable";
  titleText: string;
  warning: string | null;
  updatedAt: string | null;
  shortWindow: IndicatorMetric | null;
  weeklyWindow: IndicatorMetric | null;
}

export interface TokenDataPoint {
  timestamp: string;
  input_tokens: number;
  output_tokens: number;
  cache_creation_input_tokens: number;
  cache_read_input_tokens: number;
  total_tokens: number;
}

export interface TokenStats {
  total_input: number;
  total_output: number;
  total_cache_creation: number;
  total_cache_read: number;
  total_tokens: number;
  turn_count: number;
  avg_input_per_turn: number;
  avg_output_per_turn: number;
}

export interface BucketStats {
  provider: IntegrationProvider;
  key: string;
  label: string;
  current: number;
  avg: number;
  max: number;
  min: number;
  time_above_80: number;
  trend: TrendType;
  sample_count: number;
}

export interface HostBreakdown {
  hostname: string;
  total_tokens: number;
  turn_count: number;
  last_active: string;
}

export interface SessionBreakdown {
  provider: IntegrationProvider;
  session_id: string;
  hostname: string;
  total_tokens: number;
  turn_count: number;
  first_seen: string;
  last_active: string;
  project: string | null;
  /**
   * True when this session has at least one sub-agent chain in its
   * transcript. Drives the disclosure + expansion affordance in the Sessions
   * tab. Older backends may omit this; treat missing as `false`.
   *
   * Degrades under retention in the same mixed-horizon way as
   * `subagent_count` below: a pre-cutoff session can still report
   * `has_subagents: true` from its surviving token snapshots while
   * `get_session_subagent_tree` returns nothing.
   */
  has_subagents?: boolean;
  /**
   * Distinct sub-agent count across token_snapshots ∪ response_times ∪
   * tool_actions for this session. Older backends may omit this; treat
   * missing as `0`.
   *
   * **Accepted retention limitation (feature 014).** Retention prunes only
   * `tool_actions` of those three tables, so for a session older than the
   * retention watermark this count is computed over *mixed horizons* and can
   * disagree with its own drilldown — the badge says `+3`, the expanded tree
   * says "no sub-agents". This is documented and rendered as a footnote
   * (`RetentionBanner`, surface `sessions`), not fixed: the fix is rollup
   * aggregates, which are a deferred follow-up. Consumers must mark
   * pre-cutoff sessions via `retentionSpanFor` rather than present the count
   * as exact.
   */
  subagent_count?: number;
}

export interface SkillBreakdown {
  skill_name: string;
  total_count: number;
  claude_count: number;
  codex_count: number;
  project_count: number;
  last_used: string;
}

export interface SkillProjectBreakdown {
  skill_name: string;
  project: string | null;
  hostname: string | null;
  total_count: number;
  claude_count: number;
  codex_count: number;
  last_used: string;
}

/**
 * One row of the Now-tab Hooks breakdown (feature 009). Identity is
 * canonicalized at the backend per FR-003 — Quill-deployed scripts
 * collapse to `quill:<basename>`, `${CLAUDE_PLUGIN_ROOT}/<dir>/<file>`
 * stays verbatim, other paths reduce to basename, and missing-command
 * records fall back to `hookName`. `is_quill` is derived from the
 * `quill:` prefix so callers can classify Quill-managed hook rows.
 */
export interface HookBreakdown {
  hook_identity: string;
  hook_event: string;
  tool_name: string | null;
  is_quill: boolean;
  codex_count: number;
  claude_count: number;
  total_count: number;
  last_fired_at: string;
}

/**
 * One node in a session's sub-agent tree returned by
 * `get_session_subagent_tree`. Today every chain originates from the parent
 * transcript so depth-1 sub-agents always carry `parent_agent_id = null`,
 * but the field is reserved for future depth-N chains.
 */
export interface SubagentNode {
  agent_id: string;
  parent_agent_id: string | null;
  first_seen: string;
  last_active: string;
  turn_count: number;
  total_tokens: number;
  input_tokens: number;
  output_tokens: number;
  cache_creation_tokens: number;
  cache_read_tokens: number;
  tool_call_count: number;
  /** Human-readable label. Null today; populated in a future wave. */
  label: string | null;
}

export interface ProjectBreakdown {
  project: string;
  hostname: string;
  total_tokens: number;
  turn_count: number;
  session_count: number;
  last_active: string;
}

export type LayoutMode = "stacked" | "side-by-side";

export type TimeMode = "marker" | "dual" | "background";

/**
 * The analytics range vocabulary. Note what is *absent*: there is no `all`
 * member, and `range_to_duration` (src-tauri/src/storage.rs) has no `all` arm
 * to feed one — every range reader is capped at 30 days.
 *
 * **Retention invariant (feature 014).** That cap is load-bearing. The
 * retention preset floor is 30 days, so `get_code_stats`,
 * `get_code_stats_history` and `get_llm_runtime_stats` provably cannot reach
 * a pruned row and need no degradation treatment. Any future all-time or
 * otherwise unbounded range added here that reads `tool_actions` or
 * `session_events` breaks that proof, and must therefore:
 *
 * 1. be labelled "all retained" rather than "all time" — data below the
 *    retention watermark is deleted, not zero; and
 * 2. render `RetentionBanner` on every surface that draws it.
 *
 * The two "All time" toggles in `BreakdownPanel` are exempt and must stay
 * labelled "All time": they read `skill_usages` and `hook_invocations`, which
 * retention never prunes, so relabelling them would itself be a lie.
 */
export type RangeType = "1h" | "6h" | "24h" | "7d" | "30d";
export type CodexLiveRange = "1h" | "6h" | "12h" | "24h";

export type TrendType = "up" | "down" | "flat" | "unknown";

export type BreakdownMode = "hosts" | "projects" | "sessions" | "skills" | "hooks";

export type SortMode = "relevance" | "recency";

export interface BreakdownSelection {
  type: "host" | "project" | "session";
  key: string;
  firstSeen: string;
  lastActive: string;
  provider?: IntegrationProvider;
  sessionId?: string;
}

export type SectionId = "live" | "analytics";

export interface SectionConfig {
  id: SectionId;
  visible: boolean;
}

export interface PendingUpdate {
  version: string;
  downloadAndInstall: () => Promise<void>;
}

// Integration provider types

export type IntegrationProvider = "claude" | "codex" | "mini_max";
export type IndicatorPrimaryProvider = IntegrationProvider | null;
export type ProviderFilter = "all" | IntegrationProvider;

export type ProviderSetupState =
  | "not_installed"
  | "installing"
  | "installed"
  | "uninstalling"
  | "missing"
  | "error";

export interface ProviderStatus {
  provider: IntegrationProvider;
  detectedCli: boolean;
  detectedHome: boolean;
  enabled: boolean;
  setupState: ProviderSetupState;
  userHasMadeChoice: boolean;
  lastError: string | null;
  lastVerifiedAt: string | null;
  /**
   * Filesystem locations Quill checked when trying to find this provider's
   * CLI. Populated only when `detectedCli` is false so the integrations menu
   * can explain why the provider shows N/A despite being installed.
   */
  lastDetectionAttempts?: string[];
}

export interface ContextPreservationStatus {
  enabled: boolean;
  hasContextSavingsEvents: boolean;
}

// Code change stats types

export interface LanguageBreakdown {
	language: string;
	lines: number;
	percentage: number;
}

export interface CodeStats {
	lines_added: number;
	lines_removed: number;
	net_change: number;
	session_count: number;
	avg_per_session: number;
	by_language: LanguageBreakdown[];
}

export interface CodeStatsHistoryPoint {
	timestamp: string;
	lines_added: number;
	lines_removed: number;
	total_changed: number;
}

/**
 * Per-session line counts from `get_batch_session_code_stats`.
 *
 * **Degrades under retention (feature 014).** The command reads `tool_actions`,
 * so a session older than the retention watermark returns all-zero counts that
 * are indistinguishable from "this session changed no code". Consumers must
 * classify the session with `retentionSpanFor` and render `PRUNED_PLACEHOLDER`
 * instead of a zero when the span is `pruned`.
 */
export interface SessionCodeStats {
	lines_added: number;
	lines_removed: number;
	net_change: number;
}

export interface CodexLiveCountSeries {
  value: number;
  sparkline: SparklinePoint[];
  lastActivityAt: string | null;
}

export interface CodexLiveTokenSeries {
  value: number;
  sparkline: SparklinePoint[];
  lastActivityAt: string | null;
}

export interface CodexLiveSessionRow {
  provider: "codex";
  sessionId: string;
  hostname: string;
  project: string | null;
  firstSeen: string;
  lastActive: string;
  tokens: number;
  turnEstimate: number;
  linesAdded: number;
  linesRemoved: number;
  netChange: number;
}

export interface CodexLiveData {
  fetchedAt: string;
  lastActivityAt: string | null;
  tokens: CodexLiveTokenSeries;
  activeSessions: CodexLiveCountSeries;
  activeProjects: CodexLiveCountSeries;
  activityPulse: SparklinePoint[];
  sessions: CodexLiveSessionRow[];
}

// Learning system types

export type LearningTriggerMode = "on-demand" | "periodic";

export interface LearningSettings {
  enabled: boolean;
  trigger_mode: LearningTriggerMode;
  periodic_minutes: number;
  min_observations: number;
  min_confidence: number;
}

export interface RuntimeSettings {
  liveUsageEnabled: boolean;
  liveUsageIntervalSeconds: number;
  ruleWatcherEnabled: boolean;
  alwaysOnTop: boolean;
  crashReportingEnabled: boolean;
}

export interface IntegrationFeatures {
  contextPreservation: boolean;
  activityTracking: boolean;
  contextTelemetry: boolean;
  brevity: boolean;
}

/**
 * Operator feedback verdict for a rule (feature 005 US3 / R-5). `accept` and
 * `reject` carry the same trust level as promote/delete; `bad` is the
 * strongest negative and writes a durable tombstone. The optional `note` is
 * maintainer-only local metadata and is never fed to inference.
 */
export type OperatorFeedback = "accept" | "reject" | "bad";

/**
 * Rule lifecycle strings. `state` is a free-form string from the backend;
 * these are the known values. US3 added `superseded`/`conflict_flagged` to the
 * existing candidate/awaiting_review/active/rejected/suppressed/tombstoned set
 * (plus legacy emerging/confirmed/stale/invalidated). Treat unknown strings
 * tolerantly — render the raw value, never throw.
 */
export type RuleLifecycle =
	| "candidate"
	| "awaiting_review"
	| "active"
	| "rejected"
	| "suppressed"
	| "tombstoned"
	| "superseded"
	| "conflict_flagged"
	// Legacy evidence states still emitted by older backends.
	| "emerging"
	| "confirmed"
	| "stale"
	| "invalidated"
	| (string & {});

/**
 * Terminal / non-active lifecycle states. A rule in any of these MUST NOT be
 * presented as an active on-disk rule even if it still has a `file_path`
 * (feature 005 US3 — superseded/conflict_flagged/rejected/tombstoned).
 */
export const NON_ACTIVE_LIFECYCLES: ReadonlySet<string> = new Set([
	"rejected",
	"suppressed",
	"tombstoned",
	"superseded",
	"conflict_flagged",
	"invalidated",
]);

/**
 * True when a rule should be treated as a live, active on-disk rule. Requires
 * a written `.md` file AND a lifecycle that is not terminal/superseded.
 */
export function isActiveRule(rule: LearnedRule): boolean {
	return rule.file_path.length > 0 && !NON_ACTIVE_LIFECYCLES.has(rule.state);
}

export interface LearnedRule {
  name: string;
  domain: string | null;
  confidence: number;
  observation_count: number;
  file_path: string;
  created_at: string;
  updated_at: string;
  state: RuleLifecycle;
  project: string | null;
  is_anti_pattern: boolean;
  source: string | null;
	content: string | null;
  provider_scope: IntegrationProvider[];
  /**
   * Current operator feedback verdict, if the backend read model exposes it.
   * The base `LearnedRule` Rust model does not carry it today, so consumers
   * MUST treat it as optional and absent-by-default.
   */
  feedback?: OperatorFeedback | null;
}

export interface RunPhase {
	name: string;
	status: string;
	duration_ms: number | null;
	findings_count: number;
}

/**
 * One inference call recorded during a learning run (feature 005 R-7 / H-6).
 * Field names are snake_case to match the serde JSON emitted by the Rust
 * `RunInferenceCall` model decoded from `learning_runs.inference_metadata`.
 * Legacy/micro runs carry no inference metadata at all (see
 * `RunInferenceSummary` being optional on `LearningRun`).
 */
/**
 * Honest per-call OS-confinement descriptor (feature 006 Follow-up A,
 * R-A / C-A). `sandbox` is the recorded tag from the closed Rust
 * `SandboxKind` vocabulary (`bwrap` | `process-only` | `sandbox-exec` |
 * `job-object` | `none`). `fs_confined` is `true` only when that mechanism
 * actually denies out-of-workspace filesystem read/write
 * (`bwrap`/`sandbox-exec`); `process-only`/`job-object`/`none` ⇒ `false`.
 * Field names are snake_case to match serde JSON.
 */
export interface RunInferenceConfinement {
	sandbox: string;
	fs_confined: boolean;
}

export interface RunInferenceCall {
	phase: string;
	model: string | null;
	cost_usd: number;
	duration_ms: number;
	ttft_ms: number;
	input_tokens: number;
	output_tokens: number;
	success: boolean;
	failure_kind: string | null;
	/**
	 * Confinement actually applied to this call. Optional: absent on legacy
	 * records that recorded no `sandbox` tag (the backend skips the field
	 * when unknown), so consumers MUST treat it as optional.
	 */
	confinement?: RunInferenceConfinement;
}

/**
 * Derived per-run inference rollup (feature 005 R-7 / H-6 / FR-024). Decoded
 * tolerantly from the existing `learning_runs.inference_metadata` JSON — a
 * NULL or parse-error column yields no summary, so this is optional/nullable
 * on `LearningRun` and consumers MUST render gracefully (em-dash) when absent.
 * `primary_model` is the cost-dominant model and may be null when no call
 * carried attributable cost. Field names are snake_case to match serde JSON.
 */
export interface RunInferenceSummary {
	total_cost_usd: number;
	total_duration_ms: number;
	primary_model: string | null;
	call_count: number;
	failed_call_count: number;
	calls: RunInferenceCall[];
	/**
	 * Run-level confinement rollup (feature 006 Follow-up A, R-A / C-A):
	 * `true` iff every call that recorded a `sandbox` tag was
	 * filesystem-confined; `false` if any recorded call ran without
	 * filesystem confinement. Optional/absent when no call carried a
	 * `sandbox` tag (legacy records) — render unchanged when absent.
	 */
	all_fs_confined?: boolean;
}

export interface LearningRun {
  id: number;
  trigger_mode: string;
  observations_analyzed: number;
  rules_created: number;
  rules_updated: number;
  duration_ms: number | null;
  status: string;
  error: string | null;
  logs: string | null;
  created_at: string;
  phases: RunPhase[] | null;
  provider_scope: IntegrationProvider[];
  /**
   * Derived inference rollup. Absent on legacy runs and runs whose
   * `inference_metadata` was NULL or failed tolerant decode on the backend;
   * consumers MUST treat it as optional and render an em-dash, never crash.
   */
  inference?: RunInferenceSummary | null;
}

export interface LearningLogEvent {
  run_id: number;
  message: string;
}

export interface ToolCount {
  tool_name: string;
  count: number;
}

// Session search types

export interface SessionRef {
  provider: IntegrationProvider;
  session_id: string;
}

export function usageBucketRefKey(
  bucket: Pick<UsageBucket, "provider" | "key">,
): string {
  return `${bucket.provider}:${bucket.key}`;
}

// Unified bucket that groups multiple providers sharing the same label
export interface MergedBucket {
  label: string;
  sources: UsageBucket[];
  utilization: number;
  resets_at: string | null;
}

export function mergeBucketsByLabel(buckets: UsageBucket[]): MergedBucket[] {
  const groups = new Map<string, UsageBucket[]>();
  for (const bucket of buckets) {
    const existing = groups.get(bucket.label) ?? [];
    existing.push(bucket);
    groups.set(bucket.label, existing);
  }
  return Array.from(groups.entries()).map(([label, sources]) => ({
    label,
    sources,
    utilization:
      sources.reduce((sum, s) => sum + s.utilization, 0) / sources.length,
    resets_at:
      sources
        .map((s) => s.resets_at)
        .filter((r): r is string => r !== null)
        .sort()[0] ?? null,
  }));
}

export function sessionRefKey(ref: SessionRef): string {
  return `${ref.provider}:${ref.session_id}`;
}

export interface SearchFilters {
  provider?: IntegrationProvider;
  project?: string;
  host?: string;
  role?: "user" | "assistant";
  date_from?: string;
  date_to?: string;
  git_branch?: string;
}

export interface SearchHit {
  provider: IntegrationProvider;
	message_id: string;
	session_id: string;
	content: string;
	snippet: string;
	role: string;
	project: string;
	host: string;
	git_branch: string;
	timestamp: string;
	tools_used: string;
	files_modified: string;
	code_changes: string;
	commands_run: string;
	tool_details: string;
	score: number;
}

export interface SearchResults {
  hits: SearchHit[];
  total_hits: number;
  query_time_ms: number;
}

export interface FacetCount {
  name: string;
  count: number;
}

export interface SearchFacets {
  providers: FacetCount[];
  projects: FacetCount[];
  hosts: FacetCount[];
}

export interface ContextMessage {
	message_id: string;
	role: string;
	content: string;
	tool_summary: string;
	tools_used: string;
	timestamp: string;
	is_match: boolean;
}

export interface SessionContext {
  provider: IntegrationProvider;
  messages: ContextMessage[];
  session_id: string;
  project: string;
}

// Analytics redesign types

export type ModelRange = "1h" | "6h" | "24h" | "7d" | "30d";

export interface ModelIdentity {
  provider: string;
  modelId: string;
}

declare const modelIdentityKeyBrand: unique symbol;

/**
 * Stable frontend key for an opaque provider/model pair. The branded type
 * prevents callers from accidentally substituting a delimiter-built string.
 */
export type ModelIdentityKey = string & {
  readonly [modelIdentityKeyBrand]: true;
};

/**
 * JSON tuple encoding preserves string boundaries and cannot collide when
 * provider or model IDs contain delimiters or other arbitrary characters.
 */
export function modelIdentityKey(identity: ModelIdentity): ModelIdentityKey {
  return JSON.stringify([identity.provider, identity.modelId]) as ModelIdentityKey;
}

export type ModelBackfillTrigger =
  | "migration"
  | "startup_resume"
  | "retry"
  | "reconcile";

export type ModelBackfillState =
  | "pending"
  | "running"
  | "complete"
  | "partial"
  | "failed";

export interface ModelBackfillStatus {
  generation: number;
  trigger: ModelBackfillTrigger;
  status: ModelBackfillState;
  totalRoots: number;
  completedRoots: number;
  failedRoots: number;
  inventoryComplete: boolean;
  totalSources: number;
  processedSources: number;
  failedSources: number;
  skippedSources: number;
  remainingSources: number;
  observationsWritten: number;
  startedAt: string | null;
  updatedAt: string;
  finishedAt: string | null;
  lastError: string | null;
}

export interface ModelAnalyticsScope {
  globalSessionCount: number;
  scopedSessionCount: number;
  scopedEvidenceCount: number;
  inventoryComplete: boolean;
  scopeFinal: boolean;
}

export interface ModelUsageOverviewTotals {
  sessions: number;
  projects: number;
  turns: number;
  attributedTokens: number;
  totalTokens: number;
  coveragePercent: number | null;
  distinctModels: number;
  multiModelSessions: number;
}

export interface ModelRunningNowEntry {
  provider: string;
  modelId: string;
  lastSeenAt: string;
  runningSinceAt: string;
  previousModelId: string | null;
}

export interface ModelUsageOverviewRow {
  identity: ModelIdentity;
  sessions: number;
  sessionPercent: number | null;
  projects: number;
  turns: number;
  primaryIn: number;
  daysActive: number;
  attributedTokens: number;
  sharePercent: number | null;
  firstSeen: string;
  lastSeen: string;
}

export interface ModelActivitySeries {
  identity: ModelIdentity;
  sessionsPerBucket: number[];
}

export interface ModelActivity {
  bucketSeconds: number;
  bucketStarts: string[];
  series: ModelActivitySeries[];
}

export interface ModelProjectMatrixCell {
  identity: ModelIdentity;
  sessions: number;
}

export interface ModelProjectMatrixRow {
  project: string;
  totalSessions: number;
  cells: ModelProjectMatrixCell[];
}

export interface ModelCombinationPair {
  a: ModelIdentity;
  b: ModelIdentity;
  sharedSessions: number;
}

export interface ModelCombinations {
  single: number;
  dual: number;
  threePlus: number;
  topPairs: ModelCombinationPair[];
}

export interface ModelDelegationTop {
  identity: ModelIdentity;
  sharePercent: number;
}

export interface ModelDelegation {
  parentTokens: number;
  subagentTokens: number;
  parentTop: ModelDelegationTop | null;
  subagentTop: ModelDelegationTop | null;
}

export interface ModelUsageOverviewResponse {
  generatedAt: string;
  range: ModelRange;
  provider: string | null;
  representedProviders: string[];
  scope: ModelAnalyticsScope;
  backfill: ModelBackfillStatus;
  totals: ModelUsageOverviewTotals;
  runningNow: ModelRunningNowEntry[];
  models: ModelUsageOverviewRow[];
  activity: ModelActivity;
  projectMatrix: ModelProjectMatrixRow[];
  combinations: ModelCombinations;
  delegation: ModelDelegation;
}

export interface ModelSessionRow {
  provider: string;
  sessionId: string;
  displayName: string;
  cwd: string | null;
  hostname: string | null;
  selectedModelTokens: number;
  selectedModelTurns: number;
  lastActivityAt: string;
  primaryModel: ModelIdentity;
  distinctModels: number;
  hasWithinChainSwitches: boolean;
  chainCount: number;
}

export interface ModelSessionsResponse {
  identity: ModelIdentity;
  total: number;
  nextCursor: string | null;
  sessions: ModelSessionRow[];
}

export interface SessionModelSegmentModel {
  kind: "model";
  identity: ModelIdentity;
  startedAt: string;
  endedAt: string;
  turnCount: number;
  attributedTokens: number;
}

export interface SessionModelSegmentGap {
  kind: "modelGap";
  startedAt: string;
  endedAt: string;
  turnCount: number;
}

export type SessionModelSegment =
  | SessionModelSegmentModel
  | SessionModelSegmentGap;

export type SessionModelChainKind = "parent" | "subagent";

export interface SessionModelChain {
  chainId: string;
  parentChainId: string | null;
  kind: SessionModelChainKind;
  agentId: string | null;
  switchCount: number;
  attributedTokens: number;
  unattributedTokens: number;
  segments: SessionModelSegment[];
}

export interface SessionModelHistoryResponse {
  provider: string;
  sessionId: string;
  displayName: string;
  primaryModel: ModelIdentity | null;
  distinctModels: number;
  switchCount: number;
  attributedTokens: number;
  unattributedTokens: number;
  chains: SessionModelChain[];
}

export interface ModelAnalyticsUpdatedEvent {
  generation: number;
  status: ModelBackfillState;
  dataChanged: boolean;
  updatedAt: string;
}

export type ModelAnalyticsErrorCode =
  | "invalid_range"
  | "invalid_provider"
  | "invalid_model_id"
  | "invalid_cursor"
  | "not_found"
  | "storage_error";

export interface ModelAnalyticsError {
  code: ModelAnalyticsErrorCode;
  message: string;
}

export type AnalyticsTab = "now" | "trends" | "charts" | "models" | "context";

export type ContextSavingsEstimateConfidence =
	| "exact"
	| "high"
	| "medium"
	| "low"
	| "none"
	| number
	| string;

export interface ContextSavingsSummary {
	eventCount: number;
	routerEventCount: number;
	continuityEventCount: number;
	indexedBytes: number;
	returnedBytes: number;
	inputBytes: number;
	tokensIndexedEst: number;
	tokensReturnedEst: number;
	tokensSavedEst: number;
	tokensPreservedEst: number;
	// Category-scoped totals from backend.  Older backends omit these
	// fields entirely; consumers MUST default to 0 (not the legacy
	// tokens*Est columns) so a stale backend does not silently re-surface
	// the pre-fix inflated headline that this taxonomy was added to remove.
	tokensPreserved?: number;
	tokensRetrieved?: number;
	tokensRouting?: number;
	telemetryEventCount?: number;
	routingEventCount?: number;
	sourcesPreserved?: number;
	sourcesRetrieved?: number;
	retentionRatio?: number;
}

export interface ContextSavingsTimeSeriesPoint {
	timestamp: string;
	eventCount: number;
	routerEventCount: number;
	continuityEventCount: number;
	indexedBytes: number;
	returnedBytes: number;
	inputBytes: number;
	tokensIndexedEst: number;
	tokensReturnedEst: number;
	tokensSavedEst: number;
	tokensPreservedEst: number;
}

export interface ContextSavingsBreakdownRow {
	provider: IntegrationProvider | string | null;
	eventType: string;
	source: string;
	eventCount: number;
	indexedBytes: number;
	returnedBytes: number;
	inputBytes: number;
	tokensIndexedEst: number;
	tokensReturnedEst: number;
	tokensSavedEst: number;
	tokensPreservedEst: number;
	estimateConfidence: ContextSavingsEstimateConfidence | null;
}

export interface ContextSavingsBreakdownGroup {
	key: string;
	eventCount: number;
	deliveredCount?: number;
	indexedBytes: number;
	returnedBytes: number;
	inputBytes: number;
	tokensIndexedEst: number;
	tokensReturnedEst: number;
	tokensSavedEst: number;
	tokensPreservedEst: number;
}

export interface ContextSavingsBreakdownsResponse {
	byProvider?: ContextSavingsBreakdownGroup[];
	byEventType?: ContextSavingsBreakdownGroup[];
	bySource?: ContextSavingsBreakdownGroup[];
	byDecision?: ContextSavingsBreakdownGroup[];
	byCwd?: ContextSavingsBreakdownGroup[];
}

export interface ContextSavingsEvent {
	eventId: string;
	provider: IntegrationProvider;
	sessionId: string | null;
	hostname: string;
	cwd: string | null;
	timestamp: string;
	eventType: string;
	source: string;
	decision: string | null;
	category: string;
	reason: string | null;
	delivered: boolean;
	indexedBytes: number | null;
	returnedBytes: number | null;
	inputBytes: number | null;
	tokensIndexedEst: number | null;
	tokensReturnedEst: number | null;
	tokensSavedEst: number | null;
	tokensPreservedEst: number | null;
	estimateMethod: string | null;
	estimateConfidence: ContextSavingsEstimateConfidence | null;
	sourceRef: string | null;
	snapshotRef: string | null;
	createdAt: string;
}

export interface ContextSavingsAnalytics {
	range: RangeType;
	generatedAt: string;
	summary: ContextSavingsSummary;
	timeSeries: ContextSavingsTimeSeriesPoint[];
	breakdowns: ContextSavingsBreakdownRow[];
	recentEvents: ContextSavingsEvent[];
}

export interface ContextSavingsAnalyticsResponse
	extends Omit<ContextSavingsAnalytics, "timeSeries" | "breakdowns"> {
	timeSeries?: ContextSavingsTimeSeriesPoint[];
	timeseries?: ContextSavingsTimeSeriesPoint[];
	breakdowns?: ContextSavingsBreakdownRow[] | ContextSavingsBreakdownsResponse;
}

export interface InsightTrend {
	direction: "up" | "down" | "flat";
	percentage: number;
	/** Whether "up" is good (true) or bad (false). Null = neutral. */
	upIsGood: boolean | null;
}

export interface SparklinePoint {
	value: number;
}

export interface SessionHealthStats {
	avgDurationSeconds: number;
	avgTokens: number;
	sessionsPerDay: number;
	sessionCount: number;
	prev: {
		avgDurationSeconds: number;
		avgTokens: number;
		sessionsPerDay: number;
		sessionCount: number;
	};
}

export interface ActivityPatternData {
	/** 24 values, index 0 = midnight, index 23 = 11pm */
	hourlyTokens: number[];
	peakStart: number;
	peakEnd: number;
}

export interface LearningStatsData {
	total: number;
	emerging: number;
	confirmed: number;
	/** 5 buckets: [0-20%, 20-40%, 40-60%, 60-80%, 80-100%] */
	confidenceBuckets: number[];
	newThisWeek: number;
}

export interface ProjectTokensRaw {
	project: string;
	total_tokens: number;
	session_count: number;
}

export interface SessionStatsRaw {
	avg_duration_seconds: number;
	avg_tokens: number;
	session_count: number;
	total_tokens: number;
}

// Charts types

export interface MergedDataPoint {
	timestamp: string;
	utilization: number | null;
	total_tokens: number | null;
	total_lines_changed: number | null;
}

export interface ChartSeriesVisibility {
	utilization: boolean;
	tokens: boolean;
}

// LLM runtime types

export interface LlmRuntimeStats {
	total_runtime_secs: number;
	turn_count: number;
	session_count: number;
	avg_per_turn_secs: number;
	sparkline: number[];
}

// Restart feature types

export interface RestartInstance {
	provider: IntegrationProvider;
	pid: number;
	session_id: string | null;
	cwd: string;
	tty: string;
	terminal_type: TerminalType;
	status: InstanceStatus;
	last_seen: string;
}

export type TerminalType =
	| { type: "Tmux"; target: string }
	| { type: "Plain" };

export type InstanceStatus =
	| "Idle"
	| "Processing"
	| "Unknown"
	| "Restarting"
	| "Exited"
	| { RestartFailed: { error: string } };

export type RestartPhase =
	| "Idle"
	| "WaitingForIdle"
	| "Restarting"
	| "Complete"
	| "Cancelled"
	| "TimedOut";

export interface RestartStatus {
	phase: RestartPhase;
	instances: RestartInstance[];
	waiting_on: number;
	elapsed_seconds: number;
}

// Retention pruning types (feature 014). These mirror the Rust shapes in
// src-tauri/src/retention.rs and the four retention commands in
// src-tauri/src/lib.rs; field names stay snake_case because they arrive
// straight off `invoke()` / `listen()` with no camelCase mapping layer.
//
// The inline CompactDatabaseProgress / CompactDatabaseResult types in
// src/components/settings/PerformanceTab.tsx stay where they are — retention
// deliberately does not refactor the compaction control it sits beside.

// Per-table counts, carried twice by an audit record and once by a result:
// rows actually removed, and rows left in place because their timestamp did
// not satisfy the conformance guard (`length = 24 AND LIKE '%Z'`).
export interface RetentionTableCounts {
	tool_actions: number;
	session_events: number;
	model_usage_observations: number;
}

// "partial" means chunks committed and then the run stopped — mid-run disk
// exhaustion or a chunk-level SQL error. It is a third status rather than
// "completed" plus an `interrupted` flag because a status that has to be read
// together with a boolean is a status that will be read wrong.
export type RetentionRunStatus = "completed" | "partial" | "skipped";

// Durable record of the most recent run, stored as the JSON value of the
// `retention.last_run` setting. A skipped run is recorded exactly like a
// completed one: "I tried on this date and nothing happened, because X" is the
// question the record exists to answer once the toast is gone.
export interface RetentionAuditRecord {
	schema: number;
	status: RetentionRunStatus;
	// Skip reason. Null for a partial, whose explanation is `error_reason`.
	reason: string | null;
	// Populated if and only if `status` is "partial".
	error_reason: string | null;
	window_days: number | null;
	cutoff: string | null;
	// Conforming timestamp of the moment the run finished.
	ran_at: string;
	deleted: RetentionTableCounts;
	skipped_nonconforming: RetentionTableCounts;
	bytes_before: number;
	bytes_after: number;
}

// Returned by `get_retention_policy` and `set_retention_policy`. Every field is
// independently nullable because a fresh database carries none of the three
// settings rows, and absent on all three is the default everywhere.
export interface RetentionPolicy {
	// null means never prune. Only 30, 90, 180 and 365 are accepted on write —
	// the 30-day floor that keeps the range-capped readers provably unaffected.
	window_days: number | null;
	// Insert-time cutoff; null means no filtering.
	watermark: string | null;
	last_run: RetentionAuditRecord | null;
}

// Returned by `preview_retention`. `cutoff` is not decoration: it is the token
// the confirm step hands back to `run_retention_maintenance`, and a preview is
// the only way to obtain one, so the backend itself guarantees no destructive
// run without a preview.
export interface RetentionPreview {
	status: "ready" | "skipped";
	// Structured skip reason: retention disabled, nothing older than the
	// cutoff, or another maintenance operation already holding the lease.
	reason: string | null;
	cutoff: string | null;
	window_days: number | null;
	tool_actions_rows: number;
	session_events_rows: number;
	model_usage_observations_rows: number;
	tool_actions_nonconforming: number;
	session_events_nonconforming: number;
	// True when the cutoff covers every owned row — drives the explicit-loss
	// confirmation copy rather than the ordinary "older than N days" copy.
	everything_older: boolean;
	bytes_before: number;
	// Capability loss the confirm step must show, pre-cutoff only: session
	// drilldowns, subagent trees, batch session code stats.
	affected_surfaces: string[];
}

// Payload of `retention-maintenance-progress`, identical in shape to the
// compaction progress event. `preview_retention` reuses this event for its
// counting phase so the UI needs only one listener pair.
export interface RetentionMaintenanceProgress {
	phase: string;
	pct: number;
}

// Payload of `retention-maintenance-finished` and the return of
// `run_retention_maintenance`. `compaction_status` is reported separately from
// `status` on purpose: rows removed with bytes not yet reclaimed is a
// legitimate outcome, so "completed" with a skipped compaction and
// `bytes_after === bytes_before` must stay expressible.
export interface RetentionMaintenanceResult {
	status: RetentionRunStatus;
	// Skip reason; null otherwise.
	reason: string | null;
	// Populated if and only if `status` is "partial".
	error_reason: string | null;
	cutoff: string | null;
	window_days: number | null;
	tool_actions_deleted: number;
	session_events_deleted: number;
	model_usage_observations_deleted: number;
	tool_actions_nonconforming: number;
	session_events_nonconforming: number;
	compaction_status: "completed" | "skipped";
	compaction_reason: string | null;
	bytes_before: number;
	bytes_after: number;
	// Absolute local JSONL path when Archive & prune completed its archive step.
	archive_path: string | null;
	tool_actions_archived: number;
	session_events_archived: number;
	model_usage_observations_archived: number;
}
