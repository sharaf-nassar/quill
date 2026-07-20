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
	ModelAnalyticsUpdatedEvent,
	ModelBackfillState,
	ModelBackfillStatus,
	ModelRange,
	ModelUsageOverviewResponse,
} from "../types";
import { normalizeModelAnalyticsError } from "./modelAnalyticsErrors";

const EVENT_COALESCE_MS = 1_000;
const FALLBACK_POLL_MS = 60_000;
const SCOPE_DEBOUNCE_MS = 200;

interface ScopeCacheEntry<T> {
	data: T;
	serialized: string;
}

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
	// Identity-keyed cache of the last good response per scope. Revisiting a
	// scope renders its cached bytes instantly while a background refresh runs,
	// and identical follow-up responses reuse the cached object reference so the
	// overview ref stays stable and memoized sections skip re-rendering.
	const scopeCacheRef = useRef<Map<string, ScopeCacheEntry<T>>>(new Map());
	const hasIssuedRequestRef = useRef(false);

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
		// The first request of a hook instance issues immediately; every later
		// one is debounced so rapid range/provider toggles collapse into a single
		// backend query for the scope that settles last.
		const shouldDebounce = hasIssuedRequestRef.current;
		hasIssuedRequestRef.current = true;

		setRequestState((previous) => {
			const cached = scopeCacheRef.current.get(identity)?.data ?? null;
			const retainedData =
				previous.identity === identity
					? (previous.data ?? cached)
					: cached;
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
				if (shouldDebounce) {
					await new Promise<void>((resolve) => {
						setTimeout(resolve, SCOPE_DEBOUNCE_MS);
					});
					if (requestGeneration !== requestGenerationRef.current) return;
				}

				const data = await request();
				if (requestGeneration !== requestGenerationRef.current) return;

				// Reuse the cached reference for byte-identical responses so the
				// overview ref never churns on no-op refreshes.
				const serialized = JSON.stringify(data);
				const cachedEntry = scopeCacheRef.current.get(identity);
				const nextData =
					cachedEntry !== undefined && cachedEntry.serialized === serialized
						? cachedEntry.data
						: data;
				scopeCacheRef.current.set(identity, { data: nextData, serialized });

				onAcceptedData?.(nextData);
				setRequestState((previous) =>
					previous.identity === identity
						? {
								identity,
								generation: requestGeneration,
								data: nextData,
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

	// While a scope change is committing (before refresh reseeds requestState),
	// project a cache-seeded snapshot so a revisited scope renders instantly
	// instead of flashing a skeleton.
	let state: ScopedRequestState<T>;
	if (requestState.identity === identity) {
		state = requestState;
	} else {
		const cached = scopeCacheRef.current.get(identity)?.data ?? null;
		state = {
			identity,
			generation: requestState.generation,
			data: cached,
			initialLoading: cached === null,
			refreshing: cached !== null,
			error: null,
		};
	}

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
	active: boolean,
): UseModelAnalyticsResult {
	const overviewIdentity = JSON.stringify([range, provider]);
	const pendingRefreshWhileHiddenRef = useRef(false);
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
	const acceptOverviewData = useCallback(
		(response: ModelUsageOverviewResponse) => {
			acceptBackfillStatus(response.backfill);
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

	const overviewRequest = useScopedModelRequest(
		overviewIdentity,
		"usage overview",
		requestOverview,
		acceptOverviewData,
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
		overviewRequest.refresh();
	});

	// External signals (ingest events, poll) only refresh when the panel is
	// observable — the Models tab is active and the document is visible. While
	// hidden the signal is remembered and replayed once on the next activation,
	// so a background tab never storms the backend or fans out to detail hooks.
	const maybeRefreshFromSignal = useEffectEvent(() => {
		if (active && document.visibilityState !== "hidden") {
			refreshFromExternalSignal();
		} else {
			pendingRefreshWhileHiddenRef.current = true;
		}
	});

	const flushPendingIfObservable = useEffectEvent(() => {
		if (!active || document.visibilityState === "hidden") return;
		if (!pendingRefreshWhileHiddenRef.current) return;
		pendingRefreshWhileHiddenRef.current = false;
		refreshFromExternalSignal();
	});

	useEffect(() => {
		flushPendingIfObservable();
	}, [active]);

	useEffect(() => {
		let disposed = false;
		let unlisten: (() => void) | null = null;
		let eventTimer: ReturnType<typeof setTimeout> | null = null;

		void listen<ModelAnalyticsUpdatedEvent>(
			"model-analytics-updated",
			(event) => {
				if (disposed || eventTimer !== null) return;
				// Backfill emits one event per committed source; no-op commits carry
				// dataChanged=false. Ignore them so idle ingest cannot storm refetches.
				if (event.payload.dataChanged === false) return;

				// The first data-changing event owns the deadline. Later events join
				// this window without clearing or extending its timer.
				eventTimer = setTimeout(() => {
					eventTimer = null;
					if (!disposed) maybeRefreshFromSignal();
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
					if (eventTimer === null) maybeRefreshFromSignal();
				}
			})
			.catch((error: unknown) => {
				if (!disposed) {
					console.error("Model analytics event listener failed:", error);
				}
			});

		const pollTimer = setInterval(() => {
			if (!disposed) maybeRefreshFromSignal();
		}, FALLBACK_POLL_MS);

		const handleVisibilityChange = () => {
			if (!disposed) flushPendingIfObservable();
		};
		document.addEventListener("visibilitychange", handleVisibilityChange);

		return () => {
			disposed = true;
			if (eventTimer !== null) clearTimeout(eventTimer);
			clearInterval(pollTimer);
			document.removeEventListener("visibilitychange", handleVisibilityChange);
			unlisten?.();
		};
	}, []);

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
