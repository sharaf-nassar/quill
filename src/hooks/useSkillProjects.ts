import { useCallback, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { IntegrationProvider, SkillProjectBreakdown } from "../types";

/**
 * Per-skill project-breakdown state used by the Skills tab. One entry per
 * `(skill_name, requestKey)` pair so a skill expanded under one filter set
 * does not show stale rows when the user toggles all-time or switches
 * provider — both flips bump `requestKey` and invalidate the cache entry.
 */
export type SkillProjectsState =
  | { status: "idle" }
  | { status: "loading" }
  | { status: "loaded"; rows: SkillProjectBreakdown[] }
  | { status: "error"; message: string };

const IDLE: SkillProjectsState = { status: "idle" };

interface FetchArgs {
  days: number;
  allTime: boolean;
  provider: IntegrationProvider | null;
  limit: number;
}

function cacheKey(skillName: string, requestKey: string): string {
  return `${skillName}|${requestKey}`;
}

/**
 * Lazy fetcher for `get_skill_project_breakdown`. Caches results per
 * `(skill_name, requestKey)` so collapse/re-expand under the same filter
 * does not refetch. A filter change (different `requestKey`) writes to a
 * fresh cache slot; older slots stay around but are simply ignored by
 * `stateFor`.
 */
export function useSkillProjects() {
  const [state, setState] = useState<Record<string, SkillProjectsState>>({});
  // Mirrors `state` for use inside `fetchProjects` without recreating the
  // callback on every state change. Without this the parent component
  // would re-render every skill row each time a fetch resolved.
  const stateRef = useRef<Record<string, SkillProjectsState>>({});

  const fetchProjects = useCallback(
    async (
      skillName: string,
      requestKey: string,
      args: FetchArgs,
    ): Promise<void> => {
      const key = cacheKey(skillName, requestKey);
      const existing = stateRef.current[key];
      // Dedupe in-flight or completed fetches. On error we allow a retry
      // on the next expand by leaving the slot in `error` rather than
      // `loaded`, so a re-expand triggers a fresh attempt.
      if (
        existing &&
        (existing.status === "loading" || existing.status === "loaded")
      ) {
        return;
      }

      const loading: SkillProjectsState = { status: "loading" };
      stateRef.current = { ...stateRef.current, [key]: loading };
      setState(stateRef.current);

      try {
        const rows = await invoke<SkillProjectBreakdown[]>(
          "get_skill_project_breakdown",
          {
            skillName,
            days: args.days,
            provider: args.provider,
            allTime: args.allTime,
            limit: args.limit,
          },
        );
        const done: SkillProjectsState = { status: "loaded", rows };
        stateRef.current = { ...stateRef.current, [key]: done };
        setState(stateRef.current);
      } catch (e) {
        const failed: SkillProjectsState = {
          status: "error",
          message: String(e),
        };
        stateRef.current = { ...stateRef.current, [key]: failed };
        setState(stateRef.current);
      }
    },
    [],
  );

  const stateFor = useCallback(
    (skillName: string, requestKey: string): SkillProjectsState => {
      return state[cacheKey(skillName, requestKey)] ?? IDLE;
    },
    [state],
  );

  return { stateFor, fetchProjects };
}
