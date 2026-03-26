// Shared TypeScript interfaces matching Rust models in src-tauri/src/models.rs

export interface UsageBucket {
  label: string;
  utilization: number;
  resets_at: string | null;
}

export interface UsageData {
  buckets: UsageBucket[];
  error: string | null;
}

export interface DataPoint {
  timestamp: string;
  utilization: number;
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

export type TimeMode = "marker" | "dual" | "background";

export type RangeType = "1h" | "24h" | "7d" | "30d";

export type TrendType = "up" | "down" | "flat" | "unknown";

export type BreakdownMode = "hosts" | "projects" | "sessions";

export type SortMode = "relevance" | "recency";

export interface BreakdownSelection {
  type: "host" | "project" | "session";
  key: string;
  firstSeen: string;
  lastActive: string;
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

// Learning system types

export interface LearningSettings {
  enabled: boolean;
  trigger_mode: string;
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

export interface SearchFilters {
  project?: string;
  host?: string;
  role?: "user" | "assistant";
  date_from?: string;
  date_to?: string;
  git_branch?: string;
}

export interface SearchHit {
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
  messages: ContextMessage[];
  session_id: string;
  project: string;
}

// Analytics redesign types

export type AnalyticsTab = "now" | "trends" | "charts";

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
	name: string;
	description: string | null;
	version: string;
	author: string | null;
	category: string | null;
	source_path: string;
	installed: boolean;
}

export interface Marketplace {
	name: string;
	source_type: string;
	repo: string;
	install_location: string;
	last_updated: string | null;
	plugins: MarketplacePlugin[];
}

export interface PluginUpdate {
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
	name: string;
	status: string;
	error: string | null;
}

// Response time types

export interface ResponseTimeStats {
	avg_response_secs: number;
	peak_response_secs: number;
	avg_idle_secs: number;
	sample_count: number;
	sparkline: number[];
}

// Restart feature types

export interface ClaudeInstance {
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
	instances: ClaudeInstance[];
	waiting_on: number;
	elapsed_seconds: number;
}
