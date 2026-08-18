// Browser IPC fixtures — realistic sample data returned by the mock Tauri layer
// (see installBrowserMock.ts) so the app renders fully in a plain browser with no
// Rust backend. Dev-only; never bundled into production. Values are deterministic
// (no Math.random at build) so design screenshots stay stable across reloads.

import { emit } from "@tauri-apps/api/event";
import type {
  ActivitySeriesResponse,
  CodeStats,
  CodeStatsHistoryPoint,
  ContextPreservationStatus,
  ContextSavingsAnalytics,
  HookBreakdown,
  HostBreakdown,
  IntegrationFeatures,
  LearnedRule,
  LearningRun,
  LearningSettings,
  LlmRuntimeStats,
  ModelAnalyticsError,
  ModelAnalyticsErrorCode,
  ModelBackfillStatus,
  ModelIdentity,
  ModelRange,
  ModelUsageOverviewResponse,
  ProjectBreakdown,
  ProviderStatus,
  ProviderTokenSeries,
  ProviderTokenSeriesResponse,
  RetentionAuditRecord,
  RetentionMaintenanceProgress,
  RetentionMaintenanceResult,
  RetentionPolicy,
  RetentionPreview,
  RuntimeSettings,
  SearchFacets,
  SearchResults,
  SessionBreakdown,
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
// Most timestamps mirror the Rust backend's `to_rfc3339()` (zone-designated)
// and are consumed directly via `new Date(...)` — session times, rate-limit
// resets, verification stamps.
const iso = (msAgo: number) => new Date(now - msAgo).toISOString();
const isoIn = (msAhead: number) => new Date(now + msAhead).toISOString();
// `created_at` columns are DB-populated by SQLite `datetime('now')`, which is a
// space-separated naive-UTC string with NO "Z" (e.g. "2026-06-30 12:00:00").
// utils/time.ts#timeAgo appends "Z" only to zone-less values, matching SQLite's
// UTC semantics without corrupting the RFC3339 timestamps used elsewhere.
const sqliteUtc = (msAgo: number) =>
  new Date(now - msAgo).toISOString().replace("T", " ").slice(0, -5);

// --- Range geometry (feature 018 — widget) ------------------------------------
// The widget scopes every band by the same range toggle, including the new 6h
// step, so the fixtures have to answer per-range rather than serving one fixed
// window to everybody.

const RANGE_DURATION_MS: Record<string, number> = {
  "1h": H,
  "2h": 2 * H,
  "6h": 6 * H,
  "12h": 12 * H,
  "24h": D,
  "2d": 2 * D,
  "7d": 7 * D,
  "14d": 14 * D,
  "30d": 30 * D,
};

/** Read the `range` IPC argument, falling back to the app's default window. */
function rangeArg(args?: Record<string, unknown>): string {
  const range = args?.range;
  return typeof range === "string" && range in RANGE_DURATION_MS ? range : "24h";
}

/** Read the `buckets` IPC argument, ignoring anything not a positive integer. */
function bucketsArg(args: Record<string, unknown> | undefined, fallback: number): number {
  const buckets = args?.buckets;
  return typeof buckets === "number" && Number.isInteger(buckets) && buckets > 0
    ? buckets
    : fallback;
}

/** Bucket-start timestamps spanning `range`, oldest first, `count` of them. */
function bucketTimestamps(range: string, count: number): string[] {
  const duration = RANGE_DURATION_MS[range] ?? D;
  const step = count > 1 ? duration / (count - 1) : duration;
  return Array.from({ length: count }, (_, i) => iso(Math.round((count - 1 - i) * step)));
}

/** Seconds covered by one bucket of `count` across `range`. */
function bucketSecs(range: string, count: number): number {
  const duration = RANGE_DURATION_MS[range] ?? D;
  return Math.round(duration / Math.max(1, count - 1) / 1000);
}

/** Nearest-neighbour resample so one hand-drawn shape serves any bucket count. */
function resample(shape: readonly number[], count: number): number[] {
  if (shape.length === 0) return Array.from({ length: count }, () => 0);
  if (count === shape.length) return [...shape];
  if (count <= 1) return [shape[shape.length - 1]];
  return Array.from(
    { length: count },
    (_, i) => shape[Math.round((i * (shape.length - 1)) / (count - 1))],
  );
}

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
    provider: "pi",
    detectedCli: true,
    detectedHome: true,
    enabled: true,
    setupState: "installed",
    userHasMadeChoice: true,
    lastError: null,
    lastVerifiedAt: iso(2 * M),
    piExtensionHealth: {
      state: "alive",
      lastSeen: iso(20 * 1000),
      protocol: "2",
      extensionVersion: "0.2.0",
      minQuillVersion: "0.9.0",
      lastError: null,
      affectedReporters: 0,
      affectedSessions: 0,
      remediation: null,
      lastRecoveredAt: iso(4 * M),
      requiredProtocol: null,
      requiredExtensionVersion: null,
      requiredQuillVersion: null,
    },
  },
  // Enabled but not answering: exercises the widget LIMITS row's SETUP state
  // (paired with the `auth` provider error below), which is the only way to
  // see a provider row that has no live buckets in browser mode.
  {
    provider: "mini_max",
    detectedCli: false,
    detectedHome: true,
    enabled: true,
    setupState: "installed",
    userHasMadeChoice: true,
    lastError: "MiniMax API key was rejected",
    lastVerifiedAt: iso(20 * M),
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
    // Reset already elapsed: the row must render neutral (muted %, slate bar)
    // instead of carrying a bygone window's utilization as a live severity.
    { provider: "claude", key: "weekly_scoped_fable", label: "Fable", utilization: 22, resets_at: iso(35 * M), sort_order: 1 },
    { provider: "codex", key: "codex_5h", label: "Codex · 5h", utilization: 86, resets_at: isoIn(48 * M), sort_order: 2 },
    { provider: "codex", key: "codex_week", label: "Codex · Weekly", utilization: 52, resets_at: isoIn(4 * D), sort_order: 3 },
  ],
  provider_errors: [
    {
      provider: "mini_max",
      kind: "auth",
      message: "MiniMax API key was rejected",
    },
  ],
  provider_credits: [{ provider: "codex", balance: "$4.20" }],
  cpa_accounts: [],
  cpa_pools: [],
  error: null,
};

// --- Tokens -------------------------------------------------------------------

/** Point spacing per range; hour-granular ranges get sub-hour buckets. */
const TOKEN_HISTORY_GEOMETRY: Record<string, { count: number; stepMs: number }> = {
  "1h": { count: 12, stepMs: 5 * M },
  "2h": { count: 25, stepMs: 5 * M },
  "6h": { count: 24, stepMs: 15 * M },
  "12h": { count: 49, stepMs: 15 * M },
  "2d": { count: 49, stepMs: H },
  // Fifteen inclusive daily boundaries cover the 7d window and prior period.
  "14d": { count: 15, stepMs: D },
  "30d": { count: 30, stepMs: D },
};
const DEFAULT_TOKEN_HISTORY = { count: 48, stepMs: H };

function tokenHistory(range: string): TokenDataPoint[] {
  const { count, stepMs } = TOKEN_HISTORY_GEOMETRY[range] ?? DEFAULT_TOKEN_HISTORY;
  const pts: TokenDataPoint[] = [];
  for (let i = count - 1; i >= 0; i--) {
    // Daily points span the displayed window and its comparison period. The
    // current half runs busier and caches better so Usage deltas have evidence;
    // sub-daily ranges keep the flat shape the usage chart is verified against.
    const thisWeek = stepMs >= D && i * stepMs < 7 * D;
    const volume = thisWeek ? 1.18 : 1;
    const cacheShare = thisWeek ? 1.12 : 1;
    const input = Math.round((8_000 + ((i * 37) % 5_000)) * volume);
    const output = Math.round((3_000 + ((i * 53) % 2_500)) * volume);
    const cacheCreate = Math.round((1_500 + ((i * 17) % 1_200)) * volume);
    const cacheRead = Math.round((12_000 + ((i * 91) % 9_000)) * volume * cacheShare);
    pts.push({
      timestamp: iso(i * stepMs),
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

/** The widget's readout grid asks for 8 buckets; hour ranges shrink the step. */
const CODE_HISTORY_GEOMETRY: Record<string, { count: number; stepMs: number }> = {
  "1h": { count: 8, stepMs: 7.5 * M },
  "2h": { count: 17, stepMs: 7.5 * M },
  "6h": { count: 8, stepMs: 45 * M },
  "12h": { count: 17, stepMs: 45 * M },
  "2d": { count: 17, stepMs: 3 * H },
  "14d": { count: 15, stepMs: D },
};
const DEFAULT_CODE_HISTORY = { count: 14, stepMs: D };

function codeHistory(range: string): CodeStatsHistoryPoint[] {
  const { count, stepMs } = CODE_HISTORY_GEOMETRY[range] ?? DEFAULT_CODE_HISTORY;
  const pts: CodeStatsHistoryPoint[] = [];
  for (let i = count - 1; i >= 0; i--) {
    const added = 200 + ((i * 47) % 600);
    const removed = 80 + ((i * 31) % 300);
    pts.push({
      timestamp: iso(i * stepMs),
      lines_added: added,
      lines_removed: removed,
      total_changed: added + removed,
    });
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
  { provider: "claude", session_id: "a1b2c3d4", parent_session_id: null, hostname: "mbp.local", total_tokens: 142_900, turn_count: 96, first_seen: iso(23 * H), last_active: iso(4 * M), ended_at: null, project: "instrumentation-observability-control-plane", active_runtime_secs: 4_823, agent_count: 5, agent_runtime_secs: 4_212, current_turn_runtime_secs: 41, current_turn_runtime_active: true, runtime_as_of_ms: now - 500, active_runtime_rate: 2, observed_agents: [{ agent_id: "agent-opus-a", model_id: "claude-opus-4-6", agent_type: null, runtime_secs: 3_840, runtime_active: true }, { agent_id: "agent-opus-b", model_id: "claude-opus-4-6", agent_type: null, runtime_secs: 272, runtime_active: false }, { agent_id: "agent-sonnet", model_id: "claude-sonnet-4-6", agent_type: null, runtime_secs: null, runtime_active: true }], live_linked_sessions: null, observed_only: false },
  { provider: "pi", session_id: "pi-root", parent_session_id: null, pi_lineage: { kind: "root" }, hostname: "mbp", total_tokens: 12_480, turn_count: 8, first_seen: iso(38 * M), last_active: iso(M / 3), ended_at: null, project: "quill", active_runtime_secs: 1_260, agent_count: 2, agent_runtime_secs: 420, current_turn_runtime_secs: null, current_turn_runtime_active: false, runtime_as_of_ms: now, active_runtime_rate: 2, observed_agents: [{ agent_id: "pi-review", model_id: "claude-opus-5", agent_type: "reviewer", runtime_secs: 240, runtime_active: true }, { agent_id: "pi-research", model_id: "gpt-5.6-sol", agent_type: "researcher", runtime_secs: 180, runtime_active: true }], live_linked_sessions: [], observed_only: false },
  { provider: "codex", session_id: "e5f6a7b8", parent_session_id: null, hostname: "mbp.local", total_tokens: 88_400, turn_count: 71, first_seen: iso(20 * H), last_active: iso(2 * H), ended_at: null, project: "stable-api", active_runtime_secs: 7_260, agent_count: 3, agent_runtime_secs: 5_400, current_turn_runtime_secs: null, current_turn_runtime_active: false, runtime_as_of_ms: now - 2 * H, active_runtime_rate: 0, observed_agents: [], live_linked_sessions: null, observed_only: false },
  { provider: "claude", session_id: "c9d0e1f2", parent_session_id: null, hostname: "devbox", total_tokens: 51_200, turn_count: 44, first_seen: iso(2 * D), last_active: iso(28 * H), ended_at: null, project: "marketing-site", active_runtime_secs: null, agent_count: null, agent_runtime_secs: null, current_turn_runtime_secs: null, current_turn_runtime_active: false, runtime_as_of_ms: null, active_runtime_rate: 0, observed_agents: null, live_linked_sessions: null, observed_only: false },
  { provider: "claude", session_id: "b7c8d9e0", parent_session_id: null, hostname: "mbp.local", total_tokens: 33_800, turn_count: 29, first_seen: iso(130 * D), last_active: iso(128 * D), ended_at: null, project: "quill", active_runtime_secs: 8_400, agent_count: 0, agent_runtime_secs: 0, current_turn_runtime_secs: null, current_turn_runtime_active: false, runtime_as_of_ms: now - 128 * D, active_runtime_rate: 0, observed_agents: [], live_linked_sessions: null, observed_only: false },
  { provider: "codex", session_id: "f1a2b3c4", parent_session_id: null, hostname: "mbp.local", total_tokens: 0, turn_count: 0, first_seen: iso(3 * M), last_active: iso(M), ended_at: null, project: "/home/mamba/work/poe", active_runtime_secs: 155, agent_count: null, agent_runtime_secs: null, current_turn_runtime_secs: 51, current_turn_runtime_active: true, runtime_as_of_ms: now - 250, active_runtime_rate: 2, observed_agents: [{ agent_id: "agent-sol", model_id: "gpt-5.6-sol", agent_type: null, runtime_secs: 86, runtime_active: true }, { agent_id: "agent-terra", model_id: "gpt-5.6-terra", agent_type: null, runtime_secs: 31, runtime_active: false }], live_linked_sessions: null, observed_only: true },
];

const skillBreakdown: SkillBreakdown[] = [
  { skill_name: "impeccable", total_count: 151, claude_count: 120, codex_count: 22, pi_count: 9, project_count: 3, last_used: iso(12 * M) },
  { skill_name: "find-docs", total_count: 93, claude_count: 60, codex_count: 28, pi_count: 5, project_count: 5, last_used: iso(4 * H) },
  { skill_name: "deep-research", total_count: 33, claude_count: 31, codex_count: 0, pi_count: 2, project_count: 2, last_used: iso(2 * D) },
];

const hookBreakdown: HookBreakdown[] = [
  { hook_identity: "quill:context-router", hook_event: "PreToolUse", tool_name: "Bash", is_quill: true, codex_count: 41, claude_count: 380, pi_count: 18, total_count: 439, last_fired_at: iso(3 * M) },
  { hook_identity: "quill:observe.cjs", hook_event: "PreToolUse", tool_name: "Bash", is_quill: true, codex_count: 12, claude_count: 96, pi_count: 7, total_count: 115, last_fired_at: iso(45 * M) },
  { hook_identity: "commit_message_validator.py", hook_event: "PreToolUse", tool_name: "Bash", is_quill: false, codex_count: 0, claude_count: 64, pi_count: 3, total_count: 67, last_fired_at: iso(5 * H) },
];

// --- Stats --------------------------------------------------------------------

/**
 * Runtime answers per range, and its sparkline always sums to the total it
 * reports.
 *
 * The real command derives both from one walk over `session_events`, and
 * `useCodeInsights` relies on that to recover the prior window's active seconds
 * by prorating this sparkline. A shape that did not add up to the total would
 * make the same period read differently depending on which side of the
 * comparison it landed on. Seven buckets match the backend's fixed grid.
 */
const RUNTIME_SHAPE = [0.11, 0.14, 0.12, 0.17, 0.15, 0.18, 0.13];
/** Active LLM seconds per day of window — about 7.3 hours of real work. */
const RUNTIME_SECS_PER_DAY = 26_400;

function llmRuntimeStats(range: string): LlmRuntimeStats {
  const windowMs = RANGE_DURATION_MS[range] ?? D;
  const days = windowMs / D;
  const totalRuntimeSecs = Math.round(days * RUNTIME_SECS_PER_DAY);
  const turnCount = Math.max(1, Math.round(days * 183));
  return {
    total_runtime_secs: totalRuntimeSecs,
    turn_count: turnCount,
    session_count: Math.max(1, Math.round(days * 13.7)),
    avg_per_turn_secs: Math.round(totalRuntimeSecs / turnCount),
    sparkline: RUNTIME_SHAPE.map((share) => Math.round(totalRuntimeSecs * share)),
  };
}

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
    // Category-scoped counts and source retention, so the widget's Context
    // view renders the same coherent story in browser mode that a real
    // backend produces: 103/412 sources reused is the 0.25 ratio below.
    routingEventCount: 188,
    sourcesPreserved: 412,
    sourcesRetrieved: 103,
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
    { provider: "claude", eventType: "mcp.index", source: "web_fetch", eventCount: 96, indexedBytes: 2_410_000, returnedBytes: 0, inputBytes: 2_410_000, tokensIndexedEst: 602_000, tokensReturnedEst: 0, tokensSavedEst: 602_000, tokensPreservedEst: 410_000, estimateConfidence: "high" },
    { provider: "claude", eventType: "router.denial", source: "bash", eventCount: 142, indexedBytes: 1_802_000, returnedBytes: 980_000, inputBytes: 2_900_000, tokensIndexedEst: 451_000, tokensReturnedEst: 245_000, tokensSavedEst: 206_000, tokensPreservedEst: 132_000, estimateConfidence: "medium" },
    { provider: "codex", eventType: "mcp.source_read", source: "context_store", eventCount: 74, indexedBytes: 600_000, returnedBytes: 224_000, inputBytes: 810_000, tokensIndexedEst: 150_000, tokensReturnedEst: 56_000, tokensSavedEst: 94_000, tokensPreservedEst: 46_000, estimateConfidence: "exact" },
  ],
  recentEvents: [
    { eventId: "ev1", provider: "claude", sessionId: "a1b2c3d4", hostname: "mbp.local", cwd: "/home/mamba/work/quill", timestamp: iso(3 * M), eventType: "mcp.index", source: "web_fetch", decision: "indexed", category: "preservation", reason: null, delivered: true, indexedBytes: 184_000, returnedBytes: null, inputBytes: 184_000, tokensIndexedEst: 46_000, tokensReturnedEst: null, tokensSavedEst: 46_000, tokensPreservedEst: 31_000, estimateMethod: "tiktoken", estimateConfidence: "high", sourceRef: "src://web/abc", createdAt: iso(3 * M) },
    { eventId: "ev2", provider: "claude", sessionId: "a1b2c3d4", hostname: "mbp.local", cwd: "/home/mamba/work/quill", timestamp: iso(11 * M), eventType: "mcp.source_read", source: "context_store", decision: "returned", category: "retrieval", reason: null, delivered: true, indexedBytes: null, returnedBytes: 42_000, inputBytes: 42_000, tokensIndexedEst: null, tokensReturnedEst: 10_500, tokensSavedEst: null, tokensPreservedEst: null, estimateMethod: "tiktoken", estimateConfidence: "exact", sourceRef: "src://web/abc", createdAt: iso(11 * M) },
    { eventId: "ev3", provider: "codex", sessionId: "e5f6a7b8", hostname: "mbp.local", cwd: "/home/mamba/work/stable-api", timestamp: iso(38 * M), eventType: "router.denial", source: "bash", decision: "deny", category: "routing", reason: "large_output", delivered: false, indexedBytes: 96_000, returnedBytes: null, inputBytes: 96_000, tokensIndexedEst: 24_000, tokensReturnedEst: null, tokensSavedEst: 0, tokensPreservedEst: 0, estimateMethod: "bytes/4", estimateConfidence: "medium", sourceRef: null, createdAt: iso(38 * M) },
  ],
};

// --- Learning -----------------------------------------------------------------

const learnedRules: LearnedRule[] = [
  { name: "Prefer rg over grep for code search", domain: "shell", confidence: 0.92, observation_count: 41, file_path: "/rules/rg-over-grep.md", created_at: iso(6 * D), updated_at: iso(2 * H), state: "active", project: null, is_anti_pattern: false, source: "claude", content: null, provider_scope: ["claude", "codex"] },
  { name: "Always run lat check before finishing", domain: "workflow", confidence: 0.81, observation_count: 28, file_path: "/rules/lat-check.md", created_at: iso(4 * D), updated_at: iso(20 * H), state: "active", project: "quill", is_anti_pattern: false, source: "claude", content: null, provider_scope: ["claude"] },
  { name: "Avoid force-push on shared branches", domain: "git", confidence: 0.74, observation_count: 19, file_path: "", created_at: iso(3 * D), updated_at: iso(30 * H), state: "candidate", project: null, is_anti_pattern: true, source: "codex", content: "Discovered: 19 observations of reverted force-pushes.", provider_scope: ["codex"] },
];

const learningRuns: LearningRun[] = [
  { id: 42, trigger_mode: "periodic", observations_analyzed: 184, rules_created: 2, rules_updated: 5, duration_ms: 41_200, status: "completed", error: null, logs: null, created_at: sqliteUtc(2 * H), phases: [{ name: "collect", status: "completed", duration_ms: 1_200, findings_count: 184 }, { name: "infer", status: "completed", duration_ms: 38_000, findings_count: 7 }], provider_scope: ["claude", "codex"], inference: { total_cost_usd: 0.142, total_duration_ms: 38_000, primary_model: "claude-opus-4-8", call_count: 4, failed_call_count: 0, calls: [] } },
  { id: 41, trigger_mode: "on-demand", observations_analyzed: 96, rules_created: 1, rules_updated: 2, duration_ms: 22_800, status: "completed", error: null, logs: null, created_at: sqliteUtc(28 * H), phases: null, provider_scope: ["claude"] },
];

const searchResults: SearchResults = { hits: [], total_hits: 0, query_time_ms: 2 };
const searchFacets: SearchFacets = { providers: [], projects: [], hosts: [] };

// --- Session model analytics -------------------------------------------------

type MockModelObservationKind = "turn" | "token";

interface MockModelObservation {
  provider: ProviderStatus["provider"];
  sourceKey: string;
  sourceSuppressed?: boolean;
  sessionId: string;
  observedAt: number;
  modelId: string | null;
  kind: MockModelObservationKind;
  inputTokens: number | null;
  outputTokens: number | null;
  cacheCreationTokens: number | null;
  cacheReadTokens: number | null;
  chainId?: string;
  parentChainId?: string | null;
  agentId?: string | null;
  displayName?: string;
  cwd?: string | null;
  hostname?: string | null;
  /** Simulates deletion after the page snapshot but before lazy detail. */
}

const MODEL_RANGE_MS: Record<ModelRange, number> = {
  "1h": H,
  "6h": 6 * H,
  "24h": D,
  "7d": 7 * D,
  "30d": 30 * D,
};

const modelProviders = new Set(providerStatuses.map(({ provider }) => provider));

// IDs here are opaque sample evidence, not a supported-model catalog. Aggregate
// and selection logic below operates on every identifier present in this array.
const modelObservations: MockModelObservation[] = [
  {
    provider: "claude",
    sourceKey: "claude/model-session-mixed.jsonl",
    sessionId: "model-session-mixed",
    observedAt: now - 52 * M,
    modelId: "shared/model.snapshot",
    kind: "turn",
    inputTokens: 1_200,
    outputTokens: 320,
    cacheCreationTokens: 180,
    cacheReadTokens: 640,
  },
  {
    provider: "claude",
    sourceKey: "claude/model-session-mixed.jsonl",
    sessionId: "model-session-mixed",
    observedAt: now - 24 * M,
    modelId: "future/model.snapshot-2099",
    kind: "turn",
    inputTokens: 880,
    outputTokens: 410,
    cacheCreationTokens: 120,
    cacheReadTokens: 390,
  },
  {
    provider: "claude",
    sourceKey: "claude/model-session-shared.jsonl",
    sessionId: "model-session-shared",
    observedAt: now - 8 * M,
    modelId: "shared/model.snapshot",
    kind: "turn",
    inputTokens: 760,
    outputTokens: 190,
    cacheCreationTokens: 90,
    cacheReadTokens: 510,
  },
  {
    provider: "claude",
    sourceKey: "claude/model-session-archive.jsonl",
    sessionId: "model-session-archive",
    observedAt: now - 2 * D,
    modelId: "archive/model.case-Sensitive",
    kind: "turn",
    inputTokens: 640,
    outputTokens: 150,
    cacheCreationTokens: null,
    cacheReadTokens: 280,
  },
  {
    provider: "codex",
    sourceKey: "codex/codex-model-context.jsonl",
    sessionId: "codex-model-context",
    observedAt: now - 41 * M,
    modelId: "shared/model.snapshot",
    kind: "turn",
    inputTokens: null,
    outputTokens: null,
    cacheCreationTokens: null,
    cacheReadTokens: null,
  },
  {
    provider: "codex",
    sourceKey: "codex/codex-model-context.jsonl",
    sessionId: "codex-model-context",
    observedAt: now - 38 * M,
    modelId: null,
    kind: "token",
    inputTokens: 4_200,
    outputTokens: 1_100,
    cacheCreationTokens: null,
    cacheReadTokens: 2_300,
  },
  {
    provider: "codex",
    sourceKey: "codex/codex-model-older.jsonl",
    sessionId: "codex-model-older",
    observedAt: now - 6 * H,
    modelId: "gpt/next.preview",
    kind: "turn",
    inputTokens: null,
    outputTokens: null,
    cacheCreationTokens: null,
    cacheReadTokens: null,
  },
  {
    provider: "codex",
    sourceKey: "codex/codex-model-older.jsonl",
    sessionId: "codex-model-older",
    observedAt: now - 6 * H + M,
    modelId: null,
    kind: "token",
    inputTokens: 3_400,
    outputTokens: 880,
    cacheCreationTokens: null,
    cacheReadTokens: 1_720,
  },
  // These records bracket the 1h interval without entering it. The earlier
  // token still makes MiniMax an all-unattributed provider in the 24h range.
  {
    provider: "mini_max",
    sourceKey: "mini_max/bracketed-empty-session.jsonl",
    sessionId: "bracketed-empty-session",
    observedAt: now - 70 * M,
    modelId: null,
    kind: "token",
    inputTokens: 2_900,
    outputTokens: 760,
    cacheCreationTokens: null,
    cacheReadTokens: 1_540,
  },
  {
    provider: "mini_max",
    sourceKey: "mini_max/bracketed-empty-session.jsonl",
    sessionId: "bracketed-empty-session",
    observedAt: now + 5 * M,
    modelId: null,
    kind: "turn",
    inputTokens: null,
    outputTokens: null,
    cacheCreationTokens: null,
    cacheReadTokens: null,
  },
  // This retained file was explicitly deleted from analytics. Its opaque model
  // evidence must not affect global scope, provider inventory, rows, or history.
  {
    provider: "claude",
    sourceKey: "claude/suppressed-deleted-session.jsonl",
    sourceSuppressed: true,
    sessionId: "suppressed-deleted-session",
    observedAt: now - 12 * M,
    modelId: "suppressed/future.model-2100",
    kind: "turn",
    inputTokens: 99_000,
    outputTokens: 9_000,
    cacheCreationTokens: 4_000,
    cacheReadTokens: 18_000,
  },
  {
    provider: "claude",
    sourceKey: "claude/suppressed-deleted-session.jsonl",
    sessionId: "suppressed-deleted-session",
    observedAt: now - 11 * M,
    modelId: null,
    kind: "token",
    inputTokens: 14_000,
    outputTokens: 1_400,
    cacheCreationTokens: null,
    cacheReadTokens: 2_600,
  },
  // One chain-rich session exercises turn-only adjacency. The parent repeats a
  // model, crosses an explicit gap, and later makes two real switches. An
  // interleaved subagent remains independent, and its token-only unattributed
  // observation neither renders a segment nor resets adjacency.
  {
    provider: "claude",
    sourceKey: "claude/model-session-mixed.jsonl",
    sessionId: "model-session-mixed",
    observedAt: now - 50 * M,
    modelId: "shared/model.snapshot",
    kind: "turn",
    inputTokens: 110,
    outputTokens: 0,
    cacheCreationTokens: 0,
    cacheReadTokens: 0,
    displayName: "Model routing investigation",
    cwd: "/workspace/quill",
    hostname: "glass-cockpit.local",
  },
  {
    provider: "claude",
    sourceKey: "claude/model-session-mixed.jsonl",
    sessionId: "model-session-mixed",
    observedAt: now - 40 * M,
    modelId: null,
    kind: "turn",
    inputTokens: 90,
    outputTokens: 0,
    cacheCreationTokens: 0,
    cacheReadTokens: 0,
    displayName: "Model routing investigation",
    cwd: "/workspace/quill",
    hostname: "glass-cockpit.local",
  },
  {
    provider: "claude",
    sourceKey: "claude/model-session-mixed.jsonl",
    sessionId: "model-session-mixed",
    observedAt: now - 18 * M,
    modelId: "tie/\ud83d\ude00",
    kind: "turn",
    inputTokens: 3_100,
    outputTokens: 0,
    cacheCreationTokens: 0,
    cacheReadTokens: 0,
    displayName: "Model routing investigation",
    cwd: "/workspace/quill",
    hostname: "glass-cockpit.local",
  },
  {
    provider: "claude",
    sourceKey: "claude/model-session-mixed.jsonl",
    sessionId: "model-session-mixed",
    observedAt: now - 17 * M,
    modelId: "tie/\u03a9",
    kind: "turn",
    inputTokens: 3_100,
    outputTokens: 0,
    cacheCreationTokens: 0,
    cacheReadTokens: 0,
    displayName: "Model routing investigation",
    cwd: "/workspace/quill",
    hostname: "glass-cockpit.local",
  },
  {
    provider: "claude",
    sourceKey: "claude/model-session-mixed.jsonl",
    sessionId: "model-session-mixed",
    observedAt: now - 47 * M,
    modelId: "future/model.snapshot-2099",
    kind: "turn",
    inputTokens: 150,
    outputTokens: 0,
    cacheCreationTokens: 0,
    cacheReadTokens: 0,
    chainId: "agent-routing-a",
    parentChainId: "model-session-mixed",
    agentId: "agent-routing-a",
    displayName: "Model routing investigation",
    cwd: "/workspace/quill",
    hostname: "glass-cockpit.local",
  },
  {
    provider: "claude",
    sourceKey: "claude/model-session-mixed.jsonl",
    sessionId: "model-session-mixed",
    observedAt: now - 39 * M,
    modelId: "future/model.snapshot-2099",
    kind: "turn",
    inputTokens: 180,
    outputTokens: 0,
    cacheCreationTokens: 0,
    cacheReadTokens: 0,
    chainId: "agent-routing-a",
    parentChainId: "model-session-mixed",
    agentId: "agent-routing-a",
    displayName: "Model routing investigation",
    cwd: "/workspace/quill",
    hostname: "glass-cockpit.local",
  },
  {
    provider: "claude",
    sourceKey: "claude/model-session-mixed.jsonl",
    sessionId: "model-session-mixed",
    observedAt: now - 36 * M,
    modelId: null,
    kind: "token",
    inputTokens: 260,
    outputTokens: 0,
    cacheCreationTokens: 0,
    cacheReadTokens: 0,
    chainId: "agent-routing-a",
    parentChainId: "model-session-mixed",
    agentId: "agent-routing-a",
    displayName: "Model routing investigation",
    cwd: "/workspace/quill",
    hostname: "glass-cockpit.local",
  },
  {
    provider: "claude",
    sourceKey: "claude/model-session-mixed.jsonl",
    sessionId: "model-session-mixed",
    observedAt: now - 29 * M,
    modelId: "shared/model.snapshot",
    kind: "turn",
    inputTokens: 200,
    outputTokens: 0,
    cacheCreationTokens: 0,
    cacheReadTokens: 0,
    chainId: "agent-routing-a",
    parentChainId: "model-session-mixed",
    agentId: "agent-routing-a",
    displayName: "Model routing investigation",
    cwd: "/workspace/quill",
    hostname: "glass-cockpit.local",
  },
  {
    provider: "claude",
    sourceKey: "claude/model-session-mixed.jsonl",
    sessionId: "model-session-mixed",
    observedAt: now - 28 * M,
    modelId: null,
    kind: "turn",
    inputTokens: 40,
    outputTokens: 0,
    cacheCreationTokens: 0,
    cacheReadTokens: 0,
    chainId: "agent-routing-a",
    parentChainId: "model-session-mixed",
    agentId: "agent-routing-a",
    displayName: "Model routing investigation",
    cwd: "/workspace/quill",
    hostname: "glass-cockpit.local",
  },
  {
    provider: "claude",
    sourceKey: "claude/model-session-mixed.jsonl",
    sessionId: "model-session-mixed",
    observedAt: now - 27 * M,
    modelId: "shared/model.snapshot",
    kind: "turn",
    inputTokens: 220,
    outputTokens: 0,
    cacheCreationTokens: 0,
    cacheReadTokens: 0,
    chainId: "agent-routing-a",
    parentChainId: "model-session-mixed",
    agentId: "agent-routing-a",
    displayName: "Model routing investigation",
    cwd: "/workspace/quill",
    hostname: "glass-cockpit.local",
  },
];

type ModelFixtureScenario =
  | "pending"
  | "running"
  | "complete"
  | "partial-sources"
  | "partial-roots"
  | "failed"
  | "filter-empty"
  | "no-sessions"
  | "no-model-evidence";

type ModelFixtureFailure =
  | "overview"
  | "sessions"
  | "detail"
  | "retry"
  | "all";

const MODEL_FIXTURE_SCENARIOS = new Set<ModelFixtureScenario>([
  "pending",
  "running",
  "complete",
  "partial-sources",
  "partial-roots",
  "failed",
  "filter-empty",
  "no-sessions",
  "no-model-evidence",
]);

const MODEL_FIXTURE_FAILURES = new Set<ModelFixtureFailure>([
  "overview",
  "sessions",
  "detail",
  "retry",
  "all",
]);

const completeModelBackfill: ModelBackfillStatus = {
  generation: 3,
  trigger: "reconcile",
  status: "complete",
  totalRoots: 2,
  completedRoots: 2,
  failedRoots: 0,
  inventoryComplete: true,
  totalSources: 29,
  processedSources: 28,
  failedSources: 0,
  skippedSources: 1,
  remainingSources: 0,
  observationsWritten: 41,
  startedAt: iso(8 * M),
  updatedAt: iso(5 * M),
  finishedAt: iso(5 * M),
  lastError: null,
};

const modelBackfillFixtures: Record<
  ModelFixtureScenario,
  ModelBackfillStatus
> = {
  pending: {
    generation: 1,
    trigger: "migration",
    status: "pending",
    totalRoots: 0,
    completedRoots: 0,
    failedRoots: 0,
    inventoryComplete: false,
    totalSources: 0,
    processedSources: 0,
    failedSources: 0,
    skippedSources: 0,
    remainingSources: 0,
    observationsWritten: 0,
    startedAt: null,
    updatedAt: iso(2 * M),
    finishedAt: null,
    lastError: null,
  },
  running: {
    generation: 2,
    trigger: "startup_resume",
    status: "running",
    totalRoots: 2,
    completedRoots: 2,
    failedRoots: 0,
    inventoryComplete: false,
    totalSources: 6,
    processedSources: 3,
    failedSources: 0,
    skippedSources: 1,
    remainingSources: 2,
    observationsWritten: 5,
    startedAt: iso(4 * M),
    updatedAt: iso(20 * 1_000),
    finishedAt: null,
    lastError: null,
  },
  complete: completeModelBackfill,
  "partial-sources": {
    generation: 4,
    trigger: "retry",
    status: "partial",
    totalRoots: 2,
    completedRoots: 2,
    failedRoots: 0,
    inventoryComplete: true,
    totalSources: 6,
    processedSources: 4,
    failedSources: 1,
    skippedSources: 1,
    remainingSources: 0,
    observationsWritten: 6,
    startedAt: iso(12 * M),
    updatedAt: iso(9 * M),
    finishedAt: iso(9 * M),
    lastError: "1 retained source could not be read.",
  },
  "partial-roots": {
    generation: 5,
    trigger: "retry",
    status: "partial",
    totalRoots: 2,
    completedRoots: 1,
    failedRoots: 1,
    inventoryComplete: false,
    totalSources: 4,
    processedSources: 3,
    failedSources: 0,
    skippedSources: 1,
    remainingSources: 0,
    observationsWritten: 5,
    startedAt: iso(16 * M),
    updatedAt: iso(13 * M),
    finishedAt: iso(13 * M),
    lastError: "1 provider history root could not be enumerated.",
  },
  failed: {
    generation: 6,
    trigger: "retry",
    status: "failed",
    totalRoots: 2,
    completedRoots: 0,
    failedRoots: 2,
    inventoryComplete: false,
    totalSources: 0,
    processedSources: 0,
    failedSources: 0,
    skippedSources: 0,
    remainingSources: 0,
    observationsWritten: 0,
    startedAt: iso(20 * M),
    updatedAt: iso(19 * M),
    finishedAt: iso(19 * M),
    lastError: "Retained history roots could not be enumerated.",
  },
  "filter-empty": {
    ...completeModelBackfill,
    totalSources: 1,
    processedSources: 1,
    skippedSources: 0,
    observationsWritten: 1,
  },
  "no-sessions": {
    ...completeModelBackfill,
    totalSources: 0,
    processedSources: 0,
    skippedSources: 0,
    observationsWritten: 0,
  },
  "no-model-evidence": {
    ...completeModelBackfill,
    totalSources: 1,
    processedSources: 1,
    skippedSources: 0,
    observationsWritten: 1,
  },
};

interface ModelBackfillFixtureOverride {
  scenario: ModelFixtureScenario;
  status: ModelBackfillStatus;
}

let modelBackfillFixtureOverride: ModelBackfillFixtureOverride | null = null;

function synchronizeModelBackfillFixtureScenario(
  scenario: ModelFixtureScenario,
): void {
  if (
    modelBackfillFixtureOverride !== null &&
    modelBackfillFixtureOverride.scenario !== scenario
  ) {
    modelBackfillFixtureOverride = null;
  }
}

function getModelBackfillFixture(
  scenario: ModelFixtureScenario,
): ModelBackfillStatus {
  synchronizeModelBackfillFixtureScenario(scenario);
  return modelBackfillFixtureOverride?.status ?? modelBackfillFixtures[scenario];
}

// Browser-demo controls stay outside IPC payloads so production command
// contracts remain exact. Example:
// `?modelFixture=partial-sources&modelFailure=history`.
const warnedInvalidModelFixtureControls = new Set<string>();

function rejectInvalidModelFixtureControl(
  name: "modelFixture" | "modelFailure",
  value: string,
): never {
  const warningKey = JSON.stringify([name, value]);
  if (!warnedInvalidModelFixtureControls.has(warningKey)) {
    warnedInvalidModelFixtureControls.add(warningKey);
    console.warn(`[mock] invalid ${name} browser control:`, value);
  }
  return rejectModelAnalytics(
    "storage_error",
    `Browser model analytics control ${name} is invalid.`,
  );
}

function readModelFixtureScenario(): ModelFixtureScenario {
  if (typeof window === "undefined") return "pending";
  const requested = new URLSearchParams(window.location.search).get(
    "modelFixture",
  );
  if (requested === null || requested.length === 0) {
    synchronizeModelBackfillFixtureScenario("pending");
    return "pending";
  }
  if (MODEL_FIXTURE_SCENARIOS.has(requested as ModelFixtureScenario)) {
    const scenario = requested as ModelFixtureScenario;
    synchronizeModelBackfillFixtureScenario(scenario);
    return scenario;
  }
  return rejectInvalidModelFixtureControl("modelFixture", requested);
}

function readModelFixtureFailure(): ModelFixtureFailure | null {
  if (typeof window === "undefined") return null;
  const requested = new URLSearchParams(window.location.search).get(
    "modelFailure",
  );
  if (requested === null || requested.length === 0) return null;
  if (MODEL_FIXTURE_FAILURES.has(requested as ModelFixtureFailure)) {
    return requested as ModelFixtureFailure;
  }
  return rejectInvalidModelFixtureControl("modelFailure", requested);
}

function rejectModelAnalytics(
  code: ModelAnalyticsErrorCode,
  message: string,
): never {
  throw { code, message } satisfies ModelAnalyticsError;
}

function rejectRequestedModelFixture(
  request: Exclude<ModelFixtureFailure, "all">,
): void {
  const failure = readModelFixtureFailure();
  if (failure === request || failure === "all") {
    rejectModelAnalytics(
      "storage_error",
      "Model analytics fixture request failed. Retry this section.",
    );
  }
}

function readModelRange(args: Record<string, unknown> | undefined): ModelRange {
  const range = args?.range;
  if (
    typeof range !== "string" ||
    !Object.prototype.hasOwnProperty.call(MODEL_RANGE_MS, range)
  ) {
    return rejectModelAnalytics(
      "invalid_range",
      "Range must be one of 1h, 24h, 7d, or 30d.",
    );
  }
  return range as ModelRange;
}

function readModelProvider(
  value: unknown,
): ProviderStatus["provider"] | null {
  if (value === null || value === undefined) return null;
  if (
    typeof value !== "string" ||
    !modelProviders.has(value as ProviderStatus["provider"])
  ) {
    return rejectModelAnalytics(
      "invalid_provider",
      "Provider must use a supported Quill provider identifier.",
    );
  }
  return value as ProviderStatus["provider"];
}

function compareUnicodeScalars(left: string, right: string): number {
  const leftScalars = Array.from(left, (value) => value.codePointAt(0) ?? 0);
  const rightScalars = Array.from(right, (value) => value.codePointAt(0) ?? 0);
  const sharedLength = Math.min(leftScalars.length, rightScalars.length);
  for (let index = 0; index < sharedLength; index += 1) {
    const difference = leftScalars[index] - rightScalars[index];
    if (difference !== 0) return difference;
  }
  return leftScalars.length - rightScalars.length;
}

function compareModelIdentities(left: ModelIdentity, right: ModelIdentity): number {
  return (
    compareUnicodeScalars(left.provider, right.provider) ||
    compareUnicodeScalars(left.modelId, right.modelId)
  );
}

function modelIdentityFixtureKey(identity: ModelIdentity): string {
  return JSON.stringify([identity.provider, identity.modelId]);
}

function modelSessionFixtureKey(
  observation: Pick<MockModelObservation, "provider" | "sessionId">,
): string {
  return JSON.stringify([observation.provider, observation.sessionId]);
}

function modelSourceFixtureKey(
  observation: Pick<MockModelObservation, "provider" | "sourceKey">,
): string {
  return JSON.stringify([observation.provider, observation.sourceKey]);
}

function modelObservationTokens(observation: MockModelObservation): number {
  return (
    (observation.inputTokens ?? 0) +
    (observation.outputTokens ?? 0) +
    (observation.cacheCreationTokens ?? 0) +
    (observation.cacheReadTokens ?? 0)
  );
}

function getModelFixtureObservations(
  scenario: ModelFixtureScenario,
  range: ModelRange,
  provider: ProviderStatus["provider"] | null,
): MockModelObservation[] {
  const scenarioProvider = provider ?? "claude";
  let observations: MockModelObservation[];

  if (scenario === "filter-empty") {
    observations = [
      {
        provider: scenarioProvider,
        sourceKey: `${scenarioProvider}/filter-empty-outside-range.jsonl`,
        sessionId: "filter-empty-outside-range",
        observedAt: now - MODEL_RANGE_MS[range] - M,
        modelId: "fixture/outside-selected-range",
        kind: "turn",
        inputTokens: 700,
        outputTokens: 180,
        cacheCreationTokens: 80,
        cacheReadTokens: 340,
      },
    ];
  } else if (scenario === "no-sessions") {
    const suppressedSourceKeys = new Set(
      modelObservations
        .filter(({ sourceSuppressed }) => sourceSuppressed === true)
        .map(modelSourceFixtureKey),
    );
    observations = modelObservations.filter((observation) =>
      suppressedSourceKeys.has(modelSourceFixtureKey(observation)),
    );
  } else if (scenario === "no-model-evidence") {
    observations = [
      {
        provider: scenarioProvider,
        sourceKey: `${scenarioProvider}/unattributed-active-session.jsonl`,
        sessionId: "unattributed-active-session",
        observedAt: now - M,
        modelId: null,
        kind: "token",
        inputTokens: 2_400,
        outputTokens: 620,
        cacheCreationTokens: null,
        cacheReadTokens: 1_280,
      },
    ];
  } else {
    observations = modelObservations;
  }

  const suppressedSourceKeys = new Set(
    observations
      .filter(({ sourceSuppressed }) => sourceSuppressed === true)
      .map(modelSourceFixtureKey),
  );
  return observations.filter(
    (observation) =>
      !suppressedSourceKeys.has(modelSourceFixtureKey(observation)),
  );
}

function getScopedModelObservations(
  observations: readonly MockModelObservation[],
  range: ModelRange,
  provider: ProviderStatus["provider"] | null,
): MockModelObservation[] {
  const rangeStart = now - MODEL_RANGE_MS[range];
  return observations.filter(
    (observation) =>
      observation.observedAt >= rangeStart &&
      observation.observedAt < now &&
      (provider === null || observation.provider === provider),
  );
}

const ACTIVITY_BUCKET_SECONDS: Record<ModelRange, number> = {
  "1h": 10 * 60,
  "6h": 15 * 60,
  "24h": 60 * 60,
  "7d": 24 * 60 * 60,
  "30d": 24 * 60 * 60,
};

const OVERVIEW_MATRIX_PROJECT_LIMIT = 8;
const OVERVIEW_TOP_PAIR_LIMIT = 5;

function observationProject(observation: MockModelObservation): string {
  const cwd = observation.cwd;
  if (typeof cwd === "string" && cwd.length > 0) {
    const segments = cwd.split("/").filter((segment) => segment.length > 0);
    const tail = segments[segments.length - 1];
    if (tail !== undefined) return tail;
  }
  return observation.sessionId.split("-")[0] ?? observation.sessionId;
}

function utcDayKey(timestampMs: number): string {
  return new Date(timestampMs).toISOString().slice(0, 10);
}

function createModelUsageOverviewFixture(
  args: Record<string, unknown> | undefined,
): ModelUsageOverviewResponse {
  const range = readModelRange(args);
  const provider = readModelProvider(args?.provider);
  const scenario = readModelFixtureScenario();
  rejectRequestedModelFixture("overview");
  const backfill = getModelBackfillFixture(scenario);
  const observations = getModelFixtureObservations(scenario, range, provider);
  const scoped = getScopedModelObservations(observations, range, provider);
  const allProvidersInRange = getScopedModelObservations(
    observations,
    range,
    null,
  );

  interface OverviewModelAggregate {
    identity: ModelIdentity;
    sessionIds: Set<string>;
    projects: Set<string>;
    turns: number;
    attributedTokens: number;
    days: Set<string>;
    firstSeen: number;
    lastSeen: number;
  }

  interface OverviewSessionAggregate {
    sessionKey: string;
    project: string;
    modelKeys: Set<string>;
    tokensByModel: Map<string, number>;
    turnsByModel: Map<string, number>;
  }

  const modelAggregates = new Map<string, OverviewModelAggregate>();
  const sessionAggregates = new Map<string, OverviewSessionAggregate>();
  const identitiesByKey = new Map<string, ModelIdentity>();
  const projectSessions = new Map<string, Set<string>>();
  let attributedTokens = 0;
  let totalTokens = 0;
  let totalTurns = 0;
  let scopedEvidenceCount = 0;
  let parentTokens = 0;
  let subagentTokens = 0;
  const parentAttributedByModel = new Map<string, number>();
  const subagentAttributedByModel = new Map<string, number>();

  for (const observation of scoped) {
    const tokens = modelObservationTokens(observation);
    totalTokens += tokens;
    totalTurns += observation.kind === "turn" ? 1 : 0;
    const isSubagent =
      observation.parentChainId !== undefined &&
      observation.parentChainId !== null;
    if (isSubagent) subagentTokens += tokens;
    else parentTokens += tokens;

    const sessionKey = modelSessionFixtureKey(observation);
    const project = observationProject(observation);
    const projectSet = projectSessions.get(project) ?? new Set<string>();
    projectSet.add(sessionKey);
    projectSessions.set(project, projectSet);

    if (observation.modelId === null) continue;
    scopedEvidenceCount += 1;
    attributedTokens += tokens;

    const identity = {
      provider: observation.provider,
      modelId: observation.modelId,
    } satisfies ModelIdentity;
    const identityKey = modelIdentityFixtureKey(identity);
    identitiesByKey.set(identityKey, identity);
    const sideMap = isSubagent
      ? subagentAttributedByModel
      : parentAttributedByModel;
    sideMap.set(identityKey, (sideMap.get(identityKey) ?? 0) + tokens);

    const aggregate = modelAggregates.get(identityKey) ?? {
      identity,
      sessionIds: new Set<string>(),
      projects: new Set<string>(),
      turns: 0,
      attributedTokens: 0,
      days: new Set<string>(),
      firstSeen: observation.observedAt,
      lastSeen: observation.observedAt,
    };
    aggregate.sessionIds.add(sessionKey);
    aggregate.projects.add(project);
    aggregate.turns += observation.kind === "turn" ? 1 : 0;
    aggregate.attributedTokens += tokens;
    aggregate.days.add(utcDayKey(observation.observedAt));
    aggregate.firstSeen = Math.min(aggregate.firstSeen, observation.observedAt);
    aggregate.lastSeen = Math.max(aggregate.lastSeen, observation.observedAt);
    modelAggregates.set(identityKey, aggregate);

    const session = sessionAggregates.get(sessionKey) ?? {
      sessionKey,
      project,
      modelKeys: new Set<string>(),
      tokensByModel: new Map<string, number>(),
      turnsByModel: new Map<string, number>(),
    };
    session.modelKeys.add(identityKey);
    session.tokensByModel.set(
      identityKey,
      (session.tokensByModel.get(identityKey) ?? 0) + tokens,
    );
    session.turnsByModel.set(
      identityKey,
      (session.turnsByModel.get(identityKey) ?? 0) +
        (observation.kind === "turn" ? 1 : 0),
    );
    sessionAggregates.set(sessionKey, session);
  }

  // Primary-in counts: the model with the most attributed work per session.
  const primaryIn = new Map<string, number>();
  for (const session of sessionAggregates.values()) {
    let bestKey: string | null = null;
    let bestTokens = -1;
    let bestTurns = -1;
    for (const key of session.modelKeys) {
      const tokens = session.tokensByModel.get(key) ?? 0;
      const turns = session.turnsByModel.get(key) ?? 0;
      if (
        tokens > bestTokens ||
        (tokens === bestTokens && turns > bestTurns) ||
        (tokens === bestTokens &&
          turns === bestTurns &&
          (bestKey === null || compareUnicodeScalars(key, bestKey) < 0))
      ) {
        bestKey = key;
        bestTokens = tokens;
        bestTurns = turns;
      }
    }
    if (bestKey !== null) {
      primaryIn.set(bestKey, (primaryIn.get(bestKey) ?? 0) + 1);
    }
  }

  const scopedSessions = new Set(scoped.map(modelSessionFixtureKey));
  const totalSessions = scopedSessions.size;
  const models = Array.from(modelAggregates.entries())
    .sort(
      ([, left], [, right]) =>
        right.sessionIds.size - left.sessionIds.size ||
        right.attributedTokens - left.attributedTokens ||
        compareModelIdentities(left.identity, right.identity),
    )
    .map(([identityKey, aggregate]) => ({
      identity: aggregate.identity,
      sessions: aggregate.sessionIds.size,
      sessionPercent:
        totalSessions === 0
          ? null
          : (100 * aggregate.sessionIds.size) / totalSessions,
      projects: aggregate.projects.size,
      turns: aggregate.turns,
      primaryIn: primaryIn.get(identityKey) ?? 0,
      daysActive: aggregate.days.size,
      attributedTokens: aggregate.attributedTokens,
      sharePercent:
        attributedTokens === 0
          ? null
          : (100 * aggregate.attributedTokens) / attributedTokens,
      firstSeen: new Date(aggregate.firstSeen).toISOString(),
      lastSeen: new Date(aggregate.lastSeen).toISOString(),
    }));

  // Running now: latest attributed run per provider, with what it replaced.
  const runningNow: ModelUsageOverviewResponse["runningNow"] = [];
  const byProvider = new Map<string, MockModelObservation[]>();
  for (const observation of scoped) {
    if (observation.modelId === null) continue;
    const entries = byProvider.get(observation.provider) ?? [];
    entries.push(observation);
    byProvider.set(observation.provider, entries);
  }
  for (const [observationProvider, entries] of byProvider) {
    entries.sort((left, right) => left.observedAt - right.observedAt);
    const last = entries[entries.length - 1];
    if (last === undefined || last.modelId === null) continue;
    let runStart = entries.length - 1;
    while (
      runStart > 0 &&
      entries[runStart - 1].modelId === last.modelId
    ) {
      runStart -= 1;
    }
    runningNow.push({
      provider: observationProvider,
      modelId: last.modelId,
      lastSeenAt: new Date(last.observedAt).toISOString(),
      runningSinceAt: new Date(entries[runStart].observedAt).toISOString(),
      previousModelId: runStart > 0 ? entries[runStart - 1].modelId : null,
    });
  }
  runningNow.sort((left, right) =>
    compareUnicodeScalars(left.provider, right.provider),
  );

  // Activity: distinct sessions per model per bucket.
  const bucketSeconds = ACTIVITY_BUCKET_SECONDS[range];
  const bucketMillis = bucketSeconds * 1_000;
  const rangeStart = now - MODEL_RANGE_MS[range];
  const bucketCount = Math.max(
    1,
    Math.ceil(MODEL_RANGE_MS[range] / bucketMillis),
  );
  const bucketStarts = Array.from({ length: bucketCount }, (_unused, index) =>
    new Date(rangeStart + index * bucketMillis).toISOString(),
  );
  const activitySessions = new Map<string, Set<string>[]>();
  for (const observation of scoped) {
    if (observation.modelId === null) continue;
    const bucketIndex = Math.floor(
      (observation.observedAt - rangeStart) / bucketMillis,
    );
    if (bucketIndex < 0 || bucketIndex >= bucketCount) continue;
    const identityKey = modelIdentityFixtureKey({
      provider: observation.provider,
      modelId: observation.modelId,
    });
    const buckets =
      activitySessions.get(identityKey) ??
      Array.from({ length: bucketCount }, () => new Set<string>());
    buckets[bucketIndex].add(modelSessionFixtureKey(observation));
    activitySessions.set(identityKey, buckets);
  }
  const activitySeries = models
    .map(({ identity }) => {
      const identityKey = modelIdentityFixtureKey(identity);
      const buckets = activitySessions.get(identityKey);
      return {
        identity,
        sessionsPerBucket:
          buckets === undefined
            ? Array.from({ length: bucketCount }, () => 0)
            : buckets.map((bucket) => bucket.size),
      };
    })
    .filter((entry) =>
      entry.sessionsPerBucket.some((sessions) => sessions > 0),
    );

  // Projects × models: distinct sessions per pairing, top projects first.
  const projectMatrix = Array.from(projectSessions.entries())
    .map(([project, sessions]) => {
      const cells = models
        .map(({ identity }) => {
          const identityKey = modelIdentityFixtureKey(identity);
          let cellSessions = 0;
          for (const sessionKey of sessions) {
            if (
              sessionAggregates.get(sessionKey)?.modelKeys.has(identityKey)
            ) {
              cellSessions += 1;
            }
          }
          return { identity, sessions: cellSessions };
        })
        .filter((cell) => cell.sessions > 0);
      return { project, totalSessions: sessions.size, cells };
    })
    .filter((row) => row.cells.length > 0)
    .sort(
      (left, right) =>
        right.totalSessions - left.totalSessions ||
        compareUnicodeScalars(left.project, right.project),
    )
    .slice(0, OVERVIEW_MATRIX_PROJECT_LIMIT);

  // Combinations: distinct-model counts per session + most-shared pairs.
  let single = 0;
  let dual = 0;
  let threePlus = 0;
  const pairSessions = new Map<string, number>();
  for (const session of sessionAggregates.values()) {
    const size = session.modelKeys.size;
    if (size === 1) single += 1;
    else if (size === 2) dual += 1;
    else if (size >= 3) threePlus += 1;
    if (size < 2) continue;
    const keys = Array.from(session.modelKeys).sort(compareUnicodeScalars);
    for (let a = 0; a < keys.length; a += 1) {
      for (let b = a + 1; b < keys.length; b += 1) {
        const pairKey = JSON.stringify([keys[a], keys[b]]);
        pairSessions.set(pairKey, (pairSessions.get(pairKey) ?? 0) + 1);
      }
    }
  }
  const topPairs = Array.from(pairSessions.entries())
    .sort(
      ([leftKey, left], [rightKey, right]) =>
        right - left || compareUnicodeScalars(leftKey, rightKey),
    )
    .slice(0, OVERVIEW_TOP_PAIR_LIMIT)
    .flatMap(([pairKey, sharedSessions]) => {
      const [aKey, bKey] = JSON.parse(pairKey) as [string, string];
      const a = identitiesByKey.get(aKey);
      const b = identitiesByKey.get(bKey);
      return a === undefined || b === undefined
        ? []
        : [{ a, b, sharedSessions }];
    });

  const delegationTop = (
    sideMap: Map<string, number>,
  ): ModelUsageOverviewResponse["delegation"]["parentTop"] => {
    let bestKey: string | null = null;
    let bestTokens = 0;
    let sideTotal = 0;
    for (const [key, tokens] of sideMap) {
      sideTotal += tokens;
      if (
        tokens > bestTokens ||
        (tokens === bestTokens &&
          bestKey !== null &&
          compareUnicodeScalars(key, bestKey) < 0)
      ) {
        bestKey = key;
        bestTokens = tokens;
      }
    }
    const identity = bestKey === null ? undefined : identitiesByKey.get(bestKey);
    if (identity === undefined || sideTotal === 0) return null;
    return { identity, sharePercent: (100 * bestTokens) / sideTotal };
  };

  const globalSessions = new Set(observations.map(modelSessionFixtureKey));
  const representedProviders = Array.from(
    new Set(allProvidersInRange.map(({ provider: value }) => value)),
  ).sort(compareUnicodeScalars);
  const multiModelSessions = Array.from(sessionAggregates.values()).filter(
    (session) => session.modelKeys.size > 1,
  ).length;

  return {
    generatedAt: new Date(now).toISOString(),
    range,
    provider,
    representedProviders,
    scope: {
      globalSessionCount: globalSessions.size,
      scopedSessionCount: totalSessions,
      scopedEvidenceCount,
      inventoryComplete: backfill.inventoryComplete,
      scopeFinal:
        backfill.status === "complete" &&
        backfill.inventoryComplete &&
        backfill.failedRoots === 0 &&
        backfill.failedSources === 0 &&
        backfill.remainingSources === 0,
    },
    backfill: { ...backfill },
    totals: {
      sessions: totalSessions,
      projects: projectSessions.size,
      turns: totalTurns,
      attributedTokens,
      totalTokens,
      coveragePercent:
        totalTokens === 0 ? null : (100 * attributedTokens) / totalTokens,
      distinctModels: modelAggregates.size,
      multiModelSessions,
    },
    runningNow,
    models,
    activity: {
      bucketSeconds,
      bucketStarts,
      series: activitySeries,
    },
    projectMatrix,
    combinations: { single, dual, threePlus, topPairs },
    delegation: {
      parentTokens,
      subagentTokens,
      parentTop: delegationTop(parentAttributedByModel),
      subagentTop: delegationTop(subagentAttributedByModel),
    },
  };
}

function retryModelHistoryBackfillFixture(): ModelBackfillStatus {
  const scenario = readModelFixtureScenario();
  rejectRequestedModelFixture("retry");
  const current = getModelBackfillFixture(scenario);
  if (current.status === "pending" || current.status === "running") {
    return { ...current };
  }

  const pendingRetry: ModelBackfillStatus = {
    generation: current.generation + 1,
    trigger: "retry",
    status: "pending",
    totalRoots: 0,
    completedRoots: 0,
    failedRoots: 0,
    inventoryComplete: false,
    totalSources: 0,
    processedSources: 0,
    failedSources: 0,
    skippedSources: 0,
    remainingSources: 0,
    observationsWritten: 0,
    startedAt: null,
    updatedAt: new Date().toISOString(),
    finishedAt: null,
    lastError: null,
  };
  modelBackfillFixtureOverride = {
    scenario,
    status: pendingRetry,
  };
  return { ...pendingRetry };
}

// --- Retention pruning (feature 014) ------------------------------------------
//
// Every terminal state the settings surface has to render is reachable from the
// browser without a Rust backend, selected by a `?retentionFixture=<scenario>`
// query param in the same style as `?modelFixture=`. Browser-demo controls stay
// outside IPC payloads so the command contracts remain exact.

type RetentionScenario =
  // Preview returns exact counts and the run completes with a real VACUUM.
  | "preview"
  // Fresh install / nothing older than the cutoff: the structured no-op skip.
  | "noop"
  // Preview succeeds; the confirmation is refused because it went stale.
  | "stale_preview"
  // The quiesce lease is held, so both commands return the busy skip.
  | "busy"
  // Rows removed, but the VACUUM preflight refused: bytes_after == bytes_before.
  | "skipped_compaction"
  // Chunks committed, then the run stopped. Carries error_reason.
  | "partial"
  // The cutoff covers every owned row — drives the explicit-loss copy.
  | "everything_older";

const RETENTION_SCENARIOS: ReadonlySet<RetentionScenario> = new Set([
  "preview",
  "noop",
  "stale_preview",
  "busy",
  "skipped_compaction",
  "partial",
  "everything_older",
]);

const warnedInvalidRetentionScenarios = new Set<string>();

function readRetentionScenario(): RetentionScenario {
  if (typeof window === "undefined") return "preview";
  const requested = new URLSearchParams(window.location.search).get(
    "retentionFixture",
  );
  if (requested === null || requested.length === 0) return "preview";
  if (RETENTION_SCENARIOS.has(requested as RetentionScenario)) {
    return requested as RetentionScenario;
  }
  if (!warnedInvalidRetentionScenarios.has(requested)) {
    warnedInvalidRetentionScenarios.add(requested);
    console.warn("[mock] invalid retentionFixture browser control:", requested);
  }
  return "preview";
}

// The backend rejects anything off this list at the command boundary; the mock
// mirrors it so the preset selector cannot look more permissive in the browser
// than it is in the app.
const RETENTION_WINDOW_PRESETS = [30, 90, 180, 365] as const;

// Conforming timestamps: exactly 24 characters ending in "Z", byte-comparable
// against stored transcript timestamps. `toISOString()` produces this form.
const retentionCutoff = (windowDays: number) => iso(windowDays * D);

// Reassigned, never mutated, so `set_retention_policy` in the browser behaves
// like the real write-then-reread.
let retentionWindowDays: number | null = 90;

const affectedSurfaces = [
  "Session drilldowns for sessions older than the cutoff",
  "Subagent trees for pre-cutoff sessions",
  "Batch session code stats for pre-cutoff sessions",
];

function retentionAuditRecord(scenario: RetentionScenario): RetentionAuditRecord | null {
  if (scenario === "noop") return null;
  const cutoff = retentionCutoff(90);
  const base: RetentionAuditRecord = {
    schema: 1,
    status: "completed",
    reason: null,
    error_reason: null,
    window_days: 90,
    cutoff,
    ran_at: iso(112 * D),
    deleted: { tool_actions: 165_912, session_events: 523_847, model_usage_observations: 302_901 },
    skipped_nonconforming: { tool_actions: 3, session_events: 5, model_usage_observations: 0 },
    bytes_before: 8_106_127_360,
    bytes_after: 6_442_450_944,
  };
  if (scenario === "partial") {
    return {
      ...base,
      status: "partial",
      error_reason:
        "free space fell below the delete-phase budget after 41 chunks",
      deleted: { tool_actions: 61_440, session_events: 190_512, model_usage_observations: 112_640 },
      bytes_after: base.bytes_before,
    };
  }
  if (scenario === "busy") {
    // A skipped run is recorded exactly like a completed one, so the audit
    // surface needs a browser-reachable skip: "I tried on this date and nothing
    // happened, because X" is the question the record exists to answer.
    return {
      ...base,
      status: "skipped",
      reason: "another maintenance operation was running",
      cutoff: null,
      ran_at: iso(2 * D),
      deleted: { tool_actions: 0, session_events: 0, model_usage_observations: 0 },
      skipped_nonconforming: { tool_actions: 0, session_events: 0, model_usage_observations: 0 },
      bytes_after: base.bytes_before,
    };
  }
  if (scenario === "skipped_compaction") {
    // Rows removed, bytes not reclaimed — the audit repeats the rows-are-not-
    // bytes sentence rather than letting an unchanged file size look like a
    // failed prune.
    return { ...base, bytes_after: base.bytes_before };
  }
  return base;
}

function retentionPolicy(): RetentionPolicy {
  const scenario = readRetentionScenario();
  return {
    window_days: retentionWindowDays,
    watermark: retentionWindowDays === null ? null : retentionCutoff(90),
    last_run: retentionAuditRecord(scenario),
  };
}

function retentionPreview(scenario: RetentionScenario): RetentionPreview {
  const windowDays = retentionWindowDays;
  const empty = {
    tool_actions_rows: 0,
    session_events_rows: 0,
    model_usage_observations_rows: 0,
    tool_actions_nonconforming: 0,
    session_events_nonconforming: 0,
    everything_older: false,
    bytes_before: 8_106_127_360,
    affected_surfaces: [],
  };
  if (windowDays === null) {
    return {
      status: "skipped",
      reason: "Retention is set to never; nothing is eligible for pruning.",
      cutoff: null,
      window_days: null,
      ...empty,
    };
  }
  if (scenario === "busy") {
    return {
      status: "skipped",
      reason: "another maintenance operation is running",
      cutoff: null,
      window_days: windowDays,
      ...empty,
    };
  }
  if (scenario === "noop") {
    return {
      status: "skipped",
      reason: `No transcript rows are older than ${windowDays} days yet.`,
      cutoff: retentionCutoff(windowDays),
      window_days: windowDays,
      ...empty,
    };
  }
  return {
    status: "ready",
    reason: null,
    cutoff: retentionCutoff(windowDays),
    window_days: windowDays,
    tool_actions_rows: 165_912,
    session_events_rows: 523_847,
    model_usage_observations_rows: 302_901,
    tool_actions_nonconforming: 3,
    session_events_nonconforming: 5,
    // The cutoff covering every owned row is the case that needs the blunter
    // confirmation copy, so it gets its own scenario rather than a flag nobody
    // can reach from the browser.
    everything_older: scenario === "everything_older",
    bytes_before: 8_106_127_360,
    affected_surfaces: affectedSurfaces,
  };
}

function retentionResult(
  scenario: RetentionScenario,
  archiveBeforePrune: boolean,
): RetentionMaintenanceResult {
  const windowDays = retentionWindowDays;
  const bytesBefore = 8_106_127_360;
  const skipped = (reason: string): RetentionMaintenanceResult => ({
    status: "skipped",
    reason,
    error_reason: null,
    cutoff: null,
    window_days: windowDays,
    tool_actions_deleted: 0,
    session_events_deleted: 0,
    model_usage_observations_deleted: 0,
    tool_actions_nonconforming: 0,
    session_events_nonconforming: 0,
    compaction_status: "skipped",
    compaction_reason: null,
    bytes_before: bytesBefore,
    bytes_after: bytesBefore,
    archive_path: null,
    tool_actions_archived: 0,
    session_events_archived: 0,
    model_usage_observations_archived: 0,
  });

  if (windowDays === null) {
    return skipped("Retention is set to never; nothing was removed.");
  }
  if (scenario === "busy") return skipped("another maintenance operation is running");
  if (scenario === "noop") {
    return skipped(`No transcript rows are older than ${windowDays} days yet.`);
  }
  if (scenario === "stale_preview") {
    return skipped("stale_preview");
  }

  const completed: RetentionMaintenanceResult = {
    status: "completed",
    reason: null,
    error_reason: null,
    cutoff: retentionCutoff(windowDays),
    window_days: windowDays,
    tool_actions_deleted: 165_912,
    session_events_deleted: 523_847,
    model_usage_observations_deleted: 302_901,
    tool_actions_nonconforming: 3,
    session_events_nonconforming: 5,
    compaction_status: "completed",
    compaction_reason: null,
    bytes_before: bytesBefore,
    bytes_after: 6_442_450_944,
    archive_path: archiveBeforePrune
      ? "/home/demo/.local/share/com.quilltoolkit.app/retention-archives/quill-retention-archive.jsonl"
      : null,
    tool_actions_archived: archiveBeforePrune ? 165_915 : 0,
    session_events_archived: archiveBeforePrune ? 523_852 : 0,
    model_usage_observations_archived: archiveBeforePrune ? 302_901 : 0,
  };
  if (scenario === "skipped_compaction") {
    // Rows removed, bytes not yet reclaimed — a legitimate outcome, and the one
    // the "deletion alone frees no filesystem bytes" copy exists for.
    return {
      ...completed,
      compaction_status: "skipped",
      compaction_reason: "not enough free disk space for a safe compaction.",
      bytes_after: bytesBefore,
    };
  }
  if (scenario === "partial") {
    return {
      ...completed,
      status: "partial",
      error_reason:
        "free space fell below the delete-phase budget after 41 chunks",
      tool_actions_deleted: 61_440,
      session_events_deleted: 190_512,
      compaction_status: "skipped",
      compaction_reason: "compaction is not attempted after a partial run.",
      bytes_after: bytesBefore,
    };
  }
  return completed;
}

async function emitRetentionProgress(phase: string, pct: number) {
  const progress: RetentionMaintenanceProgress = { phase, pct };
  await emit("retention-maintenance-progress", progress);
  await new Promise((resolve) => setTimeout(resolve, 300));
}

async function previewRetentionFixture(): Promise<RetentionPreview> {
  const scenario = readRetentionScenario();
  // The counting phase is heartbeat-driven in the backend, so it visibly
  // advances rather than sitting pinned at zero.
  await emitRetentionProgress("Counting rows", 20);
  await emitRetentionProgress("Counting rows", 70);
  return retentionPreview(scenario);
}

async function runRetentionMaintenanceFixture(
  args: Record<string, unknown> | undefined,
): Promise<RetentionMaintenanceResult> {
  const scenario = readRetentionScenario();
  const archiveBeforePrune =
    args?.archiveBeforePrune === true || args?.archive_before_prune === true;
  const result = retentionResult(scenario, archiveBeforePrune);
  if (result.status !== "skipped") {
    await emitRetentionProgress("Counting rows", 10);
    if (archiveBeforePrune) {
      await emitRetentionProgress("Archiving rows", 25);
    }
    await emitRetentionProgress("Checking disk space", 25);
    await emitRetentionProgress("Removing old rows", 45);
    await emitRetentionProgress("Removing old rows", 80);
    if (result.compaction_status === "completed") {
      await emitRetentionProgress("Compacting database", 92);
    }
  }
  await emit("retention-maintenance-finished", result);
  return result;
}

function setRetentionPolicyFixture(
  args: Record<string, unknown> | undefined,
): RetentionPolicy {
  const requested = args?.windowDays ?? args?.window_days ?? null;
  if (requested === null) {
    retentionWindowDays = null;
    return retentionPolicy();
  }
  if (
    typeof requested !== "number" ||
    !RETENTION_WINDOW_PRESETS.includes(requested as 30 | 90 | 180 | 365)
  ) {
    // Mirrors the backend's command-boundary rejection: the stored window is
    // left exactly as it was, which is the half the 30-day floor depends on.
    throw new Error(
      `Unsupported retention window ${String(requested)}; expected one of ${RETENTION_WINDOW_PRESETS.join(", ")} or never`,
    );
  }
  retentionWindowDays = requested;
  return retentionPolicy();
}

// --- Widget aggregates (feature 018) ------------------------------------------
// Sample answers for `get_provider_token_series` and `get_activity_series`,
// typed by the same contract the Rust commands serialize, so a drift between
// the mock and the backend shape fails typecheck instead of only showing up in
// the browser.

/** Curves lifted from the mockup so browser mode matches the design intent. */
const CODEX_CURVE = [9, 12, 15, 14, 19, 23, 26, 25, 31, 36, 40, 45, 52] as const;
const CLAUDE_CURVE = [4, 5, 7, 9, 8, 11, 13, 15, 14, 17, 19, 20, 21] as const;
const SESSION_CURVE = [2, 3, 3, 5, 4, 6, 7, 8, 7, 9, 10, 12, 13] as const;
const PROJECT_CURVE = [1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 9] as const;

/** Tokens per curve unit, so a wider range reads as a bigger number. */
const RANGE_TOKEN_SCALE: Record<string, number> = {
  "1h": 3_200,
  "6h": 24_000,
  "24h": 78_000,
  "7d": 410_000,
  "30d": 1_450_000,
};

const PROVIDER_SERIES_BUCKETS = 13;
const ACTIVITY_SERIES_BUCKETS = 8;

function providerSeries(
  provider: string,
  curve: readonly number[],
  scale: number,
  count: number,
): ProviderTokenSeries {
  const values = resample(curve, count).map((unit) => unit * scale);
  return {
    provider,
    values,
    total_tokens: values.reduce((sum, value) => sum + value, 0),
  };
}

function providerTokenSeries(range: string, buckets: number): ProviderTokenSeriesResponse {
  const scale = RANGE_TOKEN_SCALE[range] ?? RANGE_TOKEN_SCALE["24h"];
  const series = [
    providerSeries("codex", CODEX_CURVE, scale, buckets),
    providerSeries("claude", CLAUDE_CURVE, scale, buckets),
  ];
  return {
    range,
    bucket_secs: bucketSecs(range, buckets),
    timestamps: bucketTimestamps(range, buckets),
    series,
    total_tokens: series.reduce((sum, entry) => sum + entry.total_tokens, 0),
  };
}

function activitySeries(range: string, buckets: number): ActivitySeriesResponse {
  return {
    range,
    bucket_secs: bucketSecs(range, buckets),
    timestamps: bucketTimestamps(range, buckets),
    session_counts: resample(SESSION_CURVE, buckets),
    project_counts: resample(PROJECT_CURVE, buckets),
  };
}

// --- Command → fixture map ----------------------------------------------------

type FixtureHandler = (args?: Record<string, unknown>) => unknown;

const fixtures: Record<string, FixtureHandler> = {
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
  compact_database: async () => {
    await emit("compact-database-progress", { phase: "Checking disk space", pct: 15 });
    await new Promise((resolve) => setTimeout(resolve, 350));
    await emit("compact-database-progress", { phase: "Preparing database", pct: 45 });
    await new Promise((resolve) => setTimeout(resolve, 350));
    const result = {
      status: "skipped" as const,
      reason: "not enough free disk space for a safe compaction.",
      bytes_before: 1_048_576_000,
      bytes_after: 1_048_576_000,
    };
    await emit("compact-database-finished", result);
    return result;
  },
  // retention pruning
  get_retention_policy: () => retentionPolicy(),
  set_retention_policy: (args) => setRetentionPolicyFixture(args),
  preview_retention: () => previewRetentionFixture(),
  run_retention_maintenance: (args) => runRetentionMaintenanceFixture(args),
  // live usage
  fetch_usage_data: () => usageData,
  // tokens
  get_token_history: (args) => tokenHistory(rangeArg(args)),
  get_token_stats: () => tokenStats,
  get_provider_token_series: (args) =>
    providerTokenSeries(rangeArg(args), bucketsArg(args, PROVIDER_SERIES_BUCKETS)),
  get_activity_series: (args) =>
    activitySeries(rangeArg(args), bucketsArg(args, ACTIVITY_SERIES_BUCKETS)),
  // code
  get_code_stats: () => codeStats,
  get_code_stats_history: (args) => codeHistory(rangeArg(args)),
  get_batch_session_code_stats: () => ({}),
  // breakdowns
  get_host_breakdown: () => hostBreakdown,
  get_project_breakdown: () => projectBreakdown,
  get_session_breakdown: () => sessionBreakdown,
  get_skill_breakdown: () => skillBreakdown,
  get_hook_breakdown: () => hookBreakdown,
  // stats
  get_llm_runtime_stats: (args) => llmRuntimeStats(rangeArg(args)),
  get_top_tools: () => topTools,
  get_observation_count: () => 184,
  get_unanalyzed_observation_count: () => 12,
  get_observation_sparkline: () => [4, 7, 5, 9, 6, 11, 8, 10, 12, 9, 7, 13],
  // context savings
  get_context_savings_analytics: () => contextSavings,
  // session model analytics
  get_model_usage_overview: (args) => createModelUsageOverviewFixture(args),
  retry_model_history_backfill: () => retryModelHistoryBackfillFixture(),
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
  // sessions
  search_sessions: () => searchResults,
  get_search_facets: () => searchFacets,
  get_session_context: () => ({ provider: "claude", messages: [], session_id: "a1b2c3d4", project: "quill" }),
  sync_search_index: () => 0,
  // release notes / updates
  get_release_notes: () => [],
  // misc no-ops
  set_indicator_primary_provider: () => null,
  set_minimax_api_key: () => null,
  get_cpa_connection_status: () => ({ baseUrl: null, configured: false }),
  set_cpa_connection: (args) => ({
    connection: {
      baseUrl:
        typeof args?.baseUrl === "string" ? args.baseUrl : "http://127.0.0.1:8317",
      configured: true,
    },
    smoke: {
      claude: { state: "available", message: "Claude quota path verified." },
      codex: { state: "available", message: "Codex quota path verified." },
    },
  }),
  clear_cpa_connection: () => null,
  hide_window: () => null,
  quit_app: () => null,
};

let listenerSeq = 1;

/**
 * Mock handler for every Tauri `invoke()` call in the browser. Returns realistic
 * fixtures for known commands, benign defaults for Tauri core/plugin commands
 * (including event listen/unlisten), and `null` for anything unmapped.
 */
export function handleInvoke(cmd: string, args?: Record<string, unknown>): unknown {
  // Event plugin: let `listen()` resolve with a fake registration; events never fire.
  if (cmd.startsWith("plugin:event|listen")) return listenerSeq++;
  if (cmd.startsWith("plugin:")) return undefined;

  const fixture = fixtures[cmd];
  if (fixture) return fixture(args);

  if (import.meta.env.DEV) console.debug("[mock] unhandled invoke:", cmd);
  return null;
}
