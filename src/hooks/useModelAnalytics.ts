import {
	useCallback,
	useEffect,
	useEffectEvent,
	useRef,
	useState,
} from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type {
	ModelAnalyticsError,
	ModelAnalyticsResponse,
	ModelAnalyticsUpdatedEvent,
	ModelBackfillState,
	ModelBackfillStatus,
	ModelHistoryResponse,
	ModelIdentity,
	ModelRange,
} from "../types";
import { normalizeModelAnalyticsError } from "./modelAnalyticsErrors";

const EVENT_COALESCE_MS = 1_000;
const FALLBACK_POLL_MS = 60_000;

interface ScopedRequestState<T> {
	identity: string;
	generation: number;
	data: T | null;
	initialLoading: boolean;
	refreshing: boolean;
	error: ModelAnalyticsError | null;
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
	aggregate: ModelAnalyticsRequestState<ModelAnalyticsResponse>;
	history: ModelAnalyticsRequestState<ModelHistoryResponse>;
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

function emptyRequestState<T>(identity: string): ScopedRequestState<T> {
	return {
		identity,
		generation: 0,
		data: null,
		initialLoading: true,
		refreshing: false,
		error: null,
	};
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

function useScopedModelRequest<T>(
	identity: string,
	label: string,
	request: () => Promise<T>,
	onAcceptedData?: (data: T) => void,
): {
	state: ModelAnalyticsRequestState<T>;
	refresh: () => void;
} {
	const [requestState, setRequestState] = useState<ScopedRequestState<T>>(
		() => emptyRequestState(identity),
	);
	const requestGenerationRef = useRef(0);
	const activeRequestRef = useRef<{
		identity: string;
		generation: number;
		phase: "in_flight" | "settled";
	} | null>(null);
	const pendingRefreshIdentityRef = useRef<string | null>(null);
	const deferredRefreshRef = useRef<{
		identity: string;
		timer: ReturnType<typeof setTimeout>;
	} | null>(null);

	const refresh = useCallback(function refreshScopedRequest() {
		const activeRequest = activeRequestRef.current;
		if (activeRequest?.identity === identity) {
			pendingRefreshIdentityRef.current = identity;
			return;
		}

		if (pendingRefreshIdentityRef.current === identity) return;

		const deferredRefresh = deferredRefreshRef.current;
		if (deferredRefresh !== null) {
			clearTimeout(deferredRefresh.timer);
			deferredRefreshRef.current = null;
		}
		pendingRefreshIdentityRef.current = null;

		const requestGeneration = requestGenerationRef.current + 1;
		requestGenerationRef.current = requestGeneration;
		activeRequestRef.current = {
			identity,
			generation: requestGeneration,
			phase: "in_flight",
		};

		setRequestState((previous) => {
			const retainedData =
				previous.identity === identity ? previous.data : null;
			return {
				identity,
				generation: requestGeneration,
				data: retainedData,
				initialLoading: retainedData === null,
				refreshing: retainedData !== null,
				error: null,
			};
		});

		void (async () => {
			try {
				const data = await request();
				if (requestGeneration !== requestGenerationRef.current) return;

				onAcceptedData?.(data);
				setRequestState((previous) =>
					previous.identity === identity
						? {
								identity,
								generation: requestGeneration,
								data,
								initialLoading: false,
								refreshing: false,
								error: null,
							}
						: previous,
				);
			} catch (error) {
				if (requestGeneration !== requestGenerationRef.current) return;

				console.error(`Model ${label} request failed:`, error);
				const normalizedError = normalizeModelAnalyticsError(error);
				setRequestState((previous) =>
					previous.identity === identity
						? {
								...previous,
								generation: requestGeneration,
								initialLoading: false,
								refreshing: false,
								error: normalizedError,
							}
						: previous,
				);
			} finally {
				const activeRequest = activeRequestRef.current;
				if (
					activeRequest?.identity === identity &&
					activeRequest.generation === requestGeneration
				) {
					activeRequest.phase = "settled";
				}
			}
		})();
	}, [identity, label, onAcceptedData, request]);

	useEffect(() => {
		refresh();
		return () => {
			requestGenerationRef.current += 1;
			activeRequestRef.current = null;
			pendingRefreshIdentityRef.current = null;
			const deferredRefresh = deferredRefreshRef.current;
			if (deferredRefresh !== null) {
				clearTimeout(deferredRefresh.timer);
				deferredRefreshRef.current = null;
			}
		};
	}, [refresh]);

	useEffect(() => {
		// A response settles before React commits its state update. Retain its
		// active marker through that gap, then release it from matching committed
		// state so accepted data can render before the one queued refresh.
		const activeRequest = activeRequestRef.current;
		if (
			activeRequest === null ||
			requestState.identity !== identity ||
			requestState.generation !== activeRequest.generation ||
			requestState.initialLoading ||
			requestState.refreshing ||
			activeRequest.identity !== identity ||
			activeRequest.phase !== "settled"
		) {
			return;
		}

		activeRequestRef.current = null;
		if (
			pendingRefreshIdentityRef.current !== identity ||
			deferredRefreshRef.current !== null
		) {
			return;
		}

		pendingRefreshIdentityRef.current = null;
		const timer = setTimeout(() => {
			const deferredRefresh = deferredRefreshRef.current;
			if (
				deferredRefresh?.identity !== identity ||
				deferredRefresh.timer !== timer
			) {
				return;
			}

			deferredRefreshRef.current = null;
			refresh();
		}, 0);
		deferredRefreshRef.current = { identity, timer };
	}, [identity, refresh, requestState]);

	const state =
		requestState.identity === identity
			? requestState
			: emptyRequestState<T>(identity);

	return {
		state: {
			data: state.data,
			initialLoading: state.initialLoading,
			refreshing: state.refreshing,
			error: state.error,
			retry: refresh,
		},
		refresh,
	};
}

// @lat: [[frontend#Frontend#Custom Hooks#Model Analytics Hook]]
export function useModelAnalytics(
	range: ModelRange,
	provider: string | null,
	selectedModel: ModelIdentity | null,
): UseModelAnalyticsResult {
	const selectedProvider = selectedModel?.provider ?? null;
	const selectedModelId = selectedModel?.modelId ?? null;
	const aggregateIdentity = JSON.stringify([range, provider]);
	const historyIdentity = JSON.stringify([
		range,
		provider,
		selectedProvider,
		selectedModelId,
	]);
	const [backfillStatus, setBackfillStatus] =
		useState<ModelBackfillStatus | null>(null);
	const [isBackfillRetrying, setIsBackfillRetrying] = useState(false);
	const [backfillRetryError, setBackfillRetryError] =
		useState<ModelAnalyticsError | null>(null);
	const backfillRetryGenerationRef = useRef(0);
	const backfillRetryInFlightRef = useRef(false);

	const acceptBackfillStatus = useCallback((status: ModelBackfillStatus) => {
		setBackfillStatus((current) => latestBackfillStatus(current, status));
	}, []);
	const acceptAggregateData = useCallback(
		(response: ModelAnalyticsResponse) => {
			acceptBackfillStatus(response.backfill);
		},
		[acceptBackfillStatus],
	);

	const requestAggregate = useCallback(
		() =>
			invoke<ModelAnalyticsResponse>("get_model_analytics", {
				range,
				provider,
			}),
		[provider, range],
	);
	const requestHistory = useCallback(
		() =>
			invoke<ModelHistoryResponse>("get_model_history", {
				range,
				provider,
				selectedModel:
					selectedProvider === null || selectedModelId === null
						? null
						: {
								provider: selectedProvider,
								modelId: selectedModelId,
							},
			}),
		[provider, range, selectedModelId, selectedProvider],
	);

	const aggregateRequest = useScopedModelRequest(
		aggregateIdentity,
		"aggregate",
		requestAggregate,
		acceptAggregateData,
	);
	const historyRequest = useScopedModelRequest(
		historyIdentity,
		"history",
		requestHistory,
	);
	const [refreshGeneration, setRefreshGeneration] = useState(0);

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

	const refreshFromExternalSignal = useEffectEvent(() => {
		setRefreshGeneration((generation) => generation + 1);
		aggregateRequest.refresh();
		historyRequest.refresh();
	});

	useEffect(() => {
		let disposed = false;
		let unlisten: (() => void) | null = null;
		let eventTimer: ReturnType<typeof setTimeout> | null = null;

		void listen<ModelAnalyticsUpdatedEvent>(
			"model-analytics-updated",
			() => {
				if (disposed || eventTimer !== null) return;

				// The first event owns the deadline. Later events join this window
				// without clearing or extending its timer.
				eventTimer = setTimeout(() => {
					eventTimer = null;
					if (!disposed) refreshFromExternalSignal();
				}, EVENT_COALESCE_MS);
			},
		)
			.then((stopListening) => {
				if (disposed) {
					stopListening();
				} else {
					unlisten = stopListening;
					// Reconcile once after subscription becomes active. This closes the
					// gap where a commit event can land between initial fetch and the
					// asynchronous Tauri listener registration. If the listener already
					// captured an event, its fixed-window timer owns that refresh.
					if (eventTimer === null) refreshFromExternalSignal();
				}
			})
			.catch((error: unknown) => {
				if (!disposed) {
					console.error("Model analytics event listener failed:", error);
				}
			});

		const pollTimer = setInterval(() => {
			if (!disposed) refreshFromExternalSignal();
		}, FALLBACK_POLL_MS);

		return () => {
			disposed = true;
			if (eventTimer !== null) clearTimeout(eventTimer);
			clearInterval(pollTimer);
			unlisten?.();
		};
	}, []);

	return {
		aggregate: aggregateRequest.state,
		history: historyRequest.state,
		backfill: {
			status: backfillStatus,
			isRetrying: isBackfillRetrying,
			retryError: backfillRetryError,
			retry: retryBackfill,
		},
		refreshGeneration,
	};
}
