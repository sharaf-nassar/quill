import { useState, useEffect, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { SessionHealthStats, SessionStatsRaw } from "../types";

export function useSessionHealth(days: number): {
	stats: SessionHealthStats | null;
	loading: boolean;
} {
	const [stats, setStats] = useState<SessionHealthStats | null>(null);
	const [loading, setLoading] = useState(true);

	const fetchData = useCallback(async () => {
		setLoading(true);
		try {
			const [current, previous] = await Promise.all([
				invoke<SessionStatsRaw>("get_session_stats", { days }),
				invoke<SessionStatsRaw>("get_session_stats", { days: days * 2 }),
			]);

			const prevSessionCount = previous.session_count - current.session_count;
			const prevTotalTokens = previous.total_tokens - current.total_tokens;

			setStats({
				avgDurationSeconds: current.avg_duration_seconds,
				avgTokens: current.avg_tokens,
				sessionsPerDay: days > 0 ? current.session_count / days : 0,
				sessionCount: current.session_count,
				prev: {
					avgDurationSeconds:
						prevSessionCount > 0
							? (previous.avg_duration_seconds * previous.session_count -
									current.avg_duration_seconds * current.session_count) /
								prevSessionCount
							: 0,
					avgTokens:
						prevSessionCount > 0 ? prevTotalTokens / prevSessionCount : 0,
					sessionsPerDay: days > 0 ? prevSessionCount / days : 0,
					sessionCount: prevSessionCount,
				},
			});
		} catch (e) {
			console.error("Session health fetch error:", e);
			setStats(null);
		} finally {
			setLoading(false);
		}
	}, [days]);

	useEffect(() => {
		fetchData();
	}, [fetchData]);

	useEffect(() => {
		const interval = setInterval(fetchData, 60_000);
		return () => clearInterval(interval);
	}, [fetchData]);

	return { stats, loading };
}
