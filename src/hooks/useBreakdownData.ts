import { useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useCachedInvoke } from "./useCachedInvoke";
import {
  breakdownQuery,
  type BreakdownQueryOptions,
} from "./widgetQueryPlan";
import type {
  BreakdownMode,
  HookBreakdown,
  HostBreakdown,
  ProjectBreakdown,
  RangeType,
  SessionBreakdown,
  SkillBreakdown,
} from "../types";

type BreakdownRow =
  | HostBreakdown
  | ProjectBreakdown
  | SessionBreakdown
  | SkillBreakdown
  | HookBreakdown;
interface BreakdownOptions {
  enabled?: boolean;
  skillAllTime?: BreakdownQueryOptions["skillAllTime"];
  skillProvider?: BreakdownQueryOptions["skillProvider"];
  // Feature 009: same All/Codex/Claude + ALL TIME pattern as skills,
  // but tracked independently so the user's last Skills filter doesn't
  // leak into the Hooks breakdown and vice versa.
  hookAllTime?: boolean;
  hookProvider?: BreakdownQueryOptions["hookProvider"];
}

export function useBreakdownData(mode: BreakdownMode, range: RangeType, options: BreakdownOptions = {}) {
  const enabled = options.enabled ?? true;
  const { command, args: commandArgs } = breakdownQuery(mode, range, options);

  const fetchData = useCallback(async () => {
    return invoke<BreakdownRow[]>(command, commandArgs);
  }, [command, commandArgs]);

  const { state, refresh } = useCachedInvoke({
    command,
    args: commandArgs,
    request: fetchData,
    normalizeError: String,
    onError: (error) => console.error("Breakdown data fetch error:", error),
    enabled,
    invalidationEvents: [
      "tokens-updated",
      "sessions-index-updated",
      "transcript-analytics-updated",
      ...(mode === "hooks" ? ["hooks-observed-updated"] : []),
    ],
  });

  return {
    data: state.data ?? [],
    loading: state.initialLoading,
    error: state.error,
    refresh,
  };
}
