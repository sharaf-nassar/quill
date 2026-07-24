import { useCallback, useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { RangeType, LlmRuntimeStats, SparklinePoint } from "../types";
import { useCachedInvoke } from "./useCachedInvoke";

const REFRESH_DEBOUNCE_MS = 1000;
interface LlmRuntimeStatsResult { totalRuntimeSecs: number | null; turnCount: number; sessionCount: number; avgPerTurnSecs: number | null; sparkline: SparklinePoint[]; loading: boolean; }
const EMPTY: Omit<LlmRuntimeStatsResult, "loading"> = { totalRuntimeSecs: null, turnCount: 0, sessionCount: 0, avgPerTurnSecs: null, sparkline: [] };

export function useLlmRuntimeStats(range: RangeType): LlmRuntimeStatsResult {
	const request = useCallback(async (): Promise<Omit<LlmRuntimeStatsResult, "loading">> => {
		const stats = await invoke<LlmRuntimeStats>("get_llm_runtime_stats", { range });
		return stats.turn_count === 0 ? EMPTY : { totalRuntimeSecs: stats.total_runtime_secs, turnCount: stats.turn_count, sessionCount: stats.session_count, avgPerTurnSecs: stats.avg_per_turn_secs, sparkline: stats.sparkline.map((value) => ({ value })) };
	}, [range]);
	const { state, refresh } = useCachedInvoke({ identity: `llm-runtime:${range}`, request, normalizeError: String });
	useEffect(() => {
		let timer: ReturnType<typeof setTimeout> | null = null;
		const schedule = () => { if (timer) clearTimeout(timer); timer = setTimeout(refresh, REFRESH_DEBOUNCE_MS); };
		const unlisten = [listen("tokens-updated", schedule), listen("sessions-index-updated", schedule), listen("transcript-analytics-updated", schedule)];
		return () => { if (timer) clearTimeout(timer); for (const promise of unlisten) promise.then((fn) => fn()); };
	}, [refresh]);
	useEffect(() => { const interval = setInterval(refresh, 60_000); return () => clearInterval(interval); }, [refresh]);
	return { ...(state.data ?? EMPTY), loading: state.initialLoading };
}
