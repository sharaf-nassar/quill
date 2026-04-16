import { useState, useEffect, useCallback, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { RangeType, CodeStats, CodeStatsHistoryPoint } from "../types";

const REFRESH_DEBOUNCE_MS = 1000;

export function useCodeStats(range: RangeType) {
	const [stats, setStats] = useState<CodeStats | null>(null);
	const [history, setHistory] = useState<CodeStatsHistoryPoint[]>([]);
	const [loading, setLoading] = useState(true);
	const [error, setError] = useState<string | null>(null);
	const initialLoadDone = useRef(false);

	const fetchData = useCallback(async () => {
		if (!initialLoadDone.current) {
			setLoading(true);
		}
		setError(null);

		try {
			const [statsData, historyData] = await Promise.all([
				invoke<CodeStats>("get_code_stats", { range }),
				invoke<CodeStatsHistoryPoint[]>("get_code_stats_history", { range }),
			]);
			setStats(statsData);
			setHistory(historyData);
		} catch (e) {
			console.error("Code stats fetch error:", e);
			setError(String(e));
		} finally {
			setLoading(false);
			initialLoadDone.current = true;
		}
	}, [range]);

	useEffect(() => {
		fetchData();
	}, [fetchData]);

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

	// Periodic refresh every 60s
	useEffect(() => {
		const interval = setInterval(fetchData, 60_000);
		return () => clearInterval(interval);
	}, [fetchData]);

	return { stats, history, loading, error, refresh: fetchData };
}
