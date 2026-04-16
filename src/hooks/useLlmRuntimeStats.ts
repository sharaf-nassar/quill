import { useState, useEffect, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { RangeType, LlmRuntimeStats, SparklinePoint } from "../types";

const REFRESH_DEBOUNCE_MS = 1000;

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
		let mounted = true;
		let timer: ReturnType<typeof setTimeout> | null = null;
		const unlistenPromise = listen("sessions-index-updated", () => {
			if (!mounted) return;
			if (timer) clearTimeout(timer);
			timer = setTimeout(fetchData, REFRESH_DEBOUNCE_MS);
		});

		return () => {
			mounted = false;
			if (timer) clearTimeout(timer);
			unlistenPromise.then((fn) => fn());
		};
	}, [fetchData]);
	useEffect(() => {
		const interval = setInterval(fetchData, 60_000);
		return () => clearInterval(interval);
	}, [fetchData]);

	return result;
}
