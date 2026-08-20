import { useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useCachedInvoke } from "./useCachedInvoke";
import type {
	ContextSavingsAnalytics,
	ContextSavingsAnalyticsResponse,
	ContextSavingsBreakdownGroup,
	ContextSavingsBreakdownRow,
	ContextSavingsBreakdownsResponse,
	ContextSavingsSummary,
	ContextSavingsTimeSeriesPoint,
	RangeType,
} from "../types";

function isRouterEvent(key: string): boolean {
	return key.startsWith("router.");
}

function derivedEventCount(
	breakdowns: ContextSavingsBreakdownsResponse | undefined,
	predicate: (key: string) => boolean,
): number {
	return (breakdowns?.byEventType ?? [])
		.filter((item) => predicate(item.key))
		.reduce((sum, item) => sum + item.eventCount, 0);
}

function normalizeSummary(
	summary: ContextSavingsSummary,
	breakdowns: ContextSavingsBreakdownsResponse | undefined,
): ContextSavingsSummary {
	const sourcesPreserved = summary.sourcesPreserved ?? 0;
	const sourcesRetrieved = summary.sourcesRetrieved ?? 0;
	const retentionRatio =
		summary.retentionRatio ??
		(sourcesPreserved > 0 ? sourcesRetrieved / sourcesPreserved : 0);
	return {
		...summary,
		routerEventCount:
			summary.routerEventCount ?? derivedEventCount(breakdowns, isRouterEvent),
		// Old backends do not categorize events.  Fall back to 0 — never to
		// tokensPreservedEst — so a stale backend does not silently re-surface
		// the pre-fix inflated headline.
		tokensPreserved: summary.tokensPreserved ?? 0,
		tokensRetrieved: summary.tokensRetrieved ?? 0,
		tokensRouting: summary.tokensRouting ?? 0,
		routingEventCount: summary.routingEventCount ?? 0,
		sourcesPreserved,
		sourcesRetrieved,
		retentionRatio,
	};
}

function normalizeTimeSeries(
	points: ContextSavingsTimeSeriesPoint[],
): ContextSavingsTimeSeriesPoint[] {
	return points.map((point) => ({
		...point,
		routerEventCount: point.routerEventCount ?? 0,
	}));
}

function groupRows(
	group: ContextSavingsBreakdownGroup[] | undefined,
	kind: "provider" | "eventType" | "source" | "decision" | "cwd",
): ContextSavingsBreakdownRow[] {
	return (group ?? []).map((item) => ({
		provider: kind === "provider" ? item.key : null,
		eventType: kind === "eventType" ? item.key : kind,
		source: kind === "source" || kind === "cwd" ? item.key : kind,
		eventCount: item.eventCount,
		indexedBytes: item.indexedBytes,
		returnedBytes: item.returnedBytes,
		inputBytes: item.inputBytes,
		tokensIndexedEst: item.tokensIndexedEst,
		tokensReturnedEst: item.tokensReturnedEst,
		tokensSavedEst: item.tokensSavedEst,
		tokensPreservedEst: item.tokensPreservedEst,
		estimateConfidence: null,
	}));
}

function normalizeBreakdowns(
	breakdowns: ContextSavingsAnalyticsResponse["breakdowns"],
): {
	rows: ContextSavingsBreakdownRow[];
	groups: ContextSavingsBreakdownsResponse | undefined;
} {
	if (Array.isArray(breakdowns)) {
		return { rows: breakdowns, groups: undefined };
	}
	const groups = breakdowns;
	return {
		groups,
		rows: [
			...groupRows(groups?.byEventType, "eventType"),
			...groupRows(groups?.bySource, "source"),
			...groupRows(groups?.byDecision, "decision"),
			...groupRows(groups?.byProvider, "provider"),
			...groupRows(groups?.byCwd, "cwd"),
		],
	};
}

function normalizeAnalytics(
	result: ContextSavingsAnalyticsResponse,
	range: RangeType,
): ContextSavingsAnalytics {
	const { rows, groups } = normalizeBreakdowns(result.breakdowns);
	const points = result.timeSeries ?? result.timeseries ?? [];
	return {
		...result,
		range: result.range ?? range,
		generatedAt: result.generatedAt ?? new Date().toISOString(),
		summary: normalizeSummary(result.summary, groups),
		timeSeries: normalizeTimeSeries(points),
		breakdowns: rows,
		recentEvents: result.recentEvents ?? [],
	};
}

export function useContextSavingsStats(range: RangeType, limit = 40) {
	const fetchData = useCallback(async () => {
		const result = await invoke<ContextSavingsAnalyticsResponse>(
			"get_context_savings_analytics",
			{ range, limit },
		);
		return normalizeAnalytics(result, range);
	}, [range, limit]);

	const { state, refresh } = useCachedInvoke({
		command: "get_context_savings_analytics",
		args: { range, limit },
		request: fetchData,
		normalizeError: String,
		onError: (error) =>
			console.error("Context savings analytics fetch error:", error),
		invalidationEvents: ["context-savings-updated"],
		pollMs: 60_000,
	});

	return {
		data: state.data,
		loading: state.initialLoading,
		error: state.error,
		refresh,
	};
}
