import { useCallback, useMemo } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useCachedInvoke } from "./useCachedInvoke";
import type {
  BreakdownMode,
  HookBreakdown,
  HostBreakdown,
  IntegrationProvider,
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
  skillAllTime?: boolean;
  skillProvider?: IntegrationProvider | null;
  // Feature 009: same All/Codex/Claude + ALL TIME pattern as skills,
  // but tracked independently so the user's last Skills filter doesn't
  // leak into the Hooks breakdown and vice versa.
  hookAllTime?: boolean;
  hookProvider?: IntegrationProvider | null;
}

const SESSION_BREAKDOWN_LIMIT = 200;
const SKILL_BREAKDOWN_LIMIT = 100;
const HOOK_BREAKDOWN_LIMIT = 100;

export function useBreakdownData(mode: BreakdownMode, range: RangeType, options: BreakdownOptions = {}) {
  const skillAllTime = options.skillAllTime ?? false;
  const skillProvider = options.skillProvider ?? null;
  const hookAllTime = options.hookAllTime ?? false;
  const hookProvider = options.hookProvider ?? null;
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
  const args = useMemo(
    () =>
      mode === "skills"
        ? {
            range,
            provider: skillProvider,
            allTime: skillAllTime,
            limit: SKILL_BREAKDOWN_LIMIT,
          }
        : mode === "hooks"
          ? {
              range,
              provider: hookProvider,
              allTime: hookAllTime,
              limit: HOOK_BREAKDOWN_LIMIT,
            }
          : mode === "sessions"
            ? { range, hostname: null, limit: SESSION_BREAKDOWN_LIMIT }
            : { range },
    [hookAllTime, hookProvider, mode, range, skillAllTime, skillProvider],
  );

  const fetchData = useCallback(async () => {
    return invoke<BreakdownRow[]>(command, args);
  }, [args, command]);

  const { state, refresh } = useCachedInvoke({
    command,
    args,
    request: fetchData,
    normalizeError: String,
    onError: (error) => console.error("Breakdown data fetch error:", error),
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
