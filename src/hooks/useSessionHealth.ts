import { useCallback, useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { SessionHealthStats, SessionStatsRaw } from "../types";
import { useCachedInvoke } from "./useCachedInvoke";

export function useSessionHealth(days: number): {
	stats: SessionHealthStats | null;
	loading: boolean;
} {
	const request = useCallback(async (): Promise<SessionHealthStats> => {
		const [current, previous] = await Promise.all([
			invoke<SessionStatsRaw>("get_session_stats", { days }),
			invoke<SessionStatsRaw>("get_session_stats", { days: days * 2 }),
		]);
		const prevSessionCount = previous.session_count - current.session_count;
		const prevTotalTokens = previous.total_tokens - current.total_tokens;
		return {
			avgDurationSeconds: current.avg_duration_seconds,
			avgTokens: current.avg_tokens,
			sessionsPerDay: days > 0 ? current.session_count / days : 0,
			sessionCount: current.session_count,
			prev: {
				avgDurationSeconds: prevSessionCount > 0 ? (previous.avg_duration_seconds * previous.session_count - current.avg_duration_seconds * current.session_count) / prevSessionCount : 0,
				avgTokens: prevSessionCount > 0 ? prevTotalTokens / prevSessionCount : 0,
				sessionsPerDay: days > 0 ? prevSessionCount / days : 0,
				sessionCount: prevSessionCount,
			},
		};
	}, [days]);
	const { state, refresh } = useCachedInvoke({ identity: `session-health:${days}`, request, normalizeError: String });
	useEffect(() => {
		const interval = setInterval(refresh, 60_000);
		return () => clearInterval(interval);
	}, [refresh]);
	return { stats: state.data, loading: state.initialLoading };
}
