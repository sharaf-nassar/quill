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
  ModelAnalyticsError,
  ModelAnalyticsErrorCode,
  ModelBackfillStatus,
  ModelIdentity,
  ModelRange,
  ModelSessionRow,
  ModelSessionsResponse,
  ModelUsageOverviewResponse,
  ProjectBreakdown,
  ProjectTokensRaw,
  ProviderStatus,
  RestartStatus,
  RuntimeSettings,
  SearchFacets,
  SearchResults,
  SessionBreakdown,
  SessionModelChain,
  SessionModelHistoryResponse,
  SessionModelSegment,
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
// Most timestamps mirror the Rust backend's `to_rfc3339()` (zone-designated)
// and are consumed directly via `new Date(...)` — session times, rate-limit
// resets, verification stamps.
const iso = (msAgo: number) => new Date(now - msAgo).toISOString();
const isoIn = (msAhead: number) => new Date(now + msAhead).toISOString();
// `created_at` columns are DB-populated by SQLite `datetime('now')`, which is a
// space-separated naive-UTC string with NO "Z" (e.g. "2026-06-30 12:00:00").
// utils/time.ts#timeAgo appends the "Z" to read it as UTC, so pre-suffixing one
// here double-stamps it -> Invalid Date -> "NaNd ago" in the learning header.
const sqliteUtc = (msAgo: number) =>
  new Date(now - msAgo).toISOString().replace("T", " ").slice(0, -5);

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
    { provider: "claude", key: "weekly_scoped_fable", label: "Fable", utilization: 22, resets_at: isoIn(6 * D), sort_order: 1 },
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
  { id: 42, trigger_mode: "periodic", observations_analyzed: 184, rules_created: 2, rules_updated: 5, duration_ms: 41_200, status: "completed", error: null, logs: null, created_at: sqliteUtc(2 * H), phases: [{ name: "collect", status: "completed", duration_ms: 1_200, findings_count: 184 }, { name: "infer", status: "completed", duration_ms: 38_000, findings_count: 7 }], provider_scope: ["claude", "codex"], inference: { total_cost_usd: 0.142, total_duration_ms: 38_000, primary_model: "claude-opus-4-8", call_count: 4, failed_call_count: 0, calls: [] } },
  { id: 41, trigger_mode: "on-demand", observations_analyzed: 96, rules_created: 1, rules_updated: 2, duration_ms: 22_800, status: "completed", error: null, logs: null, created_at: sqliteUtc(28 * H), phases: null, provider_scope: ["claude"] },
];

const restartStatus: RestartStatus = {
  phase: "Idle",
  instances: [],
  waiting_on: 0,
  elapsed_seconds: 0,
};

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
  detailMissing?: boolean;
}

const MODEL_RANGE_MS: Record<ModelRange, number> = {
  "1h": H,
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
  // More than one default page uses the same opaque model identity as ordinary
  // evidence. Query handlers discover these records; they never branch on it.
  ...Array.from({ length: 23 }, (_unused, index) => {
    const ordinal = index + 1;
    const sessionId = `model-detail-session-${String(ordinal).padStart(2, "0")}`;
    return {
      provider: "claude" as const,
      sourceKey: `claude/${sessionId}.jsonl`,
      sessionId,
      observedAt: now - ordinal * M,
      modelId: "shared/model.snapshot",
      kind: "turn" as const,
      inputTokens: 180 + ordinal * 7,
      outputTokens: 40 + ordinal,
      cacheCreationTokens: ordinal % 3 === 0 ? null : 20,
      cacheReadTokens: 90 + ordinal * 2,
      displayName: `Paged model session ${String(ordinal).padStart(2, "0")}`,
      cwd: ordinal % 4 === 0 ? null : `/workspace/demo-${ordinal}`,
      hostname: ordinal % 5 === 0 ? null : `fixture-host-${ordinal}.local`,
      detailMissing: ordinal === 23,
    } satisfies MockModelObservation;
  }),
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

function trimRustStringWhitespace(value: string): string {
  return value.replace(/^\p{White_Space}+|\p{White_Space}+$/gu, "");
}

function hasUnpairedUtf16Surrogate(value: string): boolean {
  for (let index = 0; index < value.length; index += 1) {
    const codeUnit = value.charCodeAt(index);
    if (codeUnit >= 0xd800 && codeUnit <= 0xdbff) {
      if (index + 1 >= value.length) return true;
      const trailingCodeUnit = value.charCodeAt(index + 1);
      if (trailingCodeUnit < 0xdc00 || trailingCodeUnit > 0xdfff) return true;
      index += 1;
    } else if (codeUnit >= 0xdc00 && codeUnit <= 0xdfff) {
      return true;
    }
  }
  return false;
}

function readSelectedModel(
  value: unknown,
  providerFilter: ProviderStatus["provider"] | null,
): ModelIdentity | null {
  if (value === null || value === undefined) return null;
  if (typeof value !== "object") {
    return rejectModelAnalytics(
      "invalid_model_id",
      "Selected model identifier is invalid.",
    );
  }

  const provider = readModelProvider(Reflect.get(value, "provider"));
  if (provider === null) {
    return rejectModelAnalytics(
      "invalid_provider",
      "Selected model provider is required.",
    );
  }
  if (providerFilter !== null && provider !== providerFilter) {
    return rejectModelAnalytics(
      "invalid_provider",
      "Selected model provider must match the active provider filter.",
    );
  }

  const rawModelId = Reflect.get(value, "modelId");
  if (typeof rawModelId !== "string") {
    return rejectModelAnalytics(
      "invalid_model_id",
      "Selected model identifier is invalid.",
    );
  }
  const modelId = trimRustStringWhitespace(rawModelId);
  if (modelId.length === 0 || hasUnpairedUtf16Surrogate(modelId)) {
    return rejectModelAnalytics(
      "invalid_model_id",
      "Selected model identifier must contain 1-256 non-control Unicode characters.",
    );
  }
  const scalarCount = Array.from(modelId).length;
  if (scalarCount > 256 || /\p{Cc}/u.test(modelId)) {
    return rejectModelAnalytics(
      "invalid_model_id",
      "Selected model identifier must contain 1-256 non-control Unicode characters.",
    );
  }

  return { provider, modelId };
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

interface IndexedModelObservation {
  observation: MockModelObservation;
  ordinal: number;
}

interface ModelPrimaryAggregate {
  identity: ModelIdentity;
  attributedTokens: number;
  turns: number;
}

interface ModelSessionsFixtureCursor {
  version: 1;
  range: ModelRange;
  modelProvider: string;
  modelId: string;
  scenario: ModelFixtureScenario;
  lastActivityAt: string;
  provider: string;
  sessionId: string;
}

const MODEL_SESSIONS_FIXTURE_CURSOR_PREFIX = "qmf1.";
const MODEL_SESSIONS_DEFAULT_LIMIT = 20;
const MODEL_SESSIONS_MAX_LIMIT = 100;

function indexedModelObservations(
  observations: readonly MockModelObservation[],
): IndexedModelObservation[] {
  return observations.map((observation, ordinal) => ({
    observation,
    ordinal,
  }));
}

function compareIndexedModelObservations(
  left: IndexedModelObservation,
  right: IndexedModelObservation,
): number {
  return (
    left.observation.observedAt - right.observation.observedAt ||
    left.ordinal - right.ordinal
  );
}

function observationChainId(observation: MockModelObservation): string {
  return observation.chainId ?? observation.sessionId;
}

function getModelPrimary(
  observations: readonly IndexedModelObservation[],
): ModelIdentity | null {
  const aggregates = new Map<string, ModelPrimaryAggregate>();
  for (const { observation } of observations) {
    if (observation.modelId === null) continue;
    const identity = {
      provider: observation.provider,
      modelId: observation.modelId,
    } satisfies ModelIdentity;
    const key = modelIdentityFixtureKey(identity);
    const aggregate = aggregates.get(key) ?? {
      identity,
      attributedTokens: 0,
      turns: 0,
    };
    aggregate.attributedTokens += modelObservationTokens(observation);
    aggregate.turns += observation.kind === "turn" ? 1 : 0;
    aggregates.set(key, aggregate);
  }

  return (
    Array.from(aggregates.values()).sort(
      (left, right) =>
        right.attributedTokens - left.attributedTokens ||
        right.turns - left.turns ||
        compareModelIdentities(left.identity, right.identity),
    )[0]?.identity ?? null
  );
}

function getWithinChainSwitchCount(
  observations: readonly IndexedModelObservation[],
): number {
  const chains = new Map<string, IndexedModelObservation[]>();
  for (const indexed of observations) {
    if (indexed.observation.kind !== "turn") continue;
    const chainId = observationChainId(indexed.observation);
    const chain = chains.get(chainId) ?? [];
    chain.push(indexed);
    chains.set(chainId, chain);
  }

  let switchCount = 0;
  for (const chain of chains.values()) {
    let previousIdentityKey: string | null = null;
    for (const { observation } of chain.sort(
      compareIndexedModelObservations,
    )) {
      if (observation.modelId === null) {
        previousIdentityKey = null;
        continue;
      }
      const identityKey = modelIdentityFixtureKey({
        provider: observation.provider,
        modelId: observation.modelId,
      });
      if (
        previousIdentityKey !== null &&
        previousIdentityKey !== identityKey
      ) {
        switchCount += 1;
      }
      previousIdentityKey = identityKey;
    }
  }
  return switchCount;
}

function firstDefinedObservationValue<T>(
  observations: readonly IndexedModelObservation[],
  select: (observation: MockModelObservation) => T | undefined,
): T | undefined {
  for (const { observation } of observations) {
    const value = select(observation);
    if (value !== undefined) return value;
  }
  return undefined;
}

function createModelSessionRow(
  observations: readonly IndexedModelObservation[],
  selectedModel: ModelIdentity,
): ModelSessionRow {
  const first = observations[0]?.observation;
  const primaryModel = getModelPrimary(observations);
  if (first === undefined || primaryModel === null) {
    return rejectModelAnalytics(
      "storage_error",
      "Browser model session fixture could not build a session row.",
    );
  }

  const identityKeys = new Set<string>();
  const chainIds = new Set<string>();
  let selectedModelTokens = 0;
  let selectedModelTurns = 0;
  let lastActivityAt = Number.NEGATIVE_INFINITY;
  for (const { observation } of observations) {
    chainIds.add(observationChainId(observation));
    lastActivityAt = Math.max(lastActivityAt, observation.observedAt);
    if (observation.modelId !== null) {
      identityKeys.add(
        modelIdentityFixtureKey({
          provider: observation.provider,
          modelId: observation.modelId,
        }),
      );
    }
    if (
      observation.provider === selectedModel.provider &&
      observation.modelId === selectedModel.modelId
    ) {
      selectedModelTokens += modelObservationTokens(observation);
      selectedModelTurns += observation.kind === "turn" ? 1 : 0;
    }
  }

  return {
    provider: first.provider,
    sessionId: first.sessionId,
    displayName:
      firstDefinedObservationValue(observations, ({ displayName }) =>
        displayName === undefined ? undefined : displayName,
      ) ?? first.sessionId,
    cwd:
      firstDefinedObservationValue(observations, ({ cwd }) => cwd) ?? null,
    hostname:
      firstDefinedObservationValue(observations, ({ hostname }) => hostname) ??
      null,
    selectedModelTokens,
    selectedModelTurns,
    lastActivityAt: new Date(lastActivityAt).toISOString(),
    primaryModel,
    distinctModels: identityKeys.size,
    hasWithinChainSwitches: getWithinChainSwitchCount(observations) > 0,
    chainCount: chainIds.size,
  };
}

function readModelSessionsLimit(
  value: unknown,
): number {
  if (value === null || value === undefined) {
    return MODEL_SESSIONS_DEFAULT_LIMIT;
  }
  if (typeof value !== "number" || !Number.isSafeInteger(value)) {
    return rejectModelAnalytics(
      "storage_error",
      "Browser model session fixture limit must be an integer.",
    );
  }
  return Math.min(MODEL_SESSIONS_MAX_LIMIT, Math.max(1, value));
}

function encodeModelSessionsFixtureCursor(
  cursor: ModelSessionsFixtureCursor,
): string {
  const bytes = new TextEncoder().encode(JSON.stringify(cursor));
  const payload = Array.from(bytes, (byte) =>
    byte.toString(16).padStart(2, "0"),
  ).join("");
  return `${MODEL_SESSIONS_FIXTURE_CURSOR_PREFIX}${payload}`;
}

function rejectInvalidModelSessionsFixtureCursor(): never {
  return rejectModelAnalytics(
    "invalid_cursor",
    "The model session cursor is malformed, stale, or belongs to another request.",
  );
}

function decodeModelSessionsFixtureCursor(
  value: unknown,
  expected: Pick<
    ModelSessionsFixtureCursor,
    "range" | "modelProvider" | "modelId" | "scenario"
  >,
): ModelSessionsFixtureCursor | null {
  if (value === null || value === undefined) return null;
  if (
    typeof value !== "string" ||
    value.length > 4_096 ||
    !value.startsWith(MODEL_SESSIONS_FIXTURE_CURSOR_PREFIX)
  ) {
    return rejectInvalidModelSessionsFixtureCursor();
  }

  const encoded = value.slice(MODEL_SESSIONS_FIXTURE_CURSOR_PREFIX.length);
  if (
    encoded.length === 0 ||
    encoded.length % 2 !== 0 ||
    !/^[0-9a-f]+$/.test(encoded)
  ) {
    return rejectInvalidModelSessionsFixtureCursor();
  }

  try {
    const bytes = new Uint8Array(encoded.length / 2);
    for (let index = 0; index < bytes.length; index += 1) {
      bytes[index] = Number.parseInt(encoded.slice(index * 2, index * 2 + 2), 16);
    }
    const decoded = JSON.parse(
      new TextDecoder("utf-8", { fatal: true }).decode(bytes),
    ) as unknown;
    if (
      decoded === null ||
      typeof decoded !== "object" ||
      Array.isArray(decoded)
    ) {
      return rejectInvalidModelSessionsFixtureCursor();
    }
    const parsed = decoded as Partial<ModelSessionsFixtureCursor>;
    const cursorFields = new Set([
      "version",
      "range",
      "modelProvider",
      "modelId",
      "scenario",
      "lastActivityAt",
      "provider",
      "sessionId",
    ]);
    if (
      Object.keys(parsed).length !== cursorFields.size ||
      Object.keys(parsed).some((field) => !cursorFields.has(field))
    ) {
      return rejectInvalidModelSessionsFixtureCursor();
    }
    if (
      parsed.version !== 1 ||
      parsed.range !== expected.range ||
      parsed.modelProvider !== expected.modelProvider ||
      parsed.modelId !== expected.modelId ||
      parsed.scenario !== expected.scenario ||
      typeof parsed.lastActivityAt !== "string" ||
      !Number.isFinite(Date.parse(parsed.lastActivityAt)) ||
      new Date(parsed.lastActivityAt).toISOString() !== parsed.lastActivityAt ||
      parsed.provider !== expected.modelProvider ||
      typeof parsed.sessionId !== "string" ||
      parsed.sessionId.length === 0
    ) {
      return rejectInvalidModelSessionsFixtureCursor();
    }
    return parsed as ModelSessionsFixtureCursor;
  } catch {
    return rejectInvalidModelSessionsFixtureCursor();
  }
}

function compareModelSessionOrder(
  left: Pick<ModelSessionRow, "lastActivityAt" | "provider" | "sessionId">,
  right: Pick<ModelSessionRow, "lastActivityAt" | "provider" | "sessionId">,
): number {
  return (
    Date.parse(right.lastActivityAt) - Date.parse(left.lastActivityAt) ||
    compareUnicodeScalars(left.provider, right.provider) ||
    compareUnicodeScalars(left.sessionId, right.sessionId)
  );
}

function createModelSessionsFixture(
  args: Record<string, unknown> | undefined,
): ModelSessionsResponse {
  const range = readModelRange(args);
  const modelProvider = readModelProvider(args?.modelProvider);
  if (modelProvider === null) {
    return rejectModelAnalytics(
      "invalid_provider",
      "Selected model provider is required.",
    );
  }
  const identity = readSelectedModel(
    { provider: modelProvider, modelId: args?.modelId },
    modelProvider,
  );
  if (identity === null) {
    return rejectModelAnalytics(
      "invalid_model_id",
      "Selected model identifier is required.",
    );
  }

  const scenario = readModelFixtureScenario();
  rejectRequestedModelFixture("sessions");
  const observations = indexedModelObservations(
    getScopedModelObservations(
      getModelFixtureObservations(scenario, range, modelProvider),
      range,
      modelProvider,
    ),
  );
  const matchingSessionKeys = new Set(
    observations
      .filter(
        ({ observation }) =>
          observation.provider === identity.provider &&
          observation.modelId === identity.modelId,
      )
      .map(({ observation }) => modelSessionFixtureKey(observation)),
  );
  const observationsBySession = new Map<string, IndexedModelObservation[]>();
  for (const indexed of observations) {
    const sessionKey = modelSessionFixtureKey(indexed.observation);
    if (!matchingSessionKeys.has(sessionKey)) continue;
    const session = observationsBySession.get(sessionKey) ?? [];
    session.push(indexed);
    observationsBySession.set(sessionKey, session);
  }

  const sessions = Array.from(observationsBySession.values())
    .map((session) => createModelSessionRow(session, identity))
    .sort(compareModelSessionOrder);
  const expectedCursor = {
    range,
    modelProvider: identity.provider,
    modelId: identity.modelId,
    scenario,
  } satisfies Pick<
    ModelSessionsFixtureCursor,
    "range" | "modelProvider" | "modelId" | "scenario"
  >;
  const cursor = decodeModelSessionsFixtureCursor(args?.cursor, expectedCursor);
  if (
    cursor !== null &&
    !sessions.some((session) => compareModelSessionOrder(session, cursor) === 0)
  ) {
    return rejectInvalidModelSessionsFixtureCursor();
  }
  const limit = readModelSessionsLimit(args?.limit);
  const eligibleSessions =
    cursor === null
      ? sessions
      : sessions.filter(
          (session) => compareModelSessionOrder(session, cursor) > 0,
        );
  const page = eligibleSessions.slice(0, limit);
  const hasMore = eligibleSessions.length > limit;
  const finalRow = page[page.length - 1];

  return {
    identity,
    total: sessions.length,
    nextCursor:
      hasMore && finalRow !== undefined
        ? encodeModelSessionsFixtureCursor({
            version: 1,
            ...expectedCursor,
            lastActivityAt: finalRow.lastActivityAt,
            provider: finalRow.provider,
            sessionId: finalRow.sessionId,
          })
        : null,
    sessions: page,
  };
}

function createSessionModelSegments(
  observations: readonly IndexedModelObservation[],
): SessionModelSegment[] {
  const segments: SessionModelSegment[] = [];
  for (const { observation } of observations
    .filter(({ observation }) => observation.kind === "turn")
    .sort(compareIndexedModelObservations)) {
    const observedAt = new Date(observation.observedAt).toISOString();
    const previous = segments[segments.length - 1];
    if (observation.modelId === null) {
      if (previous?.kind === "modelGap") {
        previous.endedAt = observedAt;
        previous.turnCount += 1;
      } else {
        segments.push({
          kind: "modelGap",
          startedAt: observedAt,
          endedAt: observedAt,
          turnCount: 1,
        });
      }
      continue;
    }

    const identity = {
      provider: observation.provider,
      modelId: observation.modelId,
    } satisfies ModelIdentity;
    if (
      previous?.kind === "model" &&
      modelIdentityFixtureKey(previous.identity) ===
        modelIdentityFixtureKey(identity)
    ) {
      previous.endedAt = observedAt;
      previous.turnCount += 1;
      previous.attributedTokens += modelObservationTokens(observation);
    } else {
      segments.push({
        kind: "model",
        identity,
        startedAt: observedAt,
        endedAt: observedAt,
        turnCount: 1,
        attributedTokens: modelObservationTokens(observation),
      });
    }
  }
  return segments;
}

function createSessionModelChain(
  observations: readonly IndexedModelObservation[],
): SessionModelChain {
  const sorted = [...observations].sort(compareIndexedModelObservations);
  const first = sorted[0]?.observation;
  if (first === undefined) {
    return rejectModelAnalytics(
      "storage_error",
      "Browser model history fixture could not build a chain.",
    );
  }
  const chainId = observationChainId(first);
  const agentId =
    firstDefinedObservationValue(sorted, ({ agentId: value }) => value) ?? null;
  const parentChainId =
    firstDefinedObservationValue(sorted, ({ parentChainId: value }) => value) ??
    null;
  let attributedTokens = 0;
  let unattributedTokens = 0;
  for (const { observation } of sorted) {
    if (observation.modelId === null) {
      unattributedTokens += modelObservationTokens(observation);
    } else {
      attributedTokens += modelObservationTokens(observation);
    }
  }

  return {
    chainId,
    parentChainId,
    kind:
      chainId === first.sessionId && agentId === null
        ? "parent"
        : "subagent",
    agentId,
    switchCount: getWithinChainSwitchCount(sorted),
    attributedTokens,
    unattributedTokens,
    segments: createSessionModelSegments(sorted),
  };
}

function createSessionModelHistoryFixture(
  args: Record<string, unknown> | undefined,
): SessionModelHistoryResponse {
  const provider = readModelProvider(args?.provider);
  if (provider === null) {
    return rejectModelAnalytics(
      "invalid_provider",
      "Session model history provider is required.",
    );
  }
  const range = readModelRange(args);
  const sessionId = args?.sessionId;
  if (typeof sessionId !== "string" || sessionId.length === 0) {
    return rejectModelAnalytics(
      "not_found",
      "No retained model history exists for this session in the selected range.",
    );
  }
  const scenario = readModelFixtureScenario();
  rejectRequestedModelFixture("detail");
  const observations = indexedModelObservations(
    getScopedModelObservations(
      getModelFixtureObservations(scenario, range, provider),
      range,
      provider,
    ).filter(
      (observation) =>
        observation.provider === provider &&
        observation.sessionId === sessionId,
    ),
  );
  if (
    observations.length === 0 ||
    observations.some(({ observation }) => observation.detailMissing === true)
  ) {
    return rejectModelAnalytics(
      "not_found",
      "No retained model history exists for this session in the selected range.",
    );
  }

  const observationsByChain = new Map<string, IndexedModelObservation[]>();
  for (const indexed of observations) {
    const chainId = observationChainId(indexed.observation);
    const chain = observationsByChain.get(chainId) ?? [];
    chain.push(indexed);
    observationsByChain.set(chainId, chain);
  }
  const chainsWithActivity = Array.from(observationsByChain.values()).map(
    (chain) => ({
      chain: createSessionModelChain(chain),
      firstActivity: Math.min(
        ...chain.map(({ observation }) => observation.observedAt),
      ),
    }),
  );
  chainsWithActivity.sort(
    (left, right) =>
      (left.chain.kind === "parent" ? 0 : 1) -
        (right.chain.kind === "parent" ? 0 : 1) ||
      left.firstActivity - right.firstActivity ||
      compareUnicodeScalars(left.chain.chainId, right.chain.chainId),
  );

  const identityKeys = new Set<string>();
  let attributedTokens = 0;
  let unattributedTokens = 0;
  for (const { observation } of observations) {
    const tokens = modelObservationTokens(observation);
    if (observation.modelId === null) {
      unattributedTokens += tokens;
    } else {
      attributedTokens += tokens;
      identityKeys.add(
        modelIdentityFixtureKey({
          provider: observation.provider,
          modelId: observation.modelId,
        }),
      );
    }
  }
  const chains = chainsWithActivity.map(({ chain }) => chain);

  return {
    provider,
    sessionId,
    displayName:
      firstDefinedObservationValue(observations, ({ displayName }) =>
        displayName === undefined ? undefined : displayName,
      ) ?? sessionId,
    primaryModel: getModelPrimary(observations),
    distinctModels: identityKeys.size,
    switchCount: chains.reduce((total, chain) => total + chain.switchCount, 0),
    attributedTokens,
    unattributedTokens,
    chains,
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
  // session model analytics
  get_model_usage_overview: (args) => createModelUsageOverviewFixture(args),
  get_model_sessions: (args) => createModelSessionsFixture(args),
  get_session_model_history: (args) =>
    createSessionModelHistoryFixture(args),
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
export function handleInvoke(cmd: string, args?: Record<string, unknown>): unknown {
  // Event plugin: let `listen()` resolve with a fake registration; events never fire.
  if (cmd.startsWith("plugin:event|listen")) return listenerSeq++;
  if (cmd.startsWith("plugin:")) return undefined;

  const fixture = fixtures[cmd];
  if (fixture) return fixture(args);

  if (import.meta.env.DEV) console.debug("[mock] unhandled invoke:", cmd);
  return null;
}
