// Shared TypeScript interfaces matching Rust models in src-tauri/src/models.rs

export interface UsageBucket {
  provider: IntegrationProvider;
  key: string;
  label: string;
  utilization: number;
  resets_at: string | null;
  sort_order?: number;
}

export interface UsageProviderError {
  provider: IntegrationProvider;
  message: string;
}

export interface ProviderCredits {
  provider: IntegrationProvider;
  balance: string | null;
}

export interface UsageProviderError {
  provider: IntegrationProvider;
  message: string;
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

export type RangeType = "1h" | "24h" | "7d" | "30d";
export type CodexLiveRange = "1h" | "6h" | "12h" | "24h";

export type TrendType = "up" | "down" | "flat" | "unknown";

export type BreakdownMode = "hosts" | "projects" | "sessions";

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
  brevityEnabled: boolean;
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

export interface LearnedRule {
  name: string;
  domain: string | null;
  confidence: number;
  observation_count: number;
  file_path: string;
  created_at: string;
  updated_at: string;
  state: string;
  project: string | null;
  is_anti_pattern: boolean;
  source: string | null;
	content: string | null;
  provider_scope: IntegrationProvider[];
}

export interface RunPhase {
	name: string;
	status: string;
	duration_ms: number | null;
	findings_count: number;
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

export type AnalyticsTab = "now" | "trends" | "charts" | "context";

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
	reason: string | null;
	delivered: boolean;
	indexedBytes: number;
	returnedBytes: number;
	inputBytes: number;
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

// Plugin manager types

export type PluginsTab = "installed" | "browse" | "marketplaces" | "updates";

export interface InstalledPlugin {
	provider: IntegrationProvider;
	plugin_id: string;
	marketplace_path: string | null;
	name: string;
	marketplace: string;
	version: string;
	scope: string;
	project_path: string | null;
	enabled: boolean;
	description: string | null;
	author: string | null;
	installed_at: string;
	last_updated: string;
	git_commit_sha: string | null;
}

export interface MarketplacePlugin {
	provider: IntegrationProvider;
	plugin_id: string;
	marketplace_path: string | null;
	name: string;
	description: string | null;
	version: string;
	author: string | null;
	category: string | null;
	source_path: string;
	installed: boolean;
	enabled: boolean;
	install_url: string | null;
}

export interface Marketplace {
	provider: IntegrationProvider;
	name: string;
	source_type: string;
	repo: string;
	install_location: string;
	last_updated: string | null;
	plugins: MarketplacePlugin[];
}

export interface PluginUpdate {
	provider: IntegrationProvider;
	name: string;
	marketplace: string;
	scope: string;
	project_path: string | null;
	current_version: string;
	available_version: string;
}

export interface UpdateCheckResult {
	plugin_updates: PluginUpdate[];
	last_checked: string | null;
	next_check: string | null;
}

export interface BulkUpdateProgress {
	total: number;
	completed: number;
	current_plugin: string | null;
	results: BulkUpdateItem[];
}

export interface BulkUpdateItem {
	plugin_key: string;
	name: string;
	status: string;
	error: string | null;
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
