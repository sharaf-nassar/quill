// UsageView — the widget's default view and the product's core surface.
//
// Five bands, all scoped by the region's range toggle: the hero chart with its
// headline overlaid, one insight line, a 3x2 readout grid, the switchable
// breakdown, and the totals footer. Everything here is read-only; the widget
// contains no editable settings (spec US8).
//
// Two rules the whole file obeys:
//
//   - **One range, one story.** Chart, delta, insight, every readout, every
//     sparkline and the footer read the same selected range. A band that
//     silently used a different window would be a quiet lie (constitution #1).
//     The insight line rotates across windows, but only under the stated rule
//     in `insightLine.ts` — never by taste and never on a timer.
//   - **Colour means something.** Metric hues appear only on a cell's swatch,
//     sparkline stroke and endpoint; values stay `--text-hi`. Green/red on a
//     delta is assigned by *meaning*, not by arrow direction — a falling
//     tokens-per-LOC is an improvement and renders green even though it points
//     down.
//
// See specs/018-widget-ui-redesign/plan.md#Affected Components.

import { useCallback, useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { AreaChart, bucketTotals, Sparkline, type VizSeries } from "../viz";
import { selectInsightLine } from "./insightLine";
import { useActivitySeries, useProviderTokenSeries } from "../../../hooks/useWidgetSeries";
import { useBreakdownData } from "../../../hooks/useBreakdownData";
import { useCachedInvoke } from "../../../hooks/useCachedInvoke";
import { queryRangeMs, shouldLoadSecondaryProjects } from "../../../hooks/widgetQueryPlan";
import { useCodeInsights } from "../../../hooks/useCodeInsights";
import { useCodeStats } from "../../../hooks/useCodeStats";
import { useContextSavingsStats } from "../../../hooks/useContextSavingsStats";
import { useLlmRuntimeStats } from "../../../hooks/useLlmRuntimeStats";
import { useRetentionCutoff } from "../../../hooks/useRetentionCutoff";
import { openManageWindow } from "../../../lib/manageWindow";
import { IS_MACOS } from "../../../lib/windowChrome";
import {
  formatAdaptiveClockDurationSecs,
  formatClockDurationSecs,
  formatDurationSecs,
  formatExtrapolatedRuntime,
  formatNumber,
  formatObservedSessionAgents,
  formatPiLineageStatus,
  formatRecency,
  isSessionLive,
  resolveSessionMetrics,
} from "../../../utils/format";
import { providerHue, providerLabel, providerTag } from "../../../utils/providers";
import { formatRetentionCutoff } from "../../../utils/retention";
import { formatTokenCount } from "../../../utils/tokens";
import type {
  BreakdownMode,
  CodeStatsHistoryPoint,
  HookBreakdown,
  HostBreakdown,
  IntegrationProvider,
  InsightTrend,
  ProjectBreakdown,
  RangeType,
  SessionBreakdown,
  SkillBreakdown,
  TokenStats,
} from "../../../types";

const CHART_HEIGHT = 118;
const REFRESH_INTERVAL_MS = 60_000;
/** Rows the breakdown shows before it would start dominating the widget. */
const BREAKDOWN_LIMIT = 5;

const MODES: ReadonlyArray<{ id: BreakdownMode; label: string }> = [
  { id: "sessions", label: "Sessions" },
  { id: "projects", label: "Projects" },
  { id: "hosts", label: "Hosts" },
  { id: "skills", label: "Skills" },
  { id: "hooks", label: "Hooks" },
];

/**
 * The Hooks tracking asymmetry, stated wherever hook counts are shown
 * (spec 009 FR-017). Codex does not log per-script hook executions, so its
 * rows are per-event where Claude's are per-script. Copy is fixed — the
 * asymmetry is intrinsic to the sources, not a defect to paper over.
 */
const HOOK_ASYMMETRY_HELP =
  "Claude hooks are tracked per script. Codex hooks are tracked per event " +
  "because Codex doesn't log per-script hook executions.";

/** Counts read as plain integers until they get long enough to need compacting. */
function formatCount(value: number): string {
  return value >= 10_000 ? formatTokenCount(value) : formatNumber(value);
}

/** Signed line count: `+1,923` / `-412` / `0`. */
function formatNetLines(value: number): string {
  if (value > 0) return `+${formatNumber(value)}`;
  return formatNumber(value);
}

/** Last path segment of a project path; null when there is no path at all. */
function projectName(path: string | null | undefined): string | null {
  if (!path) return null;
  const segments = path.split("/").filter(Boolean);
  return segments.length > 0 ? segments[segments.length - 1] : null;
}

/**
 * Axis captions for the chart. Intraday ranges read as clock time; a week
 * reads as weekdays, because eight `HH:MM` labels across seven days say
 * nothing about which day.
 */
function axisLabels(timestamps: readonly string[], range: RangeType): string[] {
  const daily = range === "7d" || range === "30d";
  return timestamps.map((timestamp) => {
    const date = new Date(timestamp);
    if (Number.isNaN(date.getTime())) return "";
    return daily
      ? date.toLocaleDateString(undefined, { weekday: "short" })
      : date.toLocaleTimeString(undefined, {
          hour: "2-digit",
          minute: "2-digit",
          hour12: false,
        });
  });
}

interface Delta {
  readonly text: string;
  readonly tone: "positive" | "negative" | "flat";
  readonly title: string;
}

/**
 * Momentum *inside* the selected range: the back half of the buckets against
 * the front half. Deliberately not a comparison with the previous window —
 * that would need a second query whose window the chart never draws, and a
 * headline delta the user cannot see the evidence for is not evidence.
 */
function rangeMomentum(totals: readonly number[]): Delta | null {
  if (totals.length < 2) return null;
  const split = Math.floor(totals.length / 2);
  const first = totals.slice(0, split).reduce((sum, value) => sum + value, 0);
  const second = totals.slice(split).reduce((sum, value) => sum + value, 0);
  if (first <= 0) return null;
  const change = ((second - first) / first) * 100;
  const rounded = Math.round(Math.abs(change) * 10) / 10;
  const title = "Second half of this range against the first";
  if (rounded < 0.1) return { text: "— 0%", tone: "flat", title };
  return {
    text: `${change > 0 ? "▲" : "▼"} ${rounded}%`,
    tone: change > 0 ? "positive" : "negative",
    title,
  };
}

/**
 * A metric trend rendered by meaning. `upIsGood` carries whether rising is an
 * improvement for this metric; when it is unknown the chip stays neutral
 * rather than guessing.
 */
function trendDelta(trend: InsightTrend | null): Delta | null {
  if (!trend) return null;
  const rounded = Math.round(trend.percentage * 10) / 10;
  const title = "Against the previous window of the same length";
  if (trend.direction === "flat" || rounded < 0.1) {
    return { text: "— 0%", tone: "flat", title };
  }
  const rising = trend.direction === "up";
  const tone =
    trend.upIsGood === null
      ? "flat"
      : trend.upIsGood === rising
        ? "positive"
        : "negative";
  return { text: `${rising ? "▲" : "▼"}${rounded}%`, tone, title };
}

/** Cache hit rate over the range, on the same denominator analytics uses. */
function cacheHitRate(stats: TokenStats | null): number | null {
  if (!stats) return null;
  const denominator =
    stats.total_input + stats.total_cache_creation + stats.total_cache_read;
  if (denominator <= 0) return null;
  return Math.round((stats.total_cache_read / denominator) * 100);
}

/** Per-bucket `lines_added − lines_removed` over the selected range. */
function netLineBuckets(
  history: readonly CodeStatsHistoryPoint[],
  range: RangeType,
  buckets: number,
): number[] {
  const windowMs = queryRangeMs(range);
  const start = Date.now() - windowMs;
  const bucketMs = windowMs / buckets;
  const values = new Array<number>(buckets).fill(0);
  for (const point of history) {
    const timestamp = new Date(point.timestamp).getTime();
    if (!Number.isFinite(timestamp) || timestamp < start) continue;
    const index = Math.min(buckets - 1, Math.floor((timestamp - start) / bucketMs));
    values[index] += point.lines_added - point.lines_removed;
  }
  return values;
}

/**
 * Range-scoped token totals for the footer.
 *
 * Deliberately narrow: it asks for `get_token_stats` and nothing else. The
 * legacy analytics hook this replaces also pulled the full point history and
 * the hostname list, neither of which the widget draws, and a background
 * instrument should not pay for reads it never renders.
 */
function useWidgetTokenStats(range: RangeType) {
  const request = useCallback(
    () =>
      invoke<TokenStats>("get_token_stats", {
        range,
        provider: null,
        hostname: null,
        sessionId: null,
        cwd: null,
      }),
    [range],
  );
  const { state, refresh } = useCachedInvoke({
    command: "get_token_stats",
    args: {
      range,
      provider: null,
      hostname: null,
      sessionId: null,
      cwd: null,
    },
    request,
    normalizeError: String,
    invalidationEvents: ["tokens-updated"],
  });

  useEffect(() => {
    const interval = setInterval(refresh, REFRESH_INTERVAL_MS);
    return () => clearInterval(interval);
  }, [refresh]);

  return { stats: state.data, loading: state.initialLoading };
}

interface ReadoutProps {
  label: string;
  /** Already formatted; `—` when the metric has nothing to report yet. */
  value: string;
  hue: string;
  values: readonly number[];
  delta?: Delta | null;
}

/** One cell of the 3x2 grid: value, hue-swatched label, and its own series. */
function Readout({ label, value, hue, values, delta }: ReadoutProps) {
  return (
    <div className="wg-cell">
      <span className="wg-cell-value">
        {value}
        {delta && (
          <span className="wg-cell-delta" data-tone={delta.tone} title={delta.title}>
            {delta.text}
          </span>
        )}
      </span>
      <span className="wg-cell-key">
        <i className="wg-cell-swatch" style={{ background: hue }} aria-hidden="true" />
        {label}
      </span>
      <Sparkline values={values} color={hue} className="wg-cell-spark" />
    </div>
  );
}

type BreakdownRow =
  | HostBreakdown
  | ProjectBreakdown
  | SessionBreakdown
  | SkillBreakdown
  | HookBreakdown;

interface RowModel {
  key: string;
  /** Present only for modes with a liveness notion (Sessions). */
  live?: boolean;
  /** True only when the live runtime value is known and may use status green. */
  liveActivity?: boolean;
  name: string;
  chip?: { text: string; tone: string };
  /** Dim secondary count, e.g. `41 sess`. */
  meta?: string;
  providerCounts?: ReadonlyArray<{
    provider: Extract<IntegrationProvider, "claude" | "codex" | "pi">;
    count: number;
  }>;
  sessionStats?: {
    runtime: string;
    runtimeLabel: string;
    turns: string;
    turnsLabel: string;
  };
  nameLabel?: string;
  chipLabel?: string;
  parentSessionId?: string;
  lineageStatus?: ReturnType<typeof formatPiLineageStatus>;
  agentSummary?: {
    count: string;
    countLabel: string;
    runtime: string;
    runtimeLabel: string;
  };
  agents?: ReturnType<typeof formatObservedSessionAgents>;
  linkedSessions?: ReadonlyArray<{ sessionId: string; modelId: string | null }>;
  value: string;
  valueLabel?: string;
  activity: string;
  activityLabel?: string;
  title: string;
}

function ProviderCounts({ counts }: { counts: NonNullable<RowModel["providerCounts"]> }) {
  return (
    <span className="wg-row-provider-counts wg-num" aria-label="Provider counts">
      {counts.map(({ provider, count }) => (
        <span
          className="wg-row-provider-count"
          data-provider={provider}
          aria-label={`${providerLabel(provider)} ${formatCount(count)}`}
          title={`${providerLabel(provider)} ${formatCount(count)}`}
          key={provider}
        >
          <span aria-hidden="true">
            {provider === "claude" ? "CL" : provider === "codex" ? "CX" : "PI"}
          </span>{" "}
          {formatCount(count)}
        </span>
      ))}
    </span>
  );
}

function AgentIcon({ className = "" }: { className?: string }) {
  return (
    <svg
      className={`wg-row-agent-icon${className ? ` ${className}` : ""}`}
      viewBox="0 0 12 12"
      fill="none"
      aria-hidden="true"
    >
      <path d="M6 2.75V1.5" />
      <rect x="1.5" y="2.75" width="9" height="7.5" rx="2" />
      <circle cx="4.25" cy="6.5" r="0.65" fill="currentColor" stroke="none" />
      <circle cx="7.75" cy="6.5" r="0.65" fill="currentColor" stroke="none" />
    </svg>
  );
}

function ActiveAgentRail({ agents }: Pick<RowModel, "agents">) {
  if (!agents || agents.length === 0) return null;
  return (
    <div className="wg-row-agent-rail" role="list" aria-label="Currently open agents">
      <AgentIcon className="wg-row-agent-live-icon" />
      {agents.map((agent) => (
        <span
          className="wg-row-agent wg-row-datum"
          key={agent.agentId}
          role="listitem"
          aria-label={`Currently open agent: ${agent.ariaLabel}`}
          data-tooltip={agent.ariaLabel}
        >
          <span className="wg-row-agent-model" aria-hidden="true">{agent.model}</span>
          <span className="wg-row-agent-time wg-num" aria-hidden="true">{agent.runtime}</span>
        </span>
      ))}
    </div>
  );
}

function LinkIcon({ className = "" }: { className?: string }) {
  return (
    <svg
      className={`wg-row-link-icon${className ? ` ${className}` : ""}`}
      viewBox="0 0 12 12"
      fill="none"
      aria-hidden="true"
    >
      <path d="M4.75 7.25 7.25 4.75M4 8.75H3a2 2 0 0 1 0-4h2M8 3.25h1a2 2 0 1 1 0 4H7" />
    </svg>
  );
}

function LiveLinkedSessionRail({
  sessions,
}: {
  sessions: RowModel["linkedSessions"];
}) {
  if (!sessions || sessions.length === 0) return null;
  const countLabel = `${sessions.length} live linked ${sessions.length === 1 ? "session" : "sessions"}`;
  return (
    <div className="wg-row-linked-rail" role="list" aria-label="Live linked sessions">
      <LinkIcon className="wg-row-linked-live-icon" />
      <span className="wg-row-linked-count" aria-label={countLabel}>
        {countLabel}
      </span>
      {sessions.map((session) => (
        <span
          className="wg-row-linked-session wg-row-datum"
          key={session.sessionId}
          role="listitem"
          aria-label={`Live linked session ${session.sessionId}${session.modelId ? `, model ${session.modelId}` : ""}`}
          data-tooltip={session.sessionId}
        >
          {session.modelId ?? session.sessionId.slice(0, 8)}
        </span>
      ))}
    </div>
  );
}

function SessionMetrics({ row }: { row: RowModel }) {
  if (!row.sessionStats) return null;
  return (
    <>
      <span
        className="wg-row-session-turns wg-row-datum"
        data-tooltip="Main-session turns"
        aria-label={row.sessionStats.turnsLabel}
      >
        <svg
          className="wg-row-turn-icon"
          viewBox="0 0 10 10"
          fill="none"
          aria-hidden="true"
          focusable="false"
        >
          <path d="M1.5 1.5h7v5h-4L2 8V6.5h-.5z" />
        </svg>
        {row.sessionStats.turns}
      </span>
      <span
        className="wg-row-datum wg-row-session-runtime"
        data-live={row.liveActivity ? "true" : undefined}
        data-tooltip="Total runtime"
        aria-label={row.sessionStats.runtimeLabel}
      >
        {row.sessionStats.runtime}
      </span>
    </>
  );
}

function RowName({ row }: { row: RowModel }) {
  return row.nameLabel ? (
    <span
      className="wg-row-name-tip wg-row-datum"
      data-tooltip="Session"
      aria-label={row.nameLabel}
    >
      <span className="wg-row-name" aria-hidden="true">
        {row.name}
      </span>
    </span>
  ) : (
    <span className="wg-row-name">{row.name}</span>
  );
}

function SessionIdentity({ row }: { row: RowModel }) {
  const linkedCount = row.linkedSessions?.length ?? 0;
  return (
    <span className="wg-row-session-identity">
      <RowName row={row} />
      {row.chip && (
        <span
          className="wg-row-session-provider wg-row-datum"
          data-tone={row.chip.tone}
          data-tooltip="Provider"
          aria-label={row.chipLabel}
        >
          {row.chip.text}
        </span>
      )}
      {row.parentSessionId && (
        <span
          className="wg-row-session-parent wg-row-datum"
          data-tooltip="Parent Pi session"
          aria-label={`Parent Pi session ${row.parentSessionId}`}
        >
          ↳ {row.parentSessionId.slice(0, 8)}
        </span>
      )}
      {row.lineageStatus && (
        <span
          className="wg-row-session-parent wg-row-datum"
          data-tooltip={row.lineageStatus.detail}
          aria-label={row.lineageStatus.detail}
        >
          {row.lineageStatus.label}
        </span>
      )}
      {linkedCount > 0 && (
        <span
          className="wg-row-linked-summary wg-row-datum"
          data-tooltip="Live linked sessions"
          aria-label={`${linkedCount} live linked ${linkedCount === 1 ? "session" : "sessions"}`}
        >
          <LinkIcon /> {linkedCount}
        </span>
      )}
    </span>
  );
}

function sessionRow(row: SessionBreakdown, nowMs: number): RowModel {
  const name = projectName(row.project) ?? row.session_id.slice(0, 8);
  const recovering =
    row.provider === "pi" &&
    row.pi_lineage?.kind === "unresolved" &&
    row.pi_lineage.reason === "recovering";
  const live = !recovering && isSessionLive(row.last_active, row.ended_at, nowMs);
  const liveActivity = live && row.current_turn_runtime_active;
  const metrics = resolveSessionMetrics(
    formatTokenCount(row.total_tokens),
    `${formatNumber(row.turn_count)} turns`,
    row.observed_only,
	row.provider,
	live,
  );
  const totalRuntime = formatExtrapolatedRuntime(
    row.active_runtime_secs,
    row.runtime_as_of_ms,
    row.active_runtime_rate,
    nowMs,
  );
  const totalRuntimeClock = formatExtrapolatedRuntime(
    row.active_runtime_secs,
    row.runtime_as_of_ms,
    row.active_runtime_rate,
    nowMs,
    formatClockDurationSecs,
  );
  const displayedTurnCount = row.turn_count + (liveActivity ? 1 : 0);
  const turnCount = metrics.turns === null && !liveActivity
    ? "—"
    : formatNumber(displayedTurnCount);
  const runtimeLabel =
    totalRuntime === "—"
      ? "Total session runtime unavailable"
      : `Total session runtime ${totalRuntime}`;
  const turnsLabel = turnCount === "—"
    ? "Main-session turn count unavailable"
    : liveActivity
      ? `${turnCount} main-session ${displayedTurnCount === 1 ? "turn" : "turns"} including active turn`
      : `${turnCount} completed main-session ${displayedTurnCount === 1 ? "turn" : "turns"}`;
  const currentTurnRuntime = live
    ? formatExtrapolatedRuntime(
        row.current_turn_runtime_secs,
        row.runtime_as_of_ms,
        row.current_turn_runtime_active ? 1 : 0,
        nowMs,
      )
    : "—";
  const activity = live
    ? formatExtrapolatedRuntime(
        row.current_turn_runtime_secs,
        row.runtime_as_of_ms,
        row.current_turn_runtime_active ? 1 : 0,
        nowMs,
        formatAdaptiveClockDurationSecs,
      )
    : formatRecency(row.last_active, nowMs);
  const activityLabel = live
    ? currentTurnRuntime === "—"
      ? "Current-turn runtime unavailable"
      : `${row.current_turn_runtime_active ? "Current-turn active runtime" : "Current-turn runtime"} ${currentTurnRuntime}`
    : `Last active ${activity}`;
  const agents = live
    ? formatObservedSessionAgents(
        row.provider,
        row.observed_agents,
        row.runtime_as_of_ms,
        nowMs,
      )
    : [];
  const hasAgentTotals =
    (row.agent_count !== null && row.agent_count > 0) ||
    (row.agent_runtime_secs !== null && row.agent_runtime_secs > 0);
  const activeAgentRuntimeRate = (row.observed_agents ?? []).filter(
    (agent) => agent.runtime_secs !== null && agent.runtime_active,
  ).length;
  const agentRuntime = formatExtrapolatedRuntime(
    row.agent_runtime_secs,
    row.runtime_as_of_ms,
    activeAgentRuntimeRate,
    nowMs,
  );
  return {
    key: `${row.provider}:${row.hostname}:${row.session_id}`,
    live,
    liveActivity,
    name,
    nameLabel: `Session ${name} on ${row.hostname}`,
    sessionStats: {
      runtime: totalRuntimeClock,
      runtimeLabel,
      turns: turnCount,
      turnsLabel,
    },
    chip: { text: providerTag(row.provider), tone: row.provider },
    chipLabel: `Provider ${providerTag(row.provider)}`,
    parentSessionId: row.provider === "pi" ? row.parent_session_id ?? undefined : undefined,
    lineageStatus:
      row.provider === "pi" && row.pi_lineage?.kind === "unresolved"
        ? formatPiLineageStatus(row.pi_lineage.reason)
        : undefined,
    agentSummary:
      hasAgentTotals || agents.length > 0
        ? {
            count: row.agent_count === null ? "—" : formatNumber(row.agent_count),
            countLabel:
              row.agent_count === null
                ? "Total agent count unavailable"
                : `${formatNumber(row.agent_count)} total agents run during this session`,
            runtime: formatExtrapolatedRuntime(
              row.agent_runtime_secs,
              row.runtime_as_of_ms,
              activeAgentRuntimeRate,
              nowMs,
              formatClockDurationSecs,
            ),
            runtimeLabel:
              agentRuntime === "—"
                ? "Total agent runtime unavailable"
                : `Total agent active runtime ${agentRuntime}`,
          }
        : undefined,
    agents,
    linkedSessions:
      row.provider === "pi"
        ? (row.live_linked_sessions ?? []).map((session) => ({
            sessionId: session.session_id,
            modelId: session.model_id,
          }))
        : undefined,
    value: metrics.tokens,
    valueLabel:
      metrics.tokens === "—"
        ? "Total session tokens unavailable"
        : `Total session tokens ${metrics.tokens}`,
    activity,
    activityLabel,
    title: [name, row.hostname, metrics.turns].filter(Boolean).join(" · "),
  };
}

function buildRows(
  mode: BreakdownMode,
  rows: readonly BreakdownRow[],
  nowMs: number,
): RowModel[] {
  return rows.slice(0, BREAKDOWN_LIMIT).map((row) => {
    if ("session_id" in row) return sessionRow(row, nowMs);
    if ("skill_name" in row) {
      return {
        key: row.skill_name,
        name: row.skill_name,
        providerCounts: [
          { provider: "claude", count: row.claude_count },
          { provider: "codex", count: row.codex_count },
          { provider: "pi", count: row.pi_count },
        ],
        value: `${formatCount(row.total_count)} uses`,
        activity: formatRecency(row.last_used, nowMs),
        title: `${row.skill_name} · ${formatNumber(row.project_count)} projects`,
      };
    }
    if ("hook_identity" in row) {
      return {
        key: `${row.hook_identity}:${row.hook_event}`,
        name: row.hook_identity,
        providerCounts: [
          { provider: "claude", count: row.claude_count },
          { provider: "codex", count: row.codex_count },
          { provider: "pi", count: row.pi_count },
        ],
        chip: row.is_quill ? { text: "QUILL", tone: "quill" } : undefined,
        value: `${formatCount(row.total_count)} fires`,
        activity: formatRecency(row.last_fired_at, nowMs),
        title: `${row.hook_identity} · ${row.hook_event}`,
      };
    }
    if ("project" in row) {
      const name = projectName(row.project) ?? row.project;
      return {
        key: `${row.project}@${row.hostname}`,
        name,
        meta: `${formatCount(row.session_count)} sess`,
        value: formatTokenCount(row.total_tokens),
        activity: formatRecency(row.last_active, nowMs),
        title: `${row.project} · ${row.hostname}`,
      };
    }
    return {
      key: row.hostname,
      name: row.hostname,
      meta: `${formatCount(row.turn_count)} turns`,
      value: formatTokenCount(row.total_tokens),
      activity: formatRecency(row.last_active, nowMs),
      title: row.hostname,
    };
  });
}

function emptyBreakdownLabel(mode: BreakdownMode): string {
  if (mode === "skills") return "No skill usage recorded";
  if (mode === "hooks") return "No hook fires in this range";
  return `No ${mode} in this range`;
}

/** `⌘M` on macOS, `Ctrl+M` everywhere else — the accelerator main.tsx binds. */
function manageAccelerator(): string {
  return IS_MACOS ? "⌘M" : "Ctrl+M";
}

export interface UsageViewProps {
  range: RangeType;
}

function UsageView({ range }: UsageViewProps) {
  const [mode, setMode] = useState<BreakdownMode>("sessions");
  const [nowMs, setNowMs] = useState(() => Date.now());

  const tokenSeries = useProviderTokenSeries(range);
  const activity = useActivitySeries(range);
  const tokens = useWidgetTokenStats(range);
  const runtime = useLlmRuntimeStats(range);
  const insights = useCodeInsights(range, runtime);
  const code = useCodeStats(range);
  const savings = useContextSavingsStats(range);
  const retention = useRetentionCutoff();
  const breakdown = useBreakdownData(mode, range);
  // Projects is a readout metric as well as a breakdown mode, so its count is
  // read separately only while another breakdown is selected. In Projects
  // mode the selected request supplies both regions, avoiding two subscribers
  // and two loading/error paths for the same command and arguments.
  const secondaryProjects = useBreakdownData("projects", range, {
    enabled: shouldLoadSecondaryProjects(mode),
  });
  const projects = mode === "projects" ? breakdown : secondaryProjects;

  // Recency and live runtime labels advance without polling the backend.
  useEffect(() => {
    const interval = setInterval(() => setNowMs(Date.now()), 1_000);
    return () => clearInterval(interval);
  }, []);

  const chart = useMemo(() => {
    const response = tokenSeries.data;
    if (!response) return null;
    const series: VizSeries[] = response.series.map((entry) => ({
      id: entry.provider,
      label: providerTag(entry.provider),
      color: providerHue(entry.provider),
      values: entry.values,
      fillOpacity: entry.provider === "claude" ? 0.09 : 0.16,
    }));
    const totals = bucketTotals(response.series);
    return {
      series,
      totals: response.total_tokens,
      delta: rangeMomentum(totals),
      labels: axisLabels(response.timestamps, range),
      summary: response.series
        .map((entry) => `${providerTag(entry.provider)} ${formatTokenCount(entry.total_tokens)}`)
        .join(", "),
    };
  }, [tokenSeries.data, range]);

  const netLines = useMemo(
    () => netLineBuckets(code.history, range, 8),
    [code.history, range],
  );

  const rows = useMemo(
    () => buildRows(mode, breakdown.data, nowMs),
    [mode, breakdown.data, nowMs],
  );

  const liveCount = useMemo(
    () =>
      mode === "sessions"
        ? breakdown.data.filter((row) => {
            if (!("session_id" in row)) return false;
            return isSessionLive(row.last_active, row.ended_at, nowMs);
          }).length
        : 0,
    [mode, breakdown.data, nowMs],
  );

  const cachePercent = cacheHitRate(tokens.stats);
  const savingsSummary = savings.data?.summary ?? null;
  const reusePercent =
    savingsSummary && (savingsSummary.sourcesPreserved ?? 0) > 0
      ? Math.round((savingsSummary.retentionRatio ?? 0) * 100)
      : null;

  // Which insight this window gets is decided by the rule in `insightLine.ts`,
  // not by whichever source happened to answer first.
  const insight = useMemo(
    () =>
      selectInsightLine({
        savings: {
          tokensSaved: savingsSummary?.tokensSavedEst ?? null,
          reusePercent,
          loading: savings.loading,
        },
        cache: {
          tokensFromCache: tokens.stats?.total_cache_read ?? null,
          percentOfInput: cachePercent,
          loading: tokens.loading,
        },
        providers: {
          totals:
            tokenSeries.data?.series.map((entry) => ({
              label: providerTag(entry.provider),
              tokens: entry.total_tokens,
            })) ?? null,
          loading: tokenSeries.loading,
        },
      }),
    [
      savingsSummary,
      reusePercent,
      savings.loading,
      tokens.stats,
      tokens.loading,
      cachePercent,
      tokenSeries.data,
      tokenSeries.loading,
    ],
  );

  return (
    <>
      <div className="wg-usage-band">
        {tokenSeries.loading && !chart ? (
          <div
            className="wg-skeleton wg-skeleton-block"
            style={{ height: `${CHART_HEIGHT}px` }}
            aria-hidden="true"
          />
        ) : tokenSeries.error && !chart ? (
          <div className="wg-state wg-state-error">
            <span className="wg-state-lamp" aria-hidden="true" />
            Token series unavailable
          </div>
        ) : (
          <AreaChart
            series={chart?.series ?? []}
            xLabels={chart?.labels}
            height={CHART_HEIGHT}
            ariaLabel={`Token usage for the selected range: ${formatTokenCount(
              chart?.totals ?? 0,
            )} total${chart?.summary ? ` — ${chart.summary}` : ""}`}
            emptyLabel="No tokens recorded in this range"
            overlay={
              <>
                {chart?.delta && (
                  <span
                    className="wg-usage-delta"
                    data-tone={chart.delta.tone}
                    title={chart.delta.title}
                  >
                    {chart.delta.text}
                  </span>
                )}
                <span className="wg-usage-headline">
                  <span className="wg-usage-big">
                    {formatTokenCount(chart?.totals ?? 0)}
                  </span>
                  <span className="wg-usage-unit">tokens</span>
                </span>
              </>
            }
          />
        )}

        {/* One insight for this window, chosen by the rotation rule and absent
            rather than zeroed when no candidate has anything true to say. */}
        {insight && (
          <p className="wg-insight wg-num">
            <span className="wg-insight-glyph" aria-hidden="true">
              ◆
            </span>
            <span>
              <b>{insight.headline}</b>
              {insight.detail !== null && ` — ${insight.detail}`}
            </span>
          </p>
        )}
      </div>

      <div className="wg-grid wg-num">
        <Readout
          label="Runtime"
          value={runtime.totalRuntimeSecs === null ? "—" : formatDurationSecs(runtime.totalRuntimeSecs)}
          hue="var(--metric-runtime)"
          values={runtime.sparkline.map((point) => point.value)}
        />
        <Readout
          label="Tok / LOC"
          value={
            insights.efficiency.tokensPerLoc === null
              ? "—"
              : formatNumber(insights.efficiency.tokensPerLoc)
          }
          hue="var(--metric-tok-per-loc)"
          values={insights.efficiency.sparkline.map((point) => point.value)}
          delta={trendDelta(insights.efficiency.trend)}
        />
        <Readout
          label="LOC / hr"
          value={
            insights.velocity.locPerHour === null
              ? "—"
              : formatNumber(insights.velocity.locPerHour)
          }
          hue="var(--metric-loc-per-hr)"
          values={insights.velocity.sparkline.map((point) => point.value)}
          delta={trendDelta(insights.velocity.trend)}
        />
        <Readout
          label="Sessions"
          value={runtime.loading ? "—" : formatNumber(runtime.sessionCount)}
          hue="var(--metric-sessions)"
          values={activity.data?.session_counts ?? []}
        />
        <Readout
          label="Projects"
          value={projects.loading ? "—" : formatNumber(projects.data.length)}
          hue="var(--metric-projects)"
          values={activity.data?.project_counts ?? []}
        />
        <Readout
          label="Net lines"
          value={code.stats === null ? "—" : formatNetLines(code.stats.net_change)}
          hue="var(--metric-net-lines)"
          values={netLines}
        />
      </div>

      <div className="wg-rule" />

      <section className="wg-breakdown" aria-label="Activity breakdown">
        <div className="wg-breakdown-head">
          <div className="wg-toggles" role="group" aria-label="Breakdown mode">
            {MODES.map((entry) => (
              <button
                key={entry.id}
                type="button"
                className="wg-toggle"
                aria-pressed={entry.id === mode}
                onClick={() => setMode(entry.id)}
              >
                {entry.label}
              </button>
            ))}
          </div>
          {mode === "sessions" && liveCount > 0 && (
            <span className="wg-breakdown-count wg-num">{liveCount} LIVE</span>
          )}
          {mode === "hooks" && (
            <button
              type="button"
              className="wg-breakdown-help"
              aria-label="About Claude and Codex hook tracking"
              title={HOOK_ASYMMETRY_HELP}
            >
              <span aria-hidden="true">?</span>
            </button>
          )}
        </div>

        {mode === "sessions" && retention.cutoff && (
          <p
            className="wg-breakdown-note"
            role="note"
            title="Tool activity recorded before this date was pruned."
          >
            Retention · tool activity before {formatRetentionCutoff(retention.cutoff)} was pruned
          </p>
        )}

        {breakdown.error ? (
          <div className="wg-state wg-state-error">
            <span className="wg-state-lamp" aria-hidden="true" />
            Breakdown unavailable
          </div>
        ) : breakdown.loading ? (
          <div className="wg-breakdown-pending" aria-hidden="true">
            <div className="wg-skeleton wg-skeleton-line" />
            <div className="wg-skeleton wg-skeleton-line" />
            <div className="wg-skeleton wg-skeleton-line" />
          </div>
        ) : rows.length === 0 ? (
          <div className="wg-state">
            <span className="wg-state-lamp" aria-hidden="true" />
            {emptyBreakdownLabel(mode)}
          </div>
        ) : (
          <ul className="wg-rows">
            {rows.map((row) => (
              <li
                className="wg-row"
                key={row.key}
                data-idle={row.live === false ? "true" : undefined}
                data-session={row.sessionStats ? "true" : undefined}
                title={row.sessionStats ? undefined : row.title}
              >
                <div className="wg-row-main">
                  {row.live !== undefined && (
                    <span
                      className="wg-row-dot"
                      data-live={row.live ? "true" : "false"}
                      aria-hidden="true"
                    />
                  )}
                  {row.sessionStats ? (
                    <SessionIdentity row={row} />
                  ) : (
                    <RowName row={row} />
                  )}
                  {row.meta && <span className="wg-row-meta wg-num">{row.meta}</span>}
                  {row.providerCounts && <ProviderCounts counts={row.providerCounts} />}
                  {row.sessionStats && (
                    <span className="wg-row-session-stats wg-num">
                      {row.agentSummary && (
                        <span className="wg-row-agent-summary">
                          <span
                            className="wg-row-datum"
                            data-tooltip="Total agents"
                            aria-label={row.agentSummary.countLabel}
                          >
                            {row.agentSummary.count}
                          </span>
                          <AgentIcon />
                          <span
                            className="wg-row-agent-total-time wg-row-datum"
                            data-tooltip="Total agent runtime"
                            aria-label={row.agentSummary.runtimeLabel}
                          >
                            {row.agentSummary.runtime}
                          </span>
                        </span>
                      )}
                      <SessionMetrics row={row} />
                    </span>
                  )}
                  {row.chip && !row.sessionStats && (
                    <span
                      className={`wg-row-chip${row.chipLabel ? " wg-row-datum" : ""}`}
                      data-tone={row.chip.tone}
                      data-tooltip={row.chipLabel ? "Provider" : undefined}
                      aria-label={row.chipLabel}
                    >
                      {row.chip.text}
                    </span>
                  )}
                  <span
                    className={`wg-row-value wg-num${row.valueLabel ? " wg-row-datum" : ""}`}
                    data-tooltip={row.valueLabel ? "Tokens" : undefined}
                    aria-label={row.valueLabel}
                  >
                    {row.value}
                  </span>
                  <span
                    className={`wg-row-time wg-num${row.activityLabel ? " wg-row-datum" : ""}`}
                    data-live={row.liveActivity ? "true" : undefined}
                    data-tooltip={
                      row.activityLabel
                        ? row.live
                          ? "Current-turn runtime"
                          : "Last active"
                        : undefined
                    }
                    aria-label={row.activityLabel}
                  >
                    {row.activity}
                  </span>
                </div>
                <ActiveAgentRail agents={row.agents} />
                <LiveLinkedSessionRail sessions={row.linkedSessions} />
              </li>
            ))}
          </ul>
        )}
      </section>

      <div className="wg-rule" />

      <footer className="wg-footer wg-num">
        <span className="wg-footer-kv">
          In <b>{tokens.stats ? formatTokenCount(tokens.stats.total_input) : "—"}</b>
        </span>
        <span className="wg-footer-kv">
          Out <b>{tokens.stats ? formatTokenCount(tokens.stats.total_output) : "—"}</b>
        </span>
        <span className="wg-footer-kv">
          Cache <b>{cachePercent === null ? "—" : `${cachePercent}%`}</b>
        </span>
        <button
          type="button"
          className="wg-manage"
          onClick={() => void openManageWindow()}
        >
          Manage
          <span className="wg-kbd" aria-hidden="true">
            {manageAccelerator()}
          </span>
        </button>
      </footer>
    </>
  );
}

export default UsageView;
