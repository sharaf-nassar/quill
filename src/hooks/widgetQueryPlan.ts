import type {
  BreakdownMode,
  IntegrationProvider,
  RangeType,
} from "../types";

const RANGE_DURATION_MS: Record<RangeType, number> = {
  "1h": 60 * 60 * 1000,
  "6h": 6 * 60 * 60 * 1000,
  "24h": 24 * 60 * 60 * 1000,
  "7d": 7 * 24 * 60 * 60 * 1000,
  "30d": 30 * 24 * 60 * 60 * 1000,
};

interface WidgetQueryDescriptor {
  readonly command: string;
  readonly args: Readonly<Record<string, unknown>>;
}

export function queryRangeMs(range: RangeType): number {
  return RANGE_DURATION_MS[range];
}

export function codeInsightsHistoryQueries(
  range: RangeType,
): readonly WidgetQueryDescriptor[] {
  return [
    {
      command: "get_token_history",
      args: { range, hostname: null, sessionId: null, cwd: null },
    },
    {
      command: "get_code_stats_history",
      args: { range },
    },
  ];
}

export interface BreakdownQueryOptions {
  readonly skillAllTime?: boolean;
  readonly skillProvider?: IntegrationProvider | null;
  readonly hookAllTime?: boolean;
  readonly hookProvider?: IntegrationProvider | null;
}

const SESSION_BREAKDOWN_LIMIT = 200;
const SKILL_BREAKDOWN_LIMIT = 100;
const HOOK_BREAKDOWN_LIMIT = 100;

export function breakdownQuery(
  mode: BreakdownMode,
  range: RangeType,
  options: BreakdownQueryOptions = {},
): WidgetQueryDescriptor {
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

  return { command, args };
}

/** The visible Projects readout needs a second query except in Projects mode. */
export function shouldLoadSecondaryProjects(mode: BreakdownMode): boolean {
  return mode !== "projects";
}

export function usageBreakdownQueries(
  mode: BreakdownMode,
  range: RangeType,
): readonly WidgetQueryDescriptor[] {
  const selected = breakdownQuery(mode, range);
  return shouldLoadSecondaryProjects(mode)
    ? [selected, breakdownQuery("projects", range)]
    : [selected];
}
