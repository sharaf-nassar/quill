// Browser IPC fixtures — realistic sample data returned by the mock Tauri layer
// (see installBrowserMock.ts) so the app renders fully in a plain browser with no
// Rust backend. Dev-only; never bundled into production. Values are deterministic
// (no Math.random at build) so design screenshots stay stable across reloads.

import type {
  BucketStats,
  CodeStats,
  CodeStatsHistoryPoint,
  ContextPreservationStatus,
  ContextSavingsAnalytics,
  DataPoint,
  HookBreakdown,
  HostBreakdown,
  IntegrationFeatures,
  LearnedRule,
  LearningRun,
  LearningSettings,
  LlmRuntimeStats,
  ProjectBreakdown,
  ProjectTokensRaw,
  ProviderStatus,
  RestartStatus,
  RuntimeSettings,
  SearchFacets,
  SearchResults,
  SessionBreakdown,
  SessionStatsRaw,
  SkillBreakdown,
  ToolCount,
  TokenDataPoint,
  TokenStats,
  UsageData,
} from "../types";

const now = Date.now();
const M = 60_000;
const H = 3_600_000;
const D = 24 * H;
const iso = (msAgo: number) => new Date(now - msAgo).toISOString();
const isoIn = (msAhead: number) => new Date(now + msAhead).toISOString();

// --- Integrations (these gate whether the app renders the dashboard) ----------

const providerStatuses: ProviderStatus[] = [
  {
    provider: "claude",
    detectedCli: true,
    detectedHome: true,
    enabled: true,
    setupState: "installed",
    userHasMadeChoice: true,
    lastError: null,
    lastVerifiedAt: iso(45 * 1000),
  },
  {
    provider: "codex",
    detectedCli: true,
    detectedHome: true,
    enabled: true,
    setupState: "installed",
    userHasMadeChoice: true,
    lastError: null,
    lastVerifiedAt: iso(90 * 1000),
  },
  {
    provider: "mini_max",
    detectedCli: false,
    detectedHome: false,
    enabled: false,
    setupState: "not_installed",
    userHasMadeChoice: false,
    lastError: null,
    lastVerifiedAt: null,
  },
];

const contextPreservation: ContextPreservationStatus = {
  enabled: true,
  hasContextSavingsEvents: true,
};

const integrationFeatures: IntegrationFeatures = {
  contextPreservation: true,
  activityTracking: true,
  contextTelemetry: true,
  brevity: false,
};

const runtimeSettings: RuntimeSettings = {
  liveUsageEnabled: true,
  liveUsageIntervalSeconds: 180,
  pluginUpdatesEnabled: true,
  pluginUpdatesIntervalHours: 6,
  ruleWatcherEnabled: true,
  alwaysOnTop: true,
  crashReportingEnabled: false,
};

const learningSettings: LearningSettings = {
  enabled: true,
  trigger_mode: "periodic",
  periodic_minutes: 120,
  min_observations: 25,
  min_confidence: 0.6,
};

// --- Live usage (utilization is a 0–100 percentage; thresholds 50 / 80) -------

const usageData: UsageData = {
  buckets: [
    { provider: "claude", key: "claude_5h", label: "Sonnet · 5h", utilization: 34, resets_at: isoIn(2 * H + 14 * M), sort_order: 0 },
    { provider: "claude", key: "claude_week", label: "Weekly", utilization: 68, resets_at: isoIn(3 * D), sort_order: 1 },
    { provider: "codex", key: "codex_5h", label: "Codex · 5h", utilization: 86, resets_at: isoIn(48 * M), sort_order: 2 },
    { provider: "codex", key: "codex_week", label: "Codex · Weekly", utilization: 52, resets_at: isoIn(4 * D), sort_order: 3 },
  ],
  provider_errors: [],
  provider_credits: [{ provider: "codex", balance: "$4.20" }],
  error: null,
};

function usageHistory(): DataPoint[] {
  const pts: DataPoint[] = [];
  for (let i = 47; i >= 0; i--) {
    pts.push({ timestamp: iso(i * H), utilization: 25 + ((i * 41) % 60) });
  }
  return pts;
}

const usageStats: BucketStats[] = usageData.buckets.map((b) => ({
  provider: b.provider,
  key: b.key,
  label: b.label,
  current: b.utilization,
  avg: Math.max(10, b.utilization - 12),
  max: Math.min(100, b.utilization + 9),
  min: Math.max(0, b.utilization - 22),
  time_above_80: b.utilization >= 80 ? 3 : 0,
  trend: b.utilization >= 80 ? "up" : "flat",
  sample_count: 96,
}));

// --- Tokens -------------------------------------------------------------------

function tokenHistory(): TokenDataPoint[] {
  const pts: TokenDataPoint[] = [];
  for (let i = 47; i >= 0; i--) {
    const input = 8_000 + ((i * 37) % 5_000);
    const output = 3_000 + ((i * 53) % 2_500);
    const cacheCreate = 1_500 + ((i * 17) % 1_200);
    const cacheRead = 12_000 + ((i * 91) % 9_000);
    pts.push({
      timestamp: iso(i * H),
      input_tokens: input,
      output_tokens: output,
      cache_creation_input_tokens: cacheCreate,
      cache_read_input_tokens: cacheRead,
      total_tokens: input + output + cacheCreate + cacheRead,
    });
  }
  return pts;
}

const tokenStats: TokenStats = {
  total_input: 412_900,
  total_output: 158_300,
  total_cache_creation: 74_500,
  total_cache_read: 612_400,
  total_tokens: 1_258_100,
  turn_count: 1_284,
  avg_input_per_turn: 321,
  avg_output_per_turn: 123,
};

// --- Code changes -------------------------------------------------------------

const codeStats: CodeStats = {
  lines_added: 9_842,
  lines_removed: 4_113,
  net_change: 5_729,
  session_count: 96,
  avg_per_session: 145,
  by_language: [
    { language: "TypeScript", lines: 6_120, percentage: 62 },
    { language: "Rust", lines: 2_540, percentage: 26 },
    { language: "CSS", lines: 820, percentage: 8 },
    { language: "Python", lines: 362, percentage: 4 },
  ],
};

function codeHistory(): CodeStatsHistoryPoint[] {
  const pts: CodeStatsHistoryPoint[] = [];
  for (let i = 13; i >= 0; i--) {
    const added = 200 + ((i * 47) % 600);
    const removed = 80 + ((i * 31) % 300);
    pts.push({ timestamp: iso(i * D), lines_added: added, lines_removed: removed, total_changed: added + removed });
  }
  return pts;
}

// --- Breakdowns ---------------------------------------------------------------

const hostBreakdown: HostBreakdown[] = [
  { hostname: "mbp.local", total_tokens: 824_300, turn_count: 842, last_active: iso(6 * M) },
  { hostname: "devbox", total_tokens: 318_900, turn_count: 311, last_active: iso(3 * H) },
  { hostname: "ci-runner-3", total_tokens: 114_900, turn_count: 131, last_active: iso(2 * D) },
];

const projectBreakdown: ProjectBreakdown[] = [
  { project: "quill", hostname: "mbp.local", total_tokens: 612_400, turn_count: 588, session_count: 41, last_active: iso(6 * M) },
  { project: "stable-api", hostname: "mbp.local", total_tokens: 281_200, turn_count: 264, session_count: 22, last_active: iso(5 * H) },
  { project: "marketing-site", hostname: "devbox", total_tokens: 98_700, turn_count: 96, session_count: 9, last_active: iso(28 * H) },
];

const sessionBreakdown: SessionBreakdown[] = [
  { provider: "claude", session_id: "a1b2c3d4", hostname: "mbp.local", total_tokens: 142_900, turn_count: 96, first_seen: iso(23 * H), last_active: iso(4 * M), project: "quill", has_subagents: true, subagent_count: 3 },
  { provider: "codex", session_id: "e5f6a7b8", hostname: "mbp.local", total_tokens: 88_400, turn_count: 71, first_seen: iso(20 * H), last_active: iso(2 * H), project: "stable-api", has_subagents: false, subagent_count: 0 },
  { provider: "claude", session_id: "c9d0e1f2", hostname: "devbox", total_tokens: 51_200, turn_count: 44, first_seen: iso(2 * D), last_active: iso(28 * H), project: "marketing-site", has_subagents: false, subagent_count: 0 },
];

const skillBreakdown: SkillBreakdown[] = [
  { skill_name: "impeccable", total_count: 142, claude_count: 120, codex_count: 22, project_count: 3, last_used: iso(12 * M) },
  { skill_name: "find-docs", total_count: 88, claude_count: 60, codex_count: 28, project_count: 5, last_used: iso(4 * H) },
  { skill_name: "deep-research", total_count: 31, claude_count: 31, codex_count: 0, project_count: 2, last_used: iso(2 * D) },
];

const hookBreakdown: HookBreakdown[] = [
  { hook_identity: "quill:context-router", hook_event: "PreToolUse", tool_name: "Bash", is_quill: true, codex_count: 41, claude_count: 380, total_count: 421, last_fired_at: iso(3 * M) },
  { hook_identity: "quill:continuity", hook_event: "SessionStart", tool_name: null, is_quill: true, codex_count: 12, claude_count: 96, total_count: 108, last_fired_at: iso(45 * M) },
  { hook_identity: "commit_message_validator.py", hook_event: "PreToolUse", tool_name: "Bash", is_quill: false, codex_count: 0, claude_count: 64, total_count: 64, last_fired_at: iso(5 * H) },
];

const projectTokens: ProjectTokensRaw[] = projectBreakdown.map((p) => ({
  project: p.project,
  total_tokens: p.total_tokens,
  session_count: p.session_count,
}));

// --- Stats --------------------------------------------------------------------

const sessionStats: SessionStatsRaw = {
  avg_duration_seconds: 2_640,
  avg_tokens: 13_106,
  session_count: 96,
  total_tokens: 1_258_100,
};

const llmRuntimeStats: LlmRuntimeStats = {
  total_runtime_secs: 184_920,
  turn_count: 1_284,
  session_count: 96,
  avg_per_turn_secs: 144,
  sparkline: [120, 138, 110, 152, 144, 168, 131, 149, 158, 142, 137, 162],
};

const topTools: ToolCount[] = [
  { tool_name: "Bash", count: 1_842 },
  { tool_name: "Edit", count: 1_204 },
  { tool_name: "Read", count: 990 },
  { tool_name: "Grep", count: 612 },
];

// --- Context savings ----------------------------------------------------------

const contextSavings: ContextSavingsAnalytics = {
  range: "24h",
  generatedAt: iso(0),
  summary: {
    eventCount: 312,
    routerEventCount: 188,
    continuityEventCount: 124,
    indexedBytes: 4_812_000,
    returnedBytes: 1_204_000,
    inputBytes: 6_120_000,
    tokensIndexedEst: 1_203_000,
    tokensReturnedEst: 301_000,
    tokensSavedEst: 902_000,
    tokensPreservedEst: 588_000,
    tokensPreserved: 588_000,
    tokensRetrieved: 301_000,
    tokensRouting: 113_000,
    retentionRatio: 0.25,
  },
  timeSeries: Array.from({ length: 24 }, (_unused, idx) => {
    const i = 23 - idx;
    const indexed = 120_000 + ((i * 7919) % 80_000);
    const returned = 30_000 + ((i * 5003) % 24_000);
    return {
      timestamp: iso(i * H),
      eventCount: 8 + ((i * 13) % 12),
      routerEventCount: 5 + ((i * 7) % 8),
      continuityEventCount: 3 + ((i * 5) % 6),
      indexedBytes: indexed * 4,
      returnedBytes: returned * 4,
      inputBytes: indexed * 5,
      tokensIndexedEst: indexed,
      tokensReturnedEst: returned,
      tokensSavedEst: indexed - returned,
      tokensPreservedEst: Math.round(indexed * 0.6),
    };
  }),
  breakdowns: [
    { provider: "claude", eventType: "capture.index", source: "web_fetch", eventCount: 96, indexedBytes: 2_410_000, returnedBytes: 0, inputBytes: 2_410_000, tokensIndexedEst: 602_000, tokensReturnedEst: 0, tokensSavedEst: 602_000, tokensPreservedEst: 410_000, estimateConfidence: "high" },
    { provider: "claude", eventType: "router.deny", source: "bash", eventCount: 142, indexedBytes: 1_802_000, returnedBytes: 980_000, inputBytes: 2_900_000, tokensIndexedEst: 451_000, tokensReturnedEst: 245_000, tokensSavedEst: 206_000, tokensPreservedEst: 132_000, estimateConfidence: "medium" },
    { provider: "codex", eventType: "source.return", source: "context_store", eventCount: 74, indexedBytes: 600_000, returnedBytes: 224_000, inputBytes: 810_000, tokensIndexedEst: 150_000, tokensReturnedEst: 56_000, tokensSavedEst: 94_000, tokensPreservedEst: 46_000, estimateConfidence: "exact" },
  ],
  recentEvents: [
    { eventId: "ev1", provider: "claude", sessionId: "a1b2c3d4", hostname: "mbp.local", cwd: "/home/mamba/work/quill", timestamp: iso(3 * M), eventType: "capture.index", source: "web_fetch", decision: null, category: "capture", reason: null, delivered: true, indexedBytes: 184_000, returnedBytes: null, inputBytes: 184_000, tokensIndexedEst: 46_000, tokensReturnedEst: null, tokensSavedEst: 46_000, tokensPreservedEst: 31_000, estimateMethod: "tiktoken", estimateConfidence: "high", sourceRef: "src://web/abc", snapshotRef: null, createdAt: iso(3 * M) },
    { eventId: "ev2", provider: "claude", sessionId: "a1b2c3d4", hostname: "mbp.local", cwd: "/home/mamba/work/quill", timestamp: iso(11 * M), eventType: "source.return", source: "context_store", decision: "return", category: "source", reason: null, delivered: true, indexedBytes: null, returnedBytes: 42_000, inputBytes: 42_000, tokensIndexedEst: null, tokensReturnedEst: 10_500, tokensSavedEst: null, tokensPreservedEst: null, estimateMethod: "tiktoken", estimateConfidence: "exact", sourceRef: "src://web/abc", snapshotRef: "snap://1", createdAt: iso(11 * M) },
    { eventId: "ev3", provider: "codex", sessionId: "e5f6a7b8", hostname: "mbp.local", cwd: "/home/mamba/work/stable-api", timestamp: iso(38 * M), eventType: "router.deny", source: "bash", decision: "deny", category: "router", reason: "large_output", delivered: false, indexedBytes: 96_000, returnedBytes: null, inputBytes: 96_000, tokensIndexedEst: 24_000, tokensReturnedEst: null, tokensSavedEst: 24_000, tokensPreservedEst: 16_000, estimateMethod: "bytes/4", estimateConfidence: "medium", sourceRef: null, snapshotRef: null, createdAt: iso(38 * M) },
  ],
};

// --- Learning -----------------------------------------------------------------

const learnedRules: LearnedRule[] = [
  { name: "Prefer rg over grep for code search", domain: "shell", confidence: 0.92, observation_count: 41, file_path: "/rules/rg-over-grep.md", created_at: iso(6 * D), updated_at: iso(2 * H), state: "active", project: null, is_anti_pattern: false, source: "claude", content: null, provider_scope: ["claude", "codex"] },
  { name: "Always run lat check before finishing", domain: "workflow", confidence: 0.81, observation_count: 28, file_path: "/rules/lat-check.md", created_at: iso(4 * D), updated_at: iso(20 * H), state: "active", project: "quill", is_anti_pattern: false, source: "claude", content: null, provider_scope: ["claude"] },
  { name: "Avoid force-push on shared branches", domain: "git", confidence: 0.74, observation_count: 19, file_path: "", created_at: iso(3 * D), updated_at: iso(30 * H), state: "candidate", project: null, is_anti_pattern: true, source: "codex", content: "Discovered: 19 observations of reverted force-pushes.", provider_scope: ["codex"] },
];

const learningRuns: LearningRun[] = [
  { id: 42, trigger_mode: "periodic", observations_analyzed: 184, rules_created: 2, rules_updated: 5, duration_ms: 41_200, status: "complete", error: null, logs: null, created_at: iso(2 * H), phases: [{ name: "collect", status: "complete", duration_ms: 1_200, findings_count: 184 }, { name: "infer", status: "complete", duration_ms: 38_000, findings_count: 7 }], provider_scope: ["claude", "codex"], inference: { total_cost_usd: 0.142, total_duration_ms: 38_000, primary_model: "claude-opus-4-8", call_count: 4, failed_call_count: 0, calls: [] } },
  { id: 41, trigger_mode: "on-demand", observations_analyzed: 96, rules_created: 1, rules_updated: 2, duration_ms: 22_800, status: "complete", error: null, logs: null, created_at: iso(28 * H), phases: null, provider_scope: ["claude"] },
];

const restartStatus: RestartStatus = {
  phase: "Idle",
  instances: [],
  waiting_on: 0,
  elapsed_seconds: 0,
};

const searchResults: SearchResults = { hits: [], total_hits: 0, query_time_ms: 2 };
const searchFacets: SearchFacets = { providers: [], projects: [], hosts: [] };

// --- Command → fixture map ----------------------------------------------------

const fixtures: Record<string, () => unknown> = {
  // integrations / settings
  get_provider_statuses: () => providerStatuses,
  rescan_integrations: () => providerStatuses,
  get_indicator_primary_provider: () => "claude",
  get_context_preservation_status: () => contextPreservation,
  set_context_preservation_enabled: () => contextPreservation,
  get_integration_features: () => integrationFeatures,
  get_runtime_settings: () => runtimeSettings,
  set_runtime_settings: () => runtimeSettings,
  get_learning_settings: () => learningSettings,
  set_learning_settings: () => learningSettings,
  // live usage
  fetch_usage_data: () => usageData,
  get_usage_history: () => usageHistory(),
  get_usage_stats: () => usageStats,
  // tokens
  get_token_history: () => tokenHistory(),
  get_token_stats: () => tokenStats,
  get_token_hostnames: () => ["mbp.local", "devbox", "ci-runner-3"],
  // code
  get_code_stats: () => codeStats,
  get_code_stats_history: () => codeHistory(),
  get_batch_session_code_stats: () => ({}),
  // breakdowns
  get_host_breakdown: () => hostBreakdown,
  get_project_breakdown: () => projectBreakdown,
  get_session_breakdown: () => sessionBreakdown,
  get_skill_breakdown: () => skillBreakdown,
  get_hook_breakdown: () => hookBreakdown,
  get_project_tokens: () => projectTokens,
  get_skill_project_breakdown: () => [],
  get_session_subagent_tree: () => [],
  // stats
  get_session_stats: () => sessionStats,
  get_llm_runtime_stats: () => llmRuntimeStats,
  get_snapshot_count: () => 1_440,
  get_top_tools: () => topTools,
  get_observation_count: () => 184,
  get_unanalyzed_observation_count: () => 12,
  get_observation_sparkline: () => [4, 7, 5, 9, 6, 11, 8, 10, 12, 9, 7, 13],
  // context savings
  get_context_savings_analytics: () => contextSavings,
  // learning
  get_learned_rules: () => learnedRules,
  get_learning_runs: () => learningRuns,
  read_rule_content: () => "# Rule\n\nSample rule content for browser preview.",
  trigger_analysis: () => null,
  promote_learned_rule: () => null,
  delete_learned_rule: () => null,
  submit_rule_feedback: () => null,
  // memory
  get_memory_files: () => [],
  get_optimization_suggestions: () => [],
  get_optimization_runs: () => [],
  get_known_projects: () => [],
  add_custom_project: () => null,
  remove_custom_project: () => null,
  trigger_memory_optimization: () => null,
  // plugins
  get_installed_plugins: () => [],
  get_marketplaces: () => [],
  get_available_updates: () => ({ plugin_updates: [], last_checked: null, next_check: null }),
  check_updates_now: () => ({ plugin_updates: [], last_checked: iso(0), next_check: isoIn(6 * H) }),
  // sessions
  search_sessions: () => searchResults,
  get_search_facets: () => searchFacets,
  get_session_context: () => ({ provider: "claude", messages: [], session_id: "a1b2c3d4", project: "quill" }),
  sync_search_index: () => 0,
  // restart
  get_restart_status: () => restartStatus,
  // release notes / updates
  get_release_notes: () => [],
  // misc no-ops
  set_indicator_primary_provider: () => null,
  set_minimax_api_key: () => null,
  hide_window: () => null,
  quit_app: () => null,
};

let listenerSeq = 1;

/**
 * Mock handler for every Tauri `invoke()` call in the browser. Returns realistic
 * fixtures for known commands, benign defaults for Tauri core/plugin commands
 * (including event listen/unlisten), and `null` for anything unmapped.
 */
export function handleInvoke(cmd: string, _args?: Record<string, unknown>): unknown {
  // Event plugin: let `listen()` resolve with a fake registration; events never fire.
  if (cmd.startsWith("plugin:event|listen")) return listenerSeq++;
  if (cmd.startsWith("plugin:")) return undefined;

  const fixture = fixtures[cmd];
  if (fixture) return fixture();

  if (import.meta.env.DEV) console.debug("[mock] unhandled invoke:", cmd);
  return null;
}
