import { useState, useEffect, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import type {
	RangeType,
	TokenDataPoint,
	CodeStatsHistoryPoint,
	InsightTrend,
	SparklinePoint,
} from "../types";

interface EfficiencyStats {
	tokensPerLoc: number | null;
	trend: InsightTrend | null;
	sparkline: SparklinePoint[];
	loading: boolean;
}

const SPARKLINE_BUCKETS = 7;

export function getRangeMs(range: RangeType): number {
	switch (range) {
		case "1h": return 60 * 60 * 1000;
		case "24h": return 24 * 60 * 60 * 1000;
		case "7d": return 7 * 24 * 60 * 60 * 1000;
		case "30d": return 30 * 24 * 60 * 60 * 1000;
	}
}

function doubledRange(range: RangeType): string {
	switch (range) {
		case "1h": return "24h";
		case "24h": return "7d";
		case "7d": return "30d";
		case "30d": return "30d";
	}
}

function computeEfficiency(tokens: number, loc: number): number | null {
	if (loc === 0) return null;
	return Math.round(tokens / loc);
}

function computeTrend(
	current: number | null,
	previous: number | null,
): InsightTrend | null {
	if (current === null || previous === null || previous === 0) return null;
	const pct = Math.round(((current - previous) / previous) * 100);
	if (Math.abs(pct) < 3) return { direction: "flat", percentage: 0, upIsGood: false };
	return {
		direction: pct > 0 ? "up" : "down",
		percentage: Math.abs(pct),
		upIsGood: false,
	};
}

export function useEfficiencyStats(range: RangeType): EfficiencyStats {
	const [result, setResult] = useState<EfficiencyStats>({
		tokensPerLoc: null, trend: null, sparkline: [], loading: true,
	});

	const fetchData = useCallback(async () => {
		try {
			const fetchRange = doubledRange(range);
			const [tokenHistory, codeHistory] = await Promise.all([
				invoke<TokenDataPoint[]>("get_token_history", {
					range: fetchRange, hostname: null, sessionId: null, cwd: null,
				}),
				invoke<CodeStatsHistoryPoint[]>("get_code_stats_history", {
					range: fetchRange,
				}),
			]);

			if (tokenHistory.length === 0 || codeHistory.length === 0) {
				setResult({ tokensPerLoc: null, trend: null, sparkline: [], loading: false });
				return;
			}

			const now = Date.now();
			const rangeMs = getRangeMs(range);
			const currentStart = now - rangeMs;
			const prevStart = currentStart - rangeMs;

			let currentTokens = 0;
			let prevTokens = 0;
			for (const point of tokenHistory) {
				const ts = new Date(point.timestamp).getTime();
				if (ts >= currentStart) currentTokens += point.total_tokens;
				else if (ts >= prevStart) prevTokens += point.total_tokens;
			}

			let currentLoc = 0;
			let prevLoc = 0;
			for (const point of codeHistory) {
				const ts = new Date(point.timestamp).getTime();
				if (ts >= currentStart) currentLoc += point.total_changed;
				else if (ts >= prevStart) prevLoc += point.total_changed;
			}

			const tokensPerLoc = computeEfficiency(currentTokens, currentLoc);
			const prevEfficiency = computeEfficiency(prevTokens, prevLoc);
			const trend = computeTrend(tokensPerLoc, prevEfficiency);

			const bucketMs = rangeMs / SPARKLINE_BUCKETS;
			const sparkline: SparklinePoint[] = [];
			for (let i = 0; i < SPARKLINE_BUCKETS; i++) {
				const bucketStart = currentStart + i * bucketMs;
				const bucketEnd = bucketStart + bucketMs;
				let bTok = 0;
				for (const p of tokenHistory) {
					const ts = new Date(p.timestamp).getTime();
					if (ts >= bucketStart && ts < bucketEnd) bTok += p.total_tokens;
				}
				let bLoc = 0;
				for (const p of codeHistory) {
					const ts = new Date(p.timestamp).getTime();
					if (ts >= bucketStart && ts < bucketEnd) bLoc += p.total_changed;
				}
				sparkline.push({ value: bLoc > 0 ? Math.round(bTok / bLoc) : 0 });
			}

			setResult({ tokensPerLoc, trend, sparkline, loading: false });
		} catch (e) {
			console.error("Efficiency stats error:", e);
			setResult({ tokensPerLoc: null, trend: null, sparkline: [], loading: false });
		}
	}, [range]);

	useEffect(() => { fetchData(); }, [fetchData]);
	useEffect(() => {
		const interval = setInterval(fetchData, 60_000);
		return () => clearInterval(interval);
	}, [fetchData]);

	return result;
}
