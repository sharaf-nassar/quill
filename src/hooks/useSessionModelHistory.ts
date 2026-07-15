import { invoke } from "@tauri-apps/api/core";
import { useCallback, useEffect, useRef, useState } from "react";
import type {
	ModelAnalyticsError,
	ModelRange,
	SessionModelHistoryResponse,
} from "../types";
import { normalizeModelAnalyticsError } from "./modelAnalyticsErrors";

interface SessionHistoryRef {
	provider: string;
	sessionId: string;
}

interface SessionHistoryEntry extends SessionHistoryRef {
	range: ModelRange;
	data: SessionModelHistoryResponse | null;
	initialLoading: boolean;
	refreshing: boolean;
	error: ModelAnalyticsError | null;
	stale: ModelAnalyticsError | null;
}

interface ActiveHistoryRequest extends SessionHistoryRef {
	token: number;
}

export interface SessionModelHistoryState {
	data: SessionModelHistoryResponse | null;
	initialLoading: boolean;
	refreshing: boolean;
	error: ModelAnalyticsError | null;
	/** A structured `not_found` result for bounded stale-row removal. */
	stale: ModelAnalyticsError | null;
}

export interface UseSessionModelHistoryResult {
	stateFor: (provider: string, sessionId: string) => SessionModelHistoryState;
	setExpanded: (
		provider: string,
		sessionId: string,
		expanded: boolean,
	) => void;
	retry: (provider: string, sessionId: string) => void;
}

const IDLE_HISTORY_STATE: SessionModelHistoryState = {
	data: null,
	initialLoading: false,
	refreshing: false,
	error: null,
	stale: null,
};

function sessionRefKey(ref: SessionHistoryRef): string {
	return JSON.stringify([ref.provider, ref.sessionId]);
}

function historyCacheKey(ref: SessionHistoryRef, range: ModelRange): string {
	return JSON.stringify([ref.provider, ref.sessionId, range]);
}

function sameSessionRef(left: SessionHistoryRef, right: SessionHistoryRef) {
	return left.provider === right.provider && left.sessionId === right.sessionId;
}

/**
 * Lazy, exact-scope cache for expanded model-session histories. Page replay is
 * intentionally outside this hook so row refreshes remain independently
 * retryable and cannot be suppressed by a selected-model page failure.
 */
// @lat: [[frontend#Frontend#Custom Hooks#Session Model History Hook]]
export function useSessionModelHistory(
	range: ModelRange,
	refreshGeneration: number,
): UseSessionModelHistoryResult {
	const [entries, setEntries] = useState<
		Record<string, SessionHistoryEntry>
	>({});
	const entriesRef = useRef<Record<string, SessionHistoryEntry>>({});
	const expandedRefsRef = useRef<Map<string, SessionHistoryRef>>(new Map());
	const nextRequestTokenRef = useRef(0);
	const activeRequestsRef = useRef<Map<string, ActiveHistoryRequest>>(
		new Map(),
	);
	const cleanupReplayRefsRef = useRef<Map<string, SessionHistoryRef>>(
		new Map(),
	);
	const previousRangeRef = useRef(range);
	const previousRefreshGenerationRef = useRef(refreshGeneration);

	const publishEntries = useCallback(
		(next: Record<string, SessionHistoryEntry>) => {
			entriesRef.current = next;
			setEntries(next);
		},
		[],
	);

	const cancelRequests = useCallback(
		(
			matches: (request: ActiveHistoryRequest) => boolean,
			publish: boolean,
			replayOnSetup: boolean,
		) => {
			let nextEntries: Record<string, SessionHistoryEntry> | null = null;
			for (const [key, request] of activeRequestsRef.current) {
				if (!matches(request)) continue;

				activeRequestsRef.current.delete(key);
				if (replayOnSetup) cleanupReplayRefsRef.current.set(key, request);
				const entry = entriesRef.current[key];
				if (!entry) continue;

				nextEntries ??= { ...entriesRef.current };
				if (entry.data === null) {
					delete nextEntries[key];
				} else {
					nextEntries[key] = {
						...entry,
						initialLoading: false,
						refreshing: false,
					};
				}
			}

			if (nextEntries === null) return;
			entriesRef.current = nextEntries;
			if (publish) setEntries(nextEntries);
		},
		[],
	);

	const requestHistory = useCallback(
		(ref: SessionHistoryRef, requestedRange: ModelRange, force: boolean) => {
			const refKey = sessionRefKey(ref);
			if (!expandedRefsRef.current.has(refKey)) return;

			const key = historyCacheKey(ref, requestedRange);
			const existing = entriesRef.current[key];
			if (activeRequestsRef.current.has(key)) return;
			if (
				!force &&
				existing !== undefined &&
				existing.data !== null &&
				existing.error === null &&
				existing.stale === null
			) {
				return;
			}

			nextRequestTokenRef.current += 1;
			const requestToken = nextRequestTokenRef.current;
			activeRequestsRef.current.set(key, { ...ref, token: requestToken });

			const retainedData = existing?.data ?? null;
			publishEntries({
				...entriesRef.current,
				[key]: {
					...ref,
					range: requestedRange,
					data: retainedData,
					initialLoading: retainedData === null,
					refreshing: retainedData !== null,
					error: null,
					stale: null,
				},
			});

			void (async () => {
				try {
					const data = await invoke<SessionModelHistoryResponse>(
						"get_session_model_history",
						{
							provider: ref.provider,
							sessionId: ref.sessionId,
							range: requestedRange,
						},
					);
					if (
						activeRequestsRef.current.get(key)?.token !== requestToken ||
						!expandedRefsRef.current.has(refKey)
					) {
						return;
					}
					if (
						data.provider !== ref.provider ||
						data.sessionId !== ref.sessionId
					) {
						throw new Error(
							"Session model history response identity did not match its request.",
						);
					}

					publishEntries({
						...entriesRef.current,
						[key]: {
							...ref,
							range: requestedRange,
							data,
							initialLoading: false,
							refreshing: false,
							error: null,
							stale: null,
						},
					});
				} catch (error) {
					if (
						activeRequestsRef.current.get(key)?.token !== requestToken ||
						!expandedRefsRef.current.has(refKey)
					) {
						return;
					}

					const normalizedError = normalizeModelAnalyticsError(
						error,
						"Session model history could not be loaded. Retry this row.",
					);
					console.error("Session model history request failed:", error);
					publishEntries({
						...entriesRef.current,
						[key]: {
							...ref,
							range: requestedRange,
							data: entriesRef.current[key]?.data ?? retainedData,
							initialLoading: false,
							refreshing: false,
							error:
								normalizedError.code === "not_found"
									? null
									: normalizedError,
							stale:
								normalizedError.code === "not_found"
									? normalizedError
									: null,
						},
					});
				} finally {
					if (
						activeRequestsRef.current.get(key)?.token === requestToken
					) {
						activeRequestsRef.current.delete(key);
					}
				}
			})();
		},
		[publishEntries],
	);

	const setExpanded = useCallback(
		(provider: string, sessionId: string, expanded: boolean) => {
			const ref = { provider, sessionId };
			const refKey = sessionRefKey(ref);
			const key = historyCacheKey(ref, range);

			if (expanded) {
				expandedRefsRef.current.set(refKey, ref);
				requestHistory(ref, range, false);
				return;
			}

			expandedRefsRef.current.delete(refKey);
			cancelRequests(
				(request) => sameSessionRef(request, ref),
				false,
				false,
			);
			for (const [replayKey, replayRef] of cleanupReplayRefsRef.current) {
				if (sameSessionRef(replayRef, ref)) {
					cleanupReplayRefsRef.current.delete(replayKey);
				}
			}

			const existing = entriesRef.current[key];
			const next = { ...entriesRef.current };
			if (existing?.data === null) {
				delete next[key];
			} else if (existing) {
				next[key] = {
					...existing,
					initialLoading: false,
					refreshing: false,
				};
			}
			publishEntries(next);
		},
		[cancelRequests, publishEntries, range, requestHistory],
	);

	const retry = useCallback(
		(provider: string, sessionId: string) => {
			requestHistory({ provider, sessionId }, range, true);
		},
		[range, requestHistory],
	);

	const stateFor = useCallback(
		(provider: string, sessionId: string): SessionModelHistoryState => {
			const entry = entries[historyCacheKey({ provider, sessionId }, range)];
			if (!entry) return IDLE_HISTORY_STATE;
			return {
				data: entry.data,
				initialLoading: entry.initialLoading,
				refreshing: entry.refreshing,
				error: entry.error,
				stale: entry.stale,
			};
		},
		[entries, range],
	);

	useEffect(() => {
		const rangeChanged = previousRangeRef.current !== range;
		const refreshAdvanced =
			refreshGeneration > previousRefreshGenerationRef.current;

		previousRangeRef.current = range;
		previousRefreshGenerationRef.current = refreshGeneration;
		if (rangeChanged || refreshAdvanced) {
			cleanupReplayRefsRef.current.clear();
			const retainedEntries: Record<string, SessionHistoryEntry> = {};
			for (const ref of expandedRefsRef.current.values()) {
				const key = historyCacheKey(ref, range);
				const entry = entriesRef.current[key];
				if (entry) {
					retainedEntries[key] = {
						...entry,
						initialLoading: false,
						refreshing: false,
					};
				}
			}
			publishEntries(retainedEntries);
		}

		for (const ref of expandedRefsRef.current.values()) {
			const key = historyCacheKey(ref, range);
			requestHistory(
				ref,
				range,
				rangeChanged ||
					refreshAdvanced ||
					cleanupReplayRefsRef.current.has(key),
			);
		}
		cleanupReplayRefsRef.current.clear();

		return () => cancelRequests(() => true, false, true);
	}, [
		cancelRequests,
		publishEntries,
		range,
		refreshGeneration,
		requestHistory,
	]);

	return { stateFor, setExpanded, retry };
}
