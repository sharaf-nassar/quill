import { useCallback, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  sessionRefKey,
  type IntegrationProvider,
  type SessionRef,
  type SubagentNode,
} from "../types";

/**
 * Per-session sub-agent tree state used by the Sessions tab. One entry per
 * `(provider, session_id)` pair, keyed by `sessionRefKey`.
 */
export interface SessionSubagentState {
  nodes: SubagentNode[];
  loading: boolean;
  error: string | null;
  /** True once a fetch (success or failure) has completed for this session. */
  loaded: boolean;
}

const EMPTY_STATE: SessionSubagentState = {
  nodes: [],
  loading: false,
  error: null,
  loaded: false,
};

/**
 * Lazy fetcher for `get_session_subagent_tree`. Caches results per
 * `(provider, session_id)` so collapse/re-expand does not refetch. The
 * top-level Sessions breakdown stays the canonical source for parent-row
 * data; this hook is only ever consulted after a row is expanded.
 */
export function useSessionSubagents() {
  const [state, setState] = useState<Record<string, SessionSubagentState>>({});
  // Mirrors `state` for use inside `fetchTree` without recreating the
  // callback on every state change. Without this the component would
  // re-render the entire tree each time a row toggles.
  const stateRef = useRef<Record<string, SessionSubagentState>>({});

  const fetchTree = useCallback(
    async (provider: IntegrationProvider, sessionId: string): Promise<void> => {
      const ref: SessionRef = { provider, session_id: sessionId };
      const key = sessionRefKey(ref);
      const existing = stateRef.current[key];
      // Skip refetch if we already have data (success) or an in-flight
      // request. On error we allow a retry on the next expand.
      if (existing && (existing.loading || existing.loaded)) return;

      const next: SessionSubagentState = {
        nodes: existing?.nodes ?? [],
        loading: true,
        error: null,
        loaded: existing?.loaded ?? false,
      };
      stateRef.current = { ...stateRef.current, [key]: next };
      setState(stateRef.current);

      try {
        const nodes = await invoke<SubagentNode[]>(
          "get_session_subagent_tree",
          { provider, sessionId },
        );
        const done: SessionSubagentState = {
          nodes,
          loading: false,
          error: null,
          loaded: true,
        };
        stateRef.current = { ...stateRef.current, [key]: done };
        setState(stateRef.current);
      } catch (e) {
        const failed: SessionSubagentState = {
          nodes: [],
          loading: false,
          error: String(e),
          loaded: false,
        };
        stateRef.current = { ...stateRef.current, [key]: failed };
        setState(stateRef.current);
      }
    },
    [],
  );

  const getState = useCallback(
    (provider: IntegrationProvider, sessionId: string): SessionSubagentState => {
      const key = sessionRefKey({ provider, session_id: sessionId });
      return state[key] ?? EMPTY_STATE;
    },
    [state],
  );

  return { fetchTree, getState };
}
