import { useState, useEffect, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { getRangeMs } from "./useEfficiencyStats";
import type {
	RangeType,
	CodeStatsHistoryPoint,
	InsightTrend,
	SparklinePoint,
} from "../types";

interface VelocityStats {
	locPerHour: number | null;
	trend: InsightTrend | null;
	sparkline: SparklinePoint[];
	loading: boolean;
}

const SPARKLINE_BUCKETS = 7;

function doubledRange(range: RangeType): string {
	switch (range) {
		case "1h": return "24h";
		case "24h": return "7d";
		case "7d": return "30d";
		case "30d": return "30d";
	}
}

function computeVelocity(loc: number, ms: number): number | null {
	const hours = ms / (60 * 60 * 1000);
	if (hours === 0) return null;
	return Math.round(loc / hours);
}

export function useVelocityStats(range: RangeType): VelocityStats {
	const [result, setResult] = useState<VelocityStats>({
		locPerHour: null, trend: null, sparkline: [], loading: true,
	});

	const fetchData = useCallback(async () => {
		try {
			const fetchRange = doubledRange(range);
			const codeHistory = await invoke<CodeStatsHistoryPoint[]>(
				"get_code_stats_history", { range: fetchRange },
			);

			if (codeHistory.length === 0) {
				setResult({ locPerHour: null, trend: null, sparkline: [], loading: false });
				return;
			}

			const now = Date.now();
			const rangeMs = getRangeMs(range);
			const currentStart = now - rangeMs;
			const prevStart = currentStart - rangeMs;

			let currentLoc = 0;
			let prevLoc = 0;
			for (const point of codeHistory) {
				const ts = new Date(point.timestamp).getTime();
				if (ts >= currentStart) currentLoc += point.total_changed;
				else if (ts >= prevStart) prevLoc += point.total_changed;
			}

			const locPerHour = computeVelocity(currentLoc, rangeMs);
			const prevVelocity = computeVelocity(prevLoc, rangeMs);

			let trend: InsightTrend | null = null;
			if (locPerHour !== null && prevVelocity !== null && prevVelocity > 0) {
				const pct = Math.round(((locPerHour - prevVelocity) / prevVelocity) * 100);
				if (Math.abs(pct) < 3) {
					trend = { direction: "flat", percentage: 0, upIsGood: true };
				} else {
					trend = {
						direction: pct > 0 ? "up" : "down",
						percentage: Math.abs(pct),
						upIsGood: true,
					};
				}
			}

			const bucketMs = rangeMs / SPARKLINE_BUCKETS;
			const sparkline: SparklinePoint[] = [];
			for (let i = 0; i < SPARKLINE_BUCKETS; i++) {
				const bucketStart = currentStart + i * bucketMs;
				const bucketEnd = bucketStart + bucketMs;
				let bucketLoc = 0;
				for (const p of codeHistory) {
					const ts = new Date(p.timestamp).getTime();
					if (ts >= bucketStart && ts < bucketEnd) bucketLoc += p.total_changed;
				}
				const bucketHours = bucketMs / (60 * 60 * 1000);
				sparkline.push({ value: bucketHours > 0 ? Math.round(bucketLoc / bucketHours) : 0 });
			}

			setResult({ locPerHour, trend, sparkline, loading: false });
		} catch (e) {
			console.error("Velocity stats error:", e);
			setResult({ locPerHour: null, trend: null, sparkline: [], loading: false });
		}
	}, [range]);

	useEffect(() => { fetchData(); }, [fetchData]);
	useEffect(() => {
		const interval = setInterval(fetchData, 60_000);
		return () => clearInterval(interval);
	}, [fetchData]);

	return result;
}
