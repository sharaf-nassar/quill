import { useCallback, useEffect, useRef, useState } from "react";
import {
	cachedInvokeKey,
	cachedInvokeStore,
	type CachedInvokeNotification,
	type CachedInvokeSnapshot,
} from "./cachedInvokeStore";

interface RequestState<T, E> {
	cacheKey: string;
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
	command: string;
	args?: unknown;
	request: () => Promise<T>;
	normalizeError: (error: unknown) => E;
	onAcceptedData?: (data: T) => void;
	onError?: (error: unknown) => void;
	enabled?: boolean;
	invalidationEvents?: readonly string[];
	pollMs?: number;
}

function localState<T, E>(
	cacheKey: string,
	snapshot: CachedInvokeSnapshot<T>,
	normalizeError: (error: unknown) => E,
): RequestState<T, E> {
	return {
		cacheKey,
		data: snapshot.data,
		initialLoading: snapshot.initialLoading,
		refreshing: snapshot.refreshing,
		error: snapshot.error === null ? null : normalizeError(snapshot.error),
	};
}

/**
 * Reads one command-and-args-scoped IPC result from the process-lifetime
 * cache. Fresh remounts render without another invoke; stale entries remain
 * visible while one shared background request revalidates them.
 */
// @lat: [[frontend#Frontend#Custom Hooks#Data Fetching Hooks]]
export function useCachedInvoke<T, E>({
	command,
	args,
	request,
	normalizeError,
	onAcceptedData,
	onError,
	enabled = true,
	invalidationEvents = [],
	pollMs,
}: UseCachedInvokeOptions<T, E>): {
	state: CachedInvokeState<T, E>;
	refresh: () => void;
} {
	const cacheKey = cachedInvokeKey({ command, args });
	const requestRef = useRef(request);
	const normalizeErrorRef = useRef(normalizeError);
	const onAcceptedDataRef = useRef(onAcceptedData);
	const onErrorRef = useRef(onError);
	const invalidationEventsRef = useRef(invalidationEvents);
	useEffect(() => {
		requestRef.current = request;
		normalizeErrorRef.current = normalizeError;
		onAcceptedDataRef.current = onAcceptedData;
		onErrorRef.current = onError;
		invalidationEventsRef.current = invalidationEvents;
	});

	const [requestState, setRequestState] = useState<RequestState<T, E>>(() =>
		localState(
			cacheKey,
			cachedInvokeStore.snapshot<T>(cacheKey),
			normalizeError,
		),
	);

	useEffect(() => {
		if (!enabled) return;
		return cachedInvokeStore.subscribe<T>(
			cacheKey,
			() => requestRef.current(),
			(notification: CachedInvokeNotification<T>) => {
				if (
					(notification.kind === "snapshot" && notification.snapshot.hasData) ||
					notification.kind === "accepted"
				) {
					onAcceptedDataRef.current?.(notification.snapshot.data as T);
				}
				if (notification.kind === "error") {
					onErrorRef.current?.(notification.snapshot.error);
				}
				setRequestState(
					localState(
						cacheKey,
						notification.snapshot,
						normalizeErrorRef.current,
					),
				);
			},
			invalidationEventsRef.current,
		);
	}, [cacheKey, enabled]);

	const refresh = useCallback(() => {
		if (!enabled) return;
		cachedInvokeStore.refresh<T>(cacheKey, () => requestRef.current());
	}, [cacheKey, enabled]);

	const retry = useCallback(() => {
		if (!enabled) return;
		cachedInvokeStore.retry<T>(cacheKey, () => requestRef.current());
	}, [cacheKey, enabled]);

	useEffect(() => {
		if (!enabled || pollMs === undefined) return;
		const interval = setInterval(refresh, pollMs);
		return () => clearInterval(interval);
	}, [enabled, pollMs, refresh]);

	const state =
		requestState.cacheKey === cacheKey
			? requestState
			: localState(
					cacheKey,
					cachedInvokeStore.snapshot<T>(cacheKey),
					normalizeError,
				);

	return {
		state: {
			data: state.data,
			initialLoading: enabled ? state.initialLoading : true,
			refreshing: enabled ? state.refreshing : false,
			error: enabled ? state.error : null,
			retry,
		},
		refresh,
	};
}
