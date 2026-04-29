import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
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

const REFRESH_INTERVAL_MS = 60_000;
const REFRESH_DEBOUNCE_MS = 1_000;

function isRouterEvent(key: string): boolean {
	return key.startsWith("router.");
}

function isContinuityEvent(key: string): boolean {
	return (
		key.startsWith("capture.") ||
		key.startsWith("continuity.") ||
		key === "mcp.continuity" ||
		key === "mcp.snapshot"
	);
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
	return {
		...summary,
		routerEventCount:
			summary.routerEventCount ?? derivedEventCount(breakdowns, isRouterEvent),
		continuityEventCount:
			summary.continuityEventCount ?? derivedEventCount(breakdowns, isContinuityEvent),
	};
}

function normalizeTimeSeries(
	points: ContextSavingsTimeSeriesPoint[],
): ContextSavingsTimeSeriesPoint[] {
	return points.map((point) => ({
		...point,
		routerEventCount: point.routerEventCount ?? 0,
		continuityEventCount: point.continuityEventCount ?? 0,
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
	const [data, setData] = useState<ContextSavingsAnalytics | null>(null);
	const [loading, setLoading] = useState(true);
	const [error, setError] = useState<string | null>(null);
	const initialLoadDone = useRef(false);
	const requestIdRef = useRef(0);

	const fetchData = useCallback(async () => {
		const requestId = requestIdRef.current + 1;
		requestIdRef.current = requestId;
		if (!initialLoadDone.current) {
			setLoading(true);
		}
		setError(null);

		try {
			const result = await invoke<ContextSavingsAnalyticsResponse>(
				"get_context_savings_analytics",
				{ range, limit },
			);

			if (requestId !== requestIdRef.current) return;
			setData(normalizeAnalytics(result, range));
		} catch (e) {
			if (requestId !== requestIdRef.current) return;
			console.error("Context savings analytics fetch error:", e);
			setError(String(e));
			setData(null);
		} finally {
			if (requestId === requestIdRef.current) {
				setLoading(false);
				initialLoadDone.current = true;
			}
		}
	}, [range, limit]);

	useEffect(() => {
		fetchData();
	}, [fetchData]);

	useEffect(() => {
		let mounted = true;
		let timer: ReturnType<typeof setTimeout> | null = null;
		const unlistenPromise = listen("context-savings-updated", () => {
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

	useEffect(() => {
		const interval = setInterval(fetchData, REFRESH_INTERVAL_MS);
		return () => clearInterval(interval);
	}, [fetchData]);

	return { data, loading, error, refresh: fetchData };
}
