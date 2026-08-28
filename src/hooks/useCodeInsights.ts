import { useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useCachedInvoke } from "./useCachedInvoke";
import { codeInsightsHistoryQueries, queryRangeMs } from "./widgetQueryPlan";
import type {
	RangeType,
	TokenDataPoint,
	CodeStatsHistoryPoint,
	SparklinePoint,
} from "../types";
import type { LlmRuntimeStatsResult } from "./useLlmRuntimeStats";

interface InsightMetric {
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

const EMPTY_RESULT: CodeInsightsResult = {
	efficiency: {
		tokensPerLoc: null,
		sparkline: [],
	},
	velocity: {
		locPerHour: null,
		sparkline: [],
	},
	loading: true,
};

export function useCodeInsights(
	range: RangeType,
	currentRuntime: LlmRuntimeStatsResult,
): CodeInsightsResult {
	const { loading: runtimeLoading, totalRuntimeSecs } = currentRuntime;

	const fetchData = useCallback(async () => {
		const [tokenQuery, codeQuery] = codeInsightsHistoryQueries(range);
		const [tokenHistory, codeHistory] = await Promise.all([
			invoke<TokenDataPoint[]>(tokenQuery.command, tokenQuery.args),
			invoke<CodeStatsHistoryPoint[]>(codeQuery.command, codeQuery.args),
		]);

		if (tokenHistory.length === 0 || codeHistory.length === 0) {
			return { ...EMPTY_RESULT, loading: false };
		}

		const now = Date.now();
		const rangeMs = queryRangeMs(range);
		const currentStart = now - rangeMs;

		let currentTokens = 0;
		for (const point of tokenHistory) {
			if (new Date(point.timestamp).getTime() >= currentStart) {
				currentTokens += point.total_tokens;
			}
		}

		let currentLoc = 0;
		for (const point of codeHistory) {
			if (new Date(point.timestamp).getTime() >= currentStart) {
				currentLoc += point.total_changed;
			}
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

		return {
				efficiency: {
					tokensPerLoc: computeEfficiency(currentTokens, currentLoc),
					sparkline: efficiencySparkline,
				},
				velocity: {
					locPerHour: computeVelocity(
						currentLoc,
						totalRuntimeSecs ?? 0,
						rangeMs,
					),
					sparkline: velocitySparkline,
				},
				loading: false,
			};
	}, [range, totalRuntimeSecs]);

	const { state } = useCachedInvoke({
		command: "widget_code_insights",
		args: { range, totalRuntimeSecs },
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
