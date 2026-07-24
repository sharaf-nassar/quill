import { useCallback, useEffect, useRef, useState } from "react";

export const CACHED_INVOKE_DEBOUNCE_MS = 200;

interface CacheEntry<T> {
	data: T;
	serialized: string;
}

interface RequestState<T, E> {
	identity: string;
	generation: number;
	data: T | null;
	initialLoading: boolean;
	refreshing: boolean;
	error: E | null;
}

export interface CachedInvokeState<T, E> {
	data: T | null;
	initialLoading: boolean;
	refreshing: boolean;
	error: E | null;
	retry: () => void;
}

export interface UseCachedInvokeOptions<T, E> {
	identity: string;
	request: () => Promise<T>;
	normalizeError: (error: unknown) => E;
	onAcceptedData?: (data: T) => void;
	onError?: (error: unknown) => void;
}

function emptyRequestState<T, E>(identity: string): RequestState<T, E> {
	return {
		identity,
		generation: 0,
		data: null,
		initialLoading: true,
		refreshing: false,
		error: null,
	};
}

/**
 * Invokes one identity-scoped IPC request while retaining accepted results for
 * stale-while-revalidate rendering. Later ports share its debounce, cache,
 * dedupe, and stale-response guard rather than reimplementing them per hook.
 */
export function useCachedInvoke<T, E>({
	identity,
	request,
	normalizeError,
	onAcceptedData,
	onError,
}: UseCachedInvokeOptions<T, E>): {
	state: CachedInvokeState<T, E>;
	refresh: () => void;
} {
	const [requestState, setRequestState] = useState<RequestState<T, E>>(() =>
		emptyRequestState(identity),
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
	const scopeCacheRef = useRef<Map<string, CacheEntry<T>>>(new Map());
	const hasIssuedRequestRef = useRef(false);

	const refresh = useCallback(function refreshCachedInvoke() {
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
		const shouldDebounce = hasIssuedRequestRef.current;
		hasIssuedRequestRef.current = true;

		setRequestState((previous) => {
			const cached = scopeCacheRef.current.get(identity)?.data ?? null;
			const retainedData =
				previous.identity === identity ? (previous.data ?? cached) : cached;
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
						setTimeout(resolve, CACHED_INVOKE_DEBOUNCE_MS);
					});
					if (requestGeneration !== requestGenerationRef.current) return;
				}

				const data = await request();
				if (requestGeneration !== requestGenerationRef.current) return;

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

				onError?.(error);
				setRequestState((previous) =>
					previous.identity === identity
						? {
								...previous,
								generation: requestGeneration,
								initialLoading: false,
								refreshing: false,
								error: normalizeError(error),
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
	}, [identity, normalizeError, onAcceptedData, onError, request]);

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
			: {
					identity,
					generation: requestState.generation,
					data: scopeCacheRef.current.get(identity)?.data ?? null,
					initialLoading:
						scopeCacheRef.current.get(identity)?.data === undefined,
					refreshing: scopeCacheRef.current.get(identity)?.data !== undefined,
					error: null,
				};

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
