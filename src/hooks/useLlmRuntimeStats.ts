import { useState, useEffect, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { RangeType, LlmRuntimeStats, SparklinePoint } from "../types";

interface LlmRuntimeStatsResult {
	totalRuntimeSecs: number | null;
	turnCount: number;
	sessionCount: number;
	avgPerTurnSecs: number | null;
	sparkline: SparklinePoint[];
	loading: boolean;
}

export function useLlmRuntimeStats(range: RangeType): LlmRuntimeStatsResult {
	const [result, setResult] = useState<LlmRuntimeStatsResult>({
		totalRuntimeSecs: null, turnCount: 0, sessionCount: 0, avgPerTurnSecs: null,
		sparkline: [], loading: true,
	});

	const fetchData = useCallback(async () => {
		try {
			const stats = await invoke<LlmRuntimeStats>("get_llm_runtime_stats", { range });

			if (stats.turn_count === 0) {
				setResult({
					totalRuntimeSecs: null, turnCount: 0, sessionCount: 0, avgPerTurnSecs: null,
					sparkline: [], loading: false,
				});
				return;
			}

			setResult({
				totalRuntimeSecs: stats.total_runtime_secs,
				turnCount: stats.turn_count,
				sessionCount: stats.session_count,
				avgPerTurnSecs: stats.avg_per_turn_secs,
				sparkline: stats.sparkline.map((v) => ({ value: v })),
				loading: false,
			});
		} catch (e) {
			console.error("LLM runtime stats error:", e);
			setResult({
				totalRuntimeSecs: null, turnCount: 0, sessionCount: 0, avgPerTurnSecs: null,
				sparkline: [], loading: false,
			});
		}
	}, [range]);

	useEffect(() => { fetchData(); }, [fetchData]);
	useEffect(() => {
		const interval = setInterval(fetchData, 60_000);
		return () => clearInterval(interval);
	}, [fetchData]);

	return result;
}
