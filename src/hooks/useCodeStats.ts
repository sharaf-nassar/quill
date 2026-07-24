import { useCallback, useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { RangeType, CodeStats, CodeStatsHistoryPoint } from "../types";
import { useCachedInvoke } from "./useCachedInvoke";

const REFRESH_DEBOUNCE_MS = 1000;
interface CodeStatsResult { stats: CodeStats; history: CodeStatsHistoryPoint[]; }

export function useCodeStats(range: RangeType) {
	const request = useCallback(async (): Promise<CodeStatsResult> => {
		const [stats, history] = await Promise.all([
			invoke<CodeStats>("get_code_stats", { range }),
			invoke<CodeStatsHistoryPoint[]>("get_code_stats_history", { range }),
		]);
		return { stats, history };
	}, [range]);
	const { state, refresh } = useCachedInvoke({ identity: `code-stats:${range}`, request, normalizeError: String });
	useEffect(() => {
		let timer: ReturnType<typeof setTimeout> | null = null;
		const schedule = () => { if (timer) clearTimeout(timer); timer = setTimeout(refresh, REFRESH_DEBOUNCE_MS); };
		const unlisten = [listen("sessions-index-updated", schedule), listen("transcript-analytics-updated", schedule)];
		return () => { if (timer) clearTimeout(timer); for (const promise of unlisten) promise.then((fn) => fn()); };
	}, [refresh]);
	useEffect(() => { const interval = setInterval(refresh, 60_000); return () => clearInterval(interval); }, [refresh]);
	return { stats: state.data?.stats ?? null, history: state.data?.history ?? [], loading: state.initialLoading, error: state.error, refresh };
}
