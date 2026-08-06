import {
	useCallback,
	useEffect,
	useRef,
	useState,
} from "react";
import { invoke } from "@tauri-apps/api/core";
import type {
	ModelAnalyticsError,
	ModelAnalyticsErrorCode,
	ModelBackfillState,
	ModelBackfillStatus,
	ModelRange,
	ModelUsageOverviewResponse,
} from "../types";
import { useCachedInvoke } from "./useCachedInvoke";

const FALLBACK_POLL_MS = 60_000;

const MODEL_ANALYTICS_ERROR_CODES = new Set<ModelAnalyticsErrorCode>([
	"invalid_range",
	"invalid_provider",
	"invalid_model_id",
	"invalid_cursor",
	"not_found",
	"storage_error",
]);

const DEFAULT_ERROR_MESSAGE =
	"Model analytics could not be loaded. Retry this section.";

function parseErrorCandidate(value: unknown): unknown {
	if (value instanceof Error) {
		return parseErrorCandidate(value.message);
	}

	if (typeof value !== "string") return value;

	try {
		return JSON.parse(value) as unknown;
	} catch {
		return value;
	}
}

/**
 * Preserve the shared model-analytics IPC envelope while keeping unexpected
 * failures bounded and safe for display. Tauri may reject with either the
 * serialized object or a JSON string depending on the runtime boundary.
 */
function normalizeModelAnalyticsError(
	error: unknown,
	fallbackMessage = DEFAULT_ERROR_MESSAGE,
): ModelAnalyticsError {
	const candidate = parseErrorCandidate(error);
	if (candidate && typeof candidate === "object") {
		const code = Reflect.get(candidate, "code");
		const message = Reflect.get(candidate, "message");
		if (
			typeof code === "string" &&
			MODEL_ANALYTICS_ERROR_CODES.has(code as ModelAnalyticsErrorCode) &&
			typeof message === "string"
		) {
			return { code: code as ModelAnalyticsErrorCode, message };
		}
	}

	return {
		code: "storage_error",
		message: fallbackMessage,
	};
}

export interface ModelAnalyticsRequestState<T> {
	data: T | null;
	initialLoading: boolean;
	refreshing: boolean;
	error: ModelAnalyticsError | null;
	retry: () => void;
}

export interface ModelBackfillRequestState {
	status: ModelBackfillStatus | null;
	isRetrying: boolean;
	retryError: ModelAnalyticsError | null;
	retry: () => void;
}

export interface UseModelAnalyticsResult {
	overview: ModelAnalyticsRequestState<ModelUsageOverviewResponse>;
	backfill: ModelBackfillRequestState;
	refreshGeneration: number;
}

const BACKFILL_STATE_ORDER: Record<ModelBackfillState, number> = {
	pending: 0,
	running: 1,
	complete: 2,
	partial: 2,
	failed: 2,
};

type BackfillProgressComparison =
	| "candidate"
	| "current"
	| "equal"
	| "conflict";

function isTerminalBackfillState(state: ModelBackfillState): boolean {
	return state === "complete" || state === "partial" || state === "failed";
}

function compareBackfillProgress(
	current: ModelBackfillStatus,
	candidate: ModelBackfillStatus,
): BackfillProgressComparison {
	let candidateAdvanced = false;
	let currentAdvanced = false;
	const compareAscending = (currentValue: number, candidateValue: number) => {
		if (candidateValue > currentValue) candidateAdvanced = true;
		if (candidateValue < currentValue) currentAdvanced = true;
	};

	compareAscending(current.totalRoots, candidate.totalRoots);
	compareAscending(current.completedRoots, candidate.completedRoots);
	compareAscending(current.failedRoots, candidate.failedRoots);
	compareAscending(current.totalSources, candidate.totalSources);
	compareAscending(current.processedSources, candidate.processedSources);
	compareAscending(current.failedSources, candidate.failedSources);
	compareAscending(current.skippedSources, candidate.skippedSources);
	compareAscending(
		current.observationsWritten,
		candidate.observationsWritten,
	);
	compareAscending(
		Number(current.inventoryComplete),
		Number(candidate.inventoryComplete),
	);
	compareAscending(
		Number(current.startedAt !== null),
		Number(candidate.startedAt !== null),
	);
	compareAscending(
		Number(current.finishedAt !== null),
		Number(candidate.finishedAt !== null),
	);

	// Publishing a nonzero source total moves remaining from zero to that total.
	// Once the total is stable, only a lower remaining count is forward progress.
	if (candidate.totalSources === current.totalSources) {
		compareAscending(candidate.remainingSources, current.remainingSources);
	}

	if (candidateAdvanced && currentAdvanced) return "conflict";
	if (candidateAdvanced) return "candidate";
	if (currentAdvanced) return "current";
	return "equal";
}

function latestBackfillStatus(
	current: ModelBackfillStatus | null,
	candidate: ModelBackfillStatus,
): ModelBackfillStatus {
	if (current === null) return candidate;
	if (candidate.generation !== current.generation) {
		return candidate.generation > current.generation ? candidate : current;
	}

	const currentStateOrder = BACKFILL_STATE_ORDER[current.status];
	const candidateStateOrder = BACKFILL_STATE_ORDER[candidate.status];
	if (candidateStateOrder !== currentStateOrder) {
		return candidateStateOrder > currentStateOrder ? candidate : current;
	}

	// A generation has exactly one terminal resolution. Never let a delayed,
	// contradictory terminal snapshot replace the first accepted resolution.
	if (
		isTerminalBackfillState(current.status) &&
		candidate.status !== current.status
	) {
		return current;
	}

	const progress = compareBackfillProgress(current, candidate);
	if (progress === "candidate") return candidate;
	if (progress === "current" || progress === "conflict") return current;

	// Wall time is only advisory after all persisted monotonic facts tie. A
	// system-clock rollback therefore cannot hide lifecycle or counter progress.
	const currentUpdatedAt = Date.parse(current.updatedAt);
	const candidateUpdatedAt = Date.parse(candidate.updatedAt);
	if (
		Number.isFinite(currentUpdatedAt) &&
		Number.isFinite(candidateUpdatedAt) &&
		candidateUpdatedAt !== currentUpdatedAt
	) {
		return candidateUpdatedAt > currentUpdatedAt ? candidate : current;
	}

	return candidate;
}

// @lat: [[frontend#Frontend#Custom Hooks#Model Analytics Hook]]
export function useModelAnalytics(
	range: ModelRange,
	provider: string | null,
	active: boolean,
): UseModelAnalyticsResult {
	const pendingRefreshWhileHiddenRef = useRef(false);
	const [backfillStatus, setBackfillStatus] =
		useState<ModelBackfillStatus | null>(null);
	const [isBackfillRetrying, setIsBackfillRetrying] = useState(false);
	const [backfillRetryError, setBackfillRetryError] =
		useState<ModelAnalyticsError | null>(null);
	const [refreshGeneration, setRefreshGeneration] = useState(0);
	const backfillRetryGenerationRef = useRef(0);
	const backfillRetryInFlightRef = useRef(false);

	const acceptBackfillStatus = useCallback((status: ModelBackfillStatus) => {
		setBackfillStatus((current) => latestBackfillStatus(current, status));
	}, []);
	const acceptOverviewData = useCallback(
		(response: ModelUsageOverviewResponse) => {
			acceptBackfillStatus(response.backfill);
			setRefreshGeneration((generation) => generation + 1);
		},
		[acceptBackfillStatus],
	);

	const requestOverview = useCallback(
		() =>
			invoke<ModelUsageOverviewResponse>("get_model_usage_overview", {
				range,
				provider,
			}),
		[provider, range],
	);
	const logOverviewError = useCallback((error: unknown) => {
		console.error("Model usage overview request failed:", error);
	}, []);

	const overviewRequest = useCachedInvoke({
		command: "get_model_usage_overview",
		args: { range, provider },
		request: requestOverview,
		normalizeError: normalizeModelAnalyticsError,
		onAcceptedData: acceptOverviewData,
		onError: logOverviewError,
		invalidationEvents: ["model-analytics-updated"],
	});
	const refreshOverview = overviewRequest.refresh;

	const retryBackfill = useCallback(() => {
		if (backfillRetryInFlightRef.current) return;

		backfillRetryInFlightRef.current = true;
		const requestGeneration = backfillRetryGenerationRef.current + 1;
		backfillRetryGenerationRef.current = requestGeneration;
		setIsBackfillRetrying(true);
		setBackfillRetryError(null);

		void (async () => {
			try {
				const status = await invoke<ModelBackfillStatus>(
					"retry_model_history_backfill",
				);
				if (requestGeneration !== backfillRetryGenerationRef.current) return;

				acceptBackfillStatus(status);
				setIsBackfillRetrying(false);
				setBackfillRetryError(null);
			} catch (error) {
				if (requestGeneration !== backfillRetryGenerationRef.current) return;

				console.error("Model backfill retry failed:", error);
				setIsBackfillRetrying(false);
				setBackfillRetryError(normalizeModelAnalyticsError(error));
			} finally {
				if (requestGeneration === backfillRetryGenerationRef.current) {
					backfillRetryInFlightRef.current = false;
				}
			}
		})();
	}, [acceptBackfillStatus]);

	useEffect(
		() => () => {
			backfillRetryGenerationRef.current += 1;
			backfillRetryInFlightRef.current = false;
		},
		[],
	);

	useEffect(() => {
		const refreshIfObservable = () => {
			if (active && document.visibilityState !== "hidden") {
				pendingRefreshWhileHiddenRef.current = false;
				refreshOverview();
			} else {
				pendingRefreshWhileHiddenRef.current = true;
			}
		};
		const flushPendingIfObservable = () => {
			if (
				!pendingRefreshWhileHiddenRef.current ||
				!active ||
				document.visibilityState === "hidden"
			) {
				return;
			}
			pendingRefreshWhileHiddenRef.current = false;
			refreshOverview();
		};

		flushPendingIfObservable();
		const pollTimer = setInterval(refreshIfObservable, FALLBACK_POLL_MS);
		document.addEventListener("visibilitychange", handleVisibilityChange);
		function handleVisibilityChange() {
			flushPendingIfObservable();
		}

		return () => {
			clearInterval(pollTimer);
			document.removeEventListener("visibilitychange", handleVisibilityChange);
		};
	}, [active, refreshOverview]);

	return {
		overview: overviewRequest.state,
		backfill: {
			status: backfillStatus,
			isRetrying: isBackfillRetrying,
			retryError: backfillRetryError,
			retry: retryBackfill,
		},
		refreshGeneration,
	};
}
