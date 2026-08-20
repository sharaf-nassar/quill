import { useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { RangeType, LlmRuntimeStats, SparklinePoint } from "../types";
import { useCachedInvoke } from "./useCachedInvoke";

export interface LlmRuntimeStatsResult { totalRuntimeSecs: number | null; turnCount: number; sessionCount: number; avgPerTurnSecs: number | null; sparkline: SparklinePoint[]; loading: boolean; }
const EMPTY: Omit<LlmRuntimeStatsResult, "loading"> = { totalRuntimeSecs: null, turnCount: 0, sessionCount: 0, avgPerTurnSecs: null, sparkline: [] };

export function useLlmRuntimeStats(range: RangeType): LlmRuntimeStatsResult {
	const request = useCallback(async (): Promise<Omit<LlmRuntimeStatsResult, "loading">> => {
		const stats = await invoke<LlmRuntimeStats>("get_llm_runtime_stats", { range });
		return stats.turn_count === 0 ? EMPTY : { totalRuntimeSecs: stats.total_runtime_secs, turnCount: stats.turn_count, sessionCount: stats.session_count, avgPerTurnSecs: stats.avg_per_turn_secs, sparkline: stats.sparkline.map((value) => ({ value })) };
	}, [range]);
	const { state } = useCachedInvoke({
		command: "get_llm_runtime_stats",
		args: { range },
		request,
		normalizeError: String,
		invalidationEvents: [
			"tokens-updated",
			"sessions-index-updated",
			"transcript-analytics-updated",
		],
		pollMs: 60_000,
	});
	return { ...(state.data ?? EMPTY), loading: state.initialLoading };
}
