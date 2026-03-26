import { useState, useEffect, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { RangeType, ResponseTimeStats, SparklinePoint } from "../types";

interface ResponseTimeStatsResult {
	avgResponseSecs: number | null;
	peakResponseSecs: number | null;
	avgIdleSecs: number | null;
	sparkline: SparklinePoint[];
	accentColor: string;
	loading: boolean;
}

function computeAccentColor(avgResponseSecs: number | null): string {
	if (avgResponseSecs === null) return "#8b949e";
	if (avgResponseSecs < 30) return "#34d399";
	if (avgResponseSecs < 120) return "#fbbf24";
	return "#f87171";
}

export function useResponseTimeStats(range: RangeType): ResponseTimeStatsResult {
	const [result, setResult] = useState<ResponseTimeStatsResult>({
		avgResponseSecs: null, peakResponseSecs: null, avgIdleSecs: null,
		sparkline: [], accentColor: "#8b949e", loading: true,
	});

	const fetchData = useCallback(async () => {
		try {
			const stats = await invoke<ResponseTimeStats>("get_response_time_stats", { range });

			if (stats.sample_count === 0) {
				setResult({
					avgResponseSecs: null, peakResponseSecs: null, avgIdleSecs: null,
					sparkline: [], accentColor: "#8b949e", loading: false,
				});
				return;
			}

			const avgResponseSecs = stats.avg_response_secs;
			const peakResponseSecs = stats.peak_response_secs;
			const avgIdleSecs = stats.avg_idle_secs;
			const sparkline: SparklinePoint[] = stats.sparkline.map((v) => ({ value: v }));
			const accentColor = computeAccentColor(avgResponseSecs);

			setResult({ avgResponseSecs, peakResponseSecs, avgIdleSecs, sparkline, accentColor, loading: false });
		} catch (e) {
			console.error("Response time stats error:", e);
			setResult({
				avgResponseSecs: null, peakResponseSecs: null, avgIdleSecs: null,
				sparkline: [], accentColor: "#8b949e", loading: false,
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
