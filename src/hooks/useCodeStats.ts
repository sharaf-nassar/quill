import { useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { RangeType, CodeStats, CodeStatsHistoryPoint } from "../types";
import { useCachedInvoke } from "./useCachedInvoke";

interface CodeStatsResult { stats: CodeStats; history: CodeStatsHistoryPoint[]; }

export function useCodeStats(range: RangeType) {
	const request = useCallback(async (): Promise<CodeStatsResult> => {
		const [stats, history] = await Promise.all([
			invoke<CodeStats>("get_code_stats", { range }),
			invoke<CodeStatsHistoryPoint[]>("get_code_stats_history", { range }),
		]);
		return { stats, history };
	}, [range]);
	const { state, refresh } = useCachedInvoke({
		command: "get_code_stats+get_code_stats_history",
		args: { range },
		request,
		normalizeError: String,
		invalidationEvents: [
			"sessions-index-updated",
			"transcript-analytics-updated",
		],
		pollMs: 60_000,
	});
	return { stats: state.data?.stats ?? null, history: state.data?.history ?? [], loading: state.initialLoading, error: state.error, refresh };
}
