import { useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useCachedInvoke } from "./useCachedInvoke";
import {
	codeInsightsComparisonRange,
	codeInsightsHistoryQueries,
	queryRangeMs,
} from "./widgetQueryPlan";
import type {
	RangeType,
	TokenDataPoint,
	CodeStatsHistoryPoint,
	InsightTrend,
	SparklinePoint,
	LlmRuntimeStats,
} from "../types";
import type { LlmRuntimeStatsResult } from "./useLlmRuntimeStats";

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

function computeEfficiency(tokens: number, loc: number): number | null {
	if (loc === 0) return null;
	return Math.round(tokens / loc);
}

// Velocity denominator is active LLM runtime, not wall-clock span, so idle
// nights/weekends no longer crush the number. When runtime is 0/unknown for
// the window we fall back to the wall-clock span so the card still shows a
// number instead of dropping to an em-dash.
//
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

export function useCodeInsights(
	range: RangeType,
	currentRuntime: LlmRuntimeStatsResult,
): CodeInsightsResult {
	const {
		loading: runtimeLoading,
		totalRuntimeSecs,
		sparkline: runtimeSparkline,
	} = currentRuntime;

	const fetchData = useCallback(async () => {
		const historyRange = codeInsightsComparisonRange(range);
		const [tokenQuery, codeQuery, runtimeQuery] =
			codeInsightsHistoryQueries(range);
		const [tokenHistory, codeHistory, comparisonRuntime] =
			await Promise.all([
					invoke<TokenDataPoint[]>(tokenQuery.command, tokenQuery.args),
					invoke<CodeStatsHistoryPoint[]>(codeQuery.command, codeQuery.args),
					// Comparison-range runtime supplies the prior window's active
					// seconds via proration. The current window comes from the shared
					// LLM Runtime hook, so this never duplicates that IPC request.
					historyRange === range
						? Promise.resolve<LlmRuntimeStats | null>(null)
						: invoke<LlmRuntimeStats>(runtimeQuery.command, runtimeQuery.args),
			]);

		if (tokenHistory.length === 0 || codeHistory.length === 0) {
			return { ...EMPTY_RESULT, loading: false };
		}

		const now = Date.now();
		const rangeMs = queryRangeMs(range);
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

		const compMs = queryRangeMs(historyRange);
		const compStart = now - compMs;
		const compRuntime = comparisonRuntime ?? {
			sparkline: runtimeSparkline.map(({ value }) => value),
		};
		const currentActiveSecs = totalRuntimeSecs ?? 0;
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

		return {
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
			};
	}, [range, runtimeSparkline, totalRuntimeSecs]);

	const { state } = useCachedInvoke({
		command: "widget_code_insights",
		args: {
			range,
			totalRuntimeSecs,
			runtimeSparkline: runtimeSparkline.map(({ value }) => value),
		},
		request: fetchData,
		normalizeError: String,
		onError: (error) => console.error("Code insights fetch error:", error),
		enabled: !runtimeLoading,
		invalidationEvents: [
			"tokens-updated",
			"sessions-index-updated",
			"transcript-analytics-updated",
		],
		pollMs: 60_000,
	});

	return state.data ?? { ...EMPTY_RESULT, loading: state.initialLoading };
}
