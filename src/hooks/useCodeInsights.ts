import { useState, useEffect, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type {
	RangeType,
	TokenDataPoint,
	CodeStatsHistoryPoint,
	InsightTrend,
	SparklinePoint,
	LlmRuntimeStats,
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

// Velocity denominator is active LLM runtime, not wall-clock span, so idle
// nights/weekends no longer crush the number. When runtime is 0/unknown for
// the window we fall back to the wall-clock span so the card still shows a
// number instead of dropping to an em-dash.
function computeVelocity(
	loc: number,
	activeSecs: number,
	fallbackMs: number,
): number | null {
	const activeHours = activeSecs / 3600;
	if (activeHours > 0) return Math.round(loc / activeHours);
	const wallHours = fallbackMs / (60 * 60 * 1000);
	if (wallHours === 0) return null;
	return Math.round(loc / wallHours);
}

// Prorate a runtime sparkline (per-bucket active seconds spanning
// [compStart, compStart + compMs]) into an arbitrary [windowStart, windowEnd)
// sub-window by linear overlap. Lets us recover the previous period's active
// runtime from the wider comparison-range fetch, since get_llm_runtime_stats
// only accepts the four fixed ranges and cannot query the prior window
// directly.
function activeSecsInWindow(
	sparkline: number[],
	compStart: number,
	compMs: number,
	windowStart: number,
	windowEnd: number,
): number {
	const buckets = sparkline.length;
	if (buckets === 0) return 0;
	const bucketMs = compMs / buckets;
	if (bucketMs === 0) return 0;
	let total = 0;
	for (let i = 0; i < buckets; i++) {
		const bStart = compStart + i * bucketMs;
		const bEnd = bStart + bucketMs;
		const overlap = Math.min(bEnd, windowEnd) - Math.max(bStart, windowStart);
		if (overlap > 0) total += sparkline[i] * (overlap / bucketMs);
	}
	return total;
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
			const [tokenHistory, codeHistory, currentRuntime, comparisonRuntime] =
				await Promise.all([
					invoke<TokenDataPoint[]>("get_token_history", {
						range: historyRange,
						hostname: null,
						sessionId: null,
						cwd: null,
					}),
					invoke<CodeStatsHistoryPoint[]>("get_code_stats_history", {
						range: historyRange,
					}),
					// Current-window active runtime — matches the LLM Runtime card,
					// which fetches the same command for the same range.
					invoke<LlmRuntimeStats>("get_llm_runtime_stats", { range }),
					// Comparison-range runtime supplies the prior window's active
					// seconds via proration. Skipped when it equals the current range
					// (30d), where the previous window falls outside history anyway.
					historyRange === range
						? Promise.resolve<LlmRuntimeStats | null>(null)
						: invoke<LlmRuntimeStats>("get_llm_runtime_stats", {
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

			const compMs = getRangeMs(historyRange);
			const compStart = now - compMs;
			const compRuntime = comparisonRuntime ?? currentRuntime;
			const currentActiveSecs = currentRuntime.total_runtime_secs;
			const prevActiveSecs = activeSecsInWindow(
				compRuntime.sparkline,
				compStart,
				compMs,
				prevStart,
				currentStart,
			);

			const tokensPerLoc = computeEfficiency(currentTokens, currentLoc);
			const prevEfficiency = computeEfficiency(prevTokens, prevLoc);
			const locPerHour = computeVelocity(currentLoc, currentActiveSecs, rangeMs);
			const prevVelocity = computeVelocity(prevLoc, prevActiveSecs, rangeMs);

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
			listen("transcript-analytics-updated", scheduleRefresh),
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
