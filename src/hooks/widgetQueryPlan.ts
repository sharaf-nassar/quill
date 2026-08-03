import type {
  BreakdownMode,
  IntegrationProvider,
  RangeType,
} from "../types";

export const WIDGET_DISPLAY_RANGES = ["1h", "6h", "24h", "7d"] as const;

export type WidgetDisplayRange = (typeof WIDGET_DISPLAY_RANGES)[number];
export type HistoryQueryRange = RangeType | "2h" | "12h" | "2d" | "14d";

const RANGE_DURATION_MS: Record<HistoryQueryRange, number> = {
  "1h": 60 * 60 * 1000,
  "2h": 2 * 60 * 60 * 1000,
  "6h": 6 * 60 * 60 * 1000,
  "12h": 12 * 60 * 60 * 1000,
  "24h": 24 * 60 * 60 * 1000,
  "2d": 2 * 24 * 60 * 60 * 1000,
  "7d": 7 * 24 * 60 * 60 * 1000,
  "14d": 14 * 24 * 60 * 60 * 1000,
  "30d": 30 * 24 * 60 * 60 * 1000,
};

export interface WidgetQueryDescriptor {
  readonly command: string;
  readonly args: Readonly<Record<string, unknown>>;
  readonly requestedRange: HistoryQueryRange;
  readonly window: "current" | "comparison";
}

export function queryRangeMs(range: HistoryQueryRange): number {
  return RANGE_DURATION_MS[range];
}

/**
 * Internal history range that contains the displayed window and its equal
 * prior period. The widget never displays 30d, so its legacy fallback stays
 * bounded to 30d without adding an unused 60d backend range.
 */
export function codeInsightsComparisonRange(range: RangeType): HistoryQueryRange {
  switch (range) {
    case "1h":
      return "2h";
    case "6h":
      return "12h";
    case "24h":
      return "2d";
    case "7d":
      return "14d";
    case "30d":
      return "30d";
  }
}

export function codeInsightsHistoryQueries(
  range: RangeType,
): readonly WidgetQueryDescriptor[] {
  const requestedRange = codeInsightsComparisonRange(range);
  return [
    {
      command: "get_token_history",
      args: {
        range: requestedRange,
        hostname: null,
        sessionId: null,
        cwd: null,
      },
      requestedRange,
      window: "comparison",
    },
    {
      command: "get_code_stats_history",
      args: { range: requestedRange },
      requestedRange,
      window: "comparison",
    },
    {
      command: "get_llm_runtime_stats",
      args: { range: requestedRange },
      requestedRange,
      window: "comparison",
    },
  ];
}

export const WEEKLY_TRENDS_HISTORY_RANGE = "14d" as const;

export function weeklyTrendQueries(): readonly WidgetQueryDescriptor[] {
  return [
    {
      command: "get_token_history",
      args: {
        range: WEEKLY_TRENDS_HISTORY_RANGE,
        provider: null,
        hostname: null,
        sessionId: null,
        cwd: null,
      },
      requestedRange: WEEKLY_TRENDS_HISTORY_RANGE,
      window: "comparison",
    },
    {
      command: "get_code_stats_history",
      args: { range: WEEKLY_TRENDS_HISTORY_RANGE },
      requestedRange: WEEKLY_TRENDS_HISTORY_RANGE,
      window: "comparison",
    },
    {
      command: "get_llm_runtime_stats",
      args: { range: WEEKLY_TRENDS_HISTORY_RANGE },
      requestedRange: WEEKLY_TRENDS_HISTORY_RANGE,
      window: "comparison",
    },
  ];
}

export interface BreakdownQueryOptions {
  readonly skillAllTime?: boolean;
  readonly skillProvider?: IntegrationProvider | null;
  readonly hookAllTime?: boolean;
  readonly hookProvider?: IntegrationProvider | null;
}

export interface BreakdownQueryDescriptor {
  readonly command: string;
  readonly args: Readonly<Record<string, unknown>>;
  readonly requestedRange: RangeType;
  readonly mode: BreakdownMode;
}

const SESSION_BREAKDOWN_LIMIT = 200;
const SKILL_BREAKDOWN_LIMIT = 100;
const HOOK_BREAKDOWN_LIMIT = 100;

export function breakdownQuery(
  mode: BreakdownMode,
  range: RangeType,
  options: BreakdownQueryOptions = {},
): BreakdownQueryDescriptor {
  const command =
    mode === "hosts"
      ? "get_host_breakdown"
      : mode === "projects"
        ? "get_project_breakdown"
        : mode === "skills"
          ? "get_skill_breakdown"
          : mode === "hooks"
            ? "get_hook_breakdown"
            : "get_session_breakdown";
  const args =
    mode === "skills"
      ? {
          range,
          provider: options.skillProvider ?? null,
          allTime: options.skillAllTime ?? false,
          limit: SKILL_BREAKDOWN_LIMIT,
        }
      : mode === "hooks"
        ? {
            range,
            provider: options.hookProvider ?? null,
            allTime: options.hookAllTime ?? false,
            limit: HOOK_BREAKDOWN_LIMIT,
          }
        : mode === "sessions"
          ? { range, hostname: null, limit: SESSION_BREAKDOWN_LIMIT }
          : { range };

  return { command, args, requestedRange: range, mode };
}

/** The visible Projects readout needs a second query except in Projects mode. */
export function shouldLoadSecondaryProjects(mode: BreakdownMode): boolean {
  return mode !== "projects";
}

export function usageBreakdownQueries(
  mode: BreakdownMode,
  range: RangeType,
): readonly BreakdownQueryDescriptor[] {
  const selected = breakdownQuery(mode, range);
  return shouldLoadSecondaryProjects(mode)
    ? [selected, breakdownQuery("projects", range)]
    : [selected];
}
