import { useState, useEffect, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type {
	RangeType,
	TokenDataPoint,
	CodeStatsHistoryPoint,
	InsightTrend,
	SparklinePoint,
} from "../types";

interface InsightMetric {
	trend: InsightTrend | null;
	sparkline: SparklinePoint[];
}

interface EfficiencyMetric extends InsightMetric {
	tokensPerLoc: number | null;
}

interface VelocityMetric extends InsightMetric {
	locPerHour: number | null;
}

interface CodeInsightsResult {
	efficiency: EfficiencyMetric;
	velocity: VelocityMetric;
	loading: boolean;
}

const SPARKLINE_BUCKETS = 7;
const REFRESH_DEBOUNCE_MS = 1000;

function getRangeMs(range: RangeType): number {
	switch (range) {
		case "1h":
			return 60 * 60 * 1000;
		case "24h":
			return 24 * 60 * 60 * 1000;
		case "7d":
			return 7 * 24 * 60 * 60 * 1000;
		case "30d":
			return 30 * 24 * 60 * 60 * 1000;
	}
}

function comparisonRange(range: RangeType): RangeType {
	switch (range) {
		case "1h":
			return "24h";
		case "24h":
			return "7d";
		case "7d":
			return "30d";
		case "30d":
			return "30d";
	}
}

function computeEfficiency(tokens: number, loc: number): number | null {
	if (loc === 0) return null;
	return Math.round(tokens / loc);
}

function computeVelocity(loc: number, ms: number): number | null {
	const hours = ms / (60 * 60 * 1000);
	if (hours === 0) return null;
	return Math.round(loc / hours);
}

function computeTrend(
	current: number | null,
	previous: number | null,
	upIsGood: boolean,
): InsightTrend | null {
	if (current === null || previous === null || previous === 0) return null;
	const pct = Math.round(((current - previous) / previous) * 100);
	if (Math.abs(pct) < 3) {
		return { direction: "flat", percentage: 0, upIsGood };
	}
	return {
		direction: pct > 0 ? "up" : "down",
		percentage: Math.abs(pct),
		upIsGood,
	};
}

const EMPTY_RESULT: CodeInsightsResult = {
	efficiency: {
		tokensPerLoc: null,
		trend: null,
		sparkline: [],
	},
	velocity: {
		locPerHour: null,
		trend: null,
		sparkline: [],
	},
	loading: true,
};

export function useCodeInsights(range: RangeType): CodeInsightsResult {
	const [result, setResult] = useState<CodeInsightsResult>(EMPTY_RESULT);

	const fetchData = useCallback(async () => {
		try {
			const historyRange = comparisonRange(range);
			const [tokenHistory, codeHistory] = await Promise.all([
				invoke<TokenDataPoint[]>("get_token_history", {
					range: historyRange,
					hostname: null,
					sessionId: null,
					cwd: null,
				}),
				invoke<CodeStatsHistoryPoint[]>("get_code_stats_history", {
					range: historyRange,
				}),
			]);

			if (tokenHistory.length === 0 || codeHistory.length === 0) {
				setResult({
					...EMPTY_RESULT,
					loading: false,
				});
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

			const bucketMs = rangeMs / SPARKLINE_BUCKETS;
			const efficiencySparkline: SparklinePoint[] = [];
			const velocitySparkline: SparklinePoint[] = [];
			for (let i = 0; i < SPARKLINE_BUCKETS; i++) {
				const bucketStart = currentStart + i * bucketMs;
				const bucketEnd = bucketStart + bucketMs;
				let bucketTokens = 0;
				let bucketLoc = 0;
				for (const point of tokenHistory) {
					const ts = new Date(point.timestamp).getTime();
					if (ts >= bucketStart && ts < bucketEnd) {
						bucketTokens += point.total_tokens;
					}
				}
				for (const point of codeHistory) {
					const ts = new Date(point.timestamp).getTime();
					if (ts >= bucketStart && ts < bucketEnd) {
						bucketLoc += point.total_changed;
					}
				}
				const bucketHours = bucketMs / (60 * 60 * 1000);
				efficiencySparkline.push({
					value: bucketLoc > 0 ? Math.round(bucketTokens / bucketLoc) : 0,
				});
				velocitySparkline.push({
					value: bucketHours > 0 ? Math.round(bucketLoc / bucketHours) : 0,
				});
			}

			const tokensPerLoc = computeEfficiency(currentTokens, currentLoc);
			const prevEfficiency = computeEfficiency(prevTokens, prevLoc);
			const locPerHour = computeVelocity(currentLoc, rangeMs);
			const prevVelocity = computeVelocity(prevLoc, rangeMs);

			setResult({
				efficiency: {
					tokensPerLoc,
					trend: computeTrend(tokensPerLoc, prevEfficiency, false),
					sparkline: efficiencySparkline,
				},
				velocity: {
					locPerHour,
					trend: computeTrend(locPerHour, prevVelocity, true),
					sparkline: velocitySparkline,
				},
				loading: false,
			});
		} catch (e) {
			console.error("Code insights fetch error:", e);
			setResult({
				...EMPTY_RESULT,
				loading: false,
			});
		}
	}, [range]);

	useEffect(() => {
		fetchData();
	}, [fetchData]);

	useEffect(() => {
		let mounted = true;
		let timer: ReturnType<typeof setTimeout> | null = null;
		const scheduleRefresh = () => {
			if (!mounted) return;
			if (timer) clearTimeout(timer);
			timer = setTimeout(fetchData, REFRESH_DEBOUNCE_MS);
		};
		const unlistenPromises = [
			listen("tokens-updated", scheduleRefresh),
			listen("sessions-index-updated", scheduleRefresh),
		];

		return () => {
			mounted = false;
			if (timer) clearTimeout(timer);
			for (const unlistenPromise of unlistenPromises) {
				unlistenPromise.then((fn) => fn());
			}
		};
	}, [fetchData]);

	useEffect(() => {
		const interval = setInterval(fetchData, 60_000);
		return () => clearInterval(interval);
	}, [fetchData]);

	return result;
}
