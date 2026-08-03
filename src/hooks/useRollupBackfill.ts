import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useCallback, useEffect, useState } from "react";
import type {
  RebuildRollupResult,
  RollupBackfillFinishedEvent,
  RollupBackfillProgressEvent,
  RollupBackfillTarget,
} from "../types";

export type RollupBackfillUiState =
  | { kind: "idle" }
  | { kind: "starting"; previousRunId: number }
  | { kind: "running"; progress: RollupBackfillProgressEvent }
  | { kind: "refused"; reason: string; runId: number | null }
  | {
      kind: "error";
      runId: number;
      detail: string;
      progress: RollupBackfillProgressEvent | null;
    }
  | {
      kind: "completed";
      runId: number;
      progress: RollupBackfillProgressEvent | null;
    };

const IDLE: RollupBackfillUiState = { kind: "idle" };

function progressFrom(state: RollupBackfillUiState) {
  if (state.kind === "running" || state.kind === "error" || state.kind === "completed") {
    return state.progress;
  }
  return null;
}

function runIdFrom(state: RollupBackfillUiState): number | null {
  if (state.kind === "running") return state.progress.runId;
  if (state.kind === "error" || state.kind === "completed") return state.runId;
  if (state.kind === "refused") return state.runId;
  if (state.kind === "starting") return state.previousRunId;
  return null;
}

function errorMessage(error: unknown): string {
  if (error instanceof Error && error.message.trim() !== "") return error.message;
  const message = String(error).trim();
  return message === "" ? "The index rebuild could not start." : message;
}

/** Committed rollup progress shared by Settings and the Models widget. */
export function useRollupBackfill(target: RollupBackfillTarget) {
  const [state, setState] = useState<RollupBackfillUiState>(IDLE);

  useEffect(() => {
    let disposed = false;
    let stopProgress: (() => void) | null = null;
    let stopFinished: (() => void) | null = null;

    void listen<RollupBackfillProgressEvent>(
      "rollup-backfill-progress",
      ({ payload }) => {
        if (disposed || payload.target !== target) return;
        setState((current) => {
          if (current.kind === "starting" && payload.runId <= current.previousRunId) {
            return current;
          }
          const currentRunId = runIdFrom(current);
          if (currentRunId !== null && payload.runId < currentRunId) return current;
          const accepted = progressFrom(current);
          if (
            accepted !== null &&
            payload.runId === accepted.runId &&
            payload.rowsDone < accepted.rowsDone
          ) {
            return current;
          }
          return { kind: "running", progress: payload };
        });
      },
    )
      .then((unlisten) => {
        if (disposed) unlisten();
        else stopProgress = unlisten;
      })
      .catch((error: unknown) => {
        if (!disposed) console.error("Rollup progress listener failed:", error);
      });

    void listen<RollupBackfillFinishedEvent>(
      "rollup-backfill-finished",
      ({ payload }) => {
        if (disposed || payload.target !== target) return;
        setState((current) => {
          if (current.kind === "starting" && payload.runId <= current.previousRunId) {
            return current;
          }
          const currentRunId = runIdFrom(current);
          if (currentRunId !== null && payload.runId < currentRunId) return current;
          const acceptedProgress = progressFrom(current);
          const progress =
            acceptedProgress?.runId === payload.runId ? acceptedProgress : null;
          if (payload.status === "completed") {
            return { kind: "completed", runId: payload.runId, progress };
          }
          return {
            kind: "error",
            runId: payload.runId,
            detail:
              payload.detail ??
              (payload.status === "interrupted"
                ? "Index build stopped. Rebuild to continue."
                : "Index build failed. Rebuild to retry."),
            progress,
          };
        });
      },
    )
      .then((unlisten) => {
        if (disposed) unlisten();
        else stopFinished = unlisten;
      })
      .catch((error: unknown) => {
        if (!disposed) console.error("Rollup finished listener failed:", error);
      });

    return () => {
      disposed = true;
      stopProgress?.();
      stopFinished?.();
    };
  }, [target]);

  const rebuild = useCallback(async () => {
    setState((current) => ({
      kind: "starting",
      previousRunId: runIdFrom(current) ?? 0,
    }));
    try {
      const result = await invoke<RebuildRollupResult>("rebuild_model_rollup", {
        target,
      });
      if (result.status === "refused") {
        setState({
          kind: "refused",
          runId: result.runId,
          reason:
            result.reason ??
            "The index rebuild was refused. Wait for database maintenance to finish, then retry.",
        });
        return result;
      }
      if (result.runId === null) {
        throw new Error("The index rebuild started without a run identifier.");
      }
      const runId = result.runId;
      setState((current) => {
        const currentRunId = runIdFrom(current);
        if (currentRunId !== null && currentRunId > runId) return current;
        if (
          currentRunId === runId &&
          (current.kind === "running" ||
            current.kind === "completed" ||
            current.kind === "error")
        ) {
          return current;
        }
        return {
          kind: "running",
          progress: {
            runId,
            target,
            phase: "preflight",
            rowsDone: result.rowsDone,
            rowsTotal: result.rowsTotal,
            hourDoneThrough: result.hourDoneThrough,
          },
        };
      });
      return result;
    } catch (error) {
      setState((current) => ({
        kind: "error",
        runId: runIdFrom(current) ?? 0,
        detail: errorMessage(error),
        progress: progressFrom(current),
      }));
      throw error;
    }
  }, [target]);

  return { state, rebuild };
}
