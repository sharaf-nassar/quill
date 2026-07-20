import { invoke } from "@tauri-apps/api/core";
import {
	useCallback,
	useEffect,
	useLayoutEffect,
	useMemo,
	useRef,
	useState,
} from "react";
import type {
	ModelAnalyticsError,
	ModelIdentity,
	ModelRange,
	ModelSessionRow,
	ModelSessionsResponse,
} from "../types";
import { normalizeModelAnalyticsError } from "./modelAnalyticsErrors";

const MODEL_SESSIONS_PAGE_SIZE = 20;

type ModelSessionsOperation = "initial" | "loadMore" | "replay";

interface ModelSessionsScope {
	identity: string;
	range: ModelRange;
	modelProvider: string;
	modelId: string;
}

interface ModelSessionsState {
	identity: string | null;
	data: ModelSessionsResponse | null;
	loadedPageCount: number;
	usedCursors: string[];
	activeOperation: ModelSessionsOperation | null;
	initialError: ModelAnalyticsError | null;
	loadMoreError: ModelAnalyticsError | null;
	replayError: ModelAnalyticsError | null;
}

interface ModelSessionsPageArgs {
	requestEpoch: number;
	range: ModelRange;
	modelProvider: string;
	modelId: string;
	cursor: string | null;
	limit: number;
}

interface ActiveModelSessionsRequest {
	generation: number;
	requestEpoch: number;
	identity: string;
	operation: ModelSessionsOperation;
}

interface SequentialPageResult {
	data: ModelSessionsResponse;
	loadedPageCount: number;
	usedCursors: string[];
}

export interface ModelSessionsOperationState {
	loading: boolean;
	error: ModelAnalyticsError | null;
	retry: () => void;
}

export interface ModelSessionsLoadMoreState
	extends ModelSessionsOperationState {
	hasMore: boolean;
	run: () => void;
}

/**
 * `data` changes atomically: initial and Load more requests publish one page,
 * while shared-refresh replay publishes only after every previously loaded
 * page succeeds. Replay failures therefore leave the last complete page set
 * visible. Each operation owns its loading, error, and Retry state.
 */
export interface UseModelSessionsResult {
	data: ModelSessionsResponse | null;
	loadedPageCount: number;
	initial: ModelSessionsOperationState;
	loadMore: ModelSessionsLoadMoreState;
	replay: ModelSessionsOperationState;
}

const inFlightModelSessionPages = new Map<
	string,
	Promise<ModelSessionsResponse>
>();
let nextModelSessionsRequestEpoch = 1;

// One logical epoch may be shared by React's setup-cleanup-setup effect replay.
// Scope transitions and refresh advances allocate a new epoch, so neither can
// attach to an unresolved page request from the prior data snapshot.
function allocateModelSessionsRequestEpoch(): number {
	const epoch = nextModelSessionsRequestEpoch;
	nextModelSessionsRequestEpoch += 1;
	return epoch;
}

function emptyState(
	identity: string | null,
	selected: boolean,
): ModelSessionsState {
	return {
		identity,
		data: null,
		loadedPageCount: 0,
		usedCursors: [],
		activeOperation: selected ? "initial" : null,
		initialError: null,
		loadMoreError: null,
		replayError: null,
	};
}

function requestModelSessionsPage(
	args: ModelSessionsPageArgs,
): Promise<ModelSessionsResponse> {
	const requestKey = JSON.stringify([
		args.requestEpoch,
		args.range,
		args.modelProvider,
		args.modelId,
		args.cursor,
		args.limit,
	]);
	const existingRequest = inFlightModelSessionPages.get(requestKey);
	if (existingRequest) return existingRequest;

	const request = invoke<ModelSessionsResponse>("get_model_sessions", {
		range: args.range,
		modelProvider: args.modelProvider,
		modelId: args.modelId,
		cursor: args.cursor,
		limit: args.limit,
	}).finally(() => {
		if (inFlightModelSessionPages.get(requestKey) === request) {
			inFlightModelSessionPages.delete(requestKey);
		}
	});

	inFlightModelSessionPages.set(requestKey, request);
	return request;
}

function sessionKey(session: ModelSessionRow): string {
	return JSON.stringify([session.provider, session.sessionId]);
}

function appendUniqueSessions(
	current: readonly ModelSessionRow[],
	page: readonly ModelSessionRow[],
): ModelSessionRow[] {
	const sessions = [...current];
	const seen = new Set(current.map(sessionKey));
	for (const session of page) {
		const key = sessionKey(session);
		if (seen.has(key)) continue;
		seen.add(key);
		sessions.push(session);
	}
	return sessions;
}

function requestError(
	code: ModelAnalyticsError["code"],
	message: string,
): ModelAnalyticsError {
	return { code, message };
}

function validatePageIdentity(
	page: ModelSessionsResponse,
	scope: ModelSessionsScope,
	continuation = false,
): void {
	if (
		page.identity.provider !== scope.modelProvider ||
		page.identity.modelId !== scope.modelId
	) {
		throw requestError(
			continuation ? "invalid_cursor" : "storage_error",
			continuation
				? "The model session snapshot changed between pages. Retry from the beginning."
				: "Model sessions returned for a different model. Retry this section.",
		);
	}
}

function validateNextCursor(
	nextCursor: string | null,
	usedCursors: ReadonlySet<string>,
): void {
	if (nextCursor === null) return;
	if (nextCursor.length === 0 || usedCursors.has(nextCursor)) {
		throw requestError(
			"invalid_cursor",
			"The model session cursor is stale. Retry this page from the beginning.",
		);
	}
}

async function requestSequentialPages(
	scope: ModelSessionsScope,
	targetPageCount: number,
	requestEpoch: number,
): Promise<SequentialPageResult> {
	let cursor: string | null = null;
	let firstPage: ModelSessionsResponse | null = null;
	let latestPage: ModelSessionsResponse | null = null;
	let sessions: ModelSessionRow[] = [];
	const usedCursors = new Set<string>();
	let loadedPageCount = 0;

	while (loadedPageCount < targetPageCount) {
		if (cursor !== null) usedCursors.add(cursor);
		const page = await requestModelSessionsPage({
			requestEpoch,
			range: scope.range,
			modelProvider: scope.modelProvider,
			modelId: scope.modelId,
			cursor,
			limit: MODEL_SESSIONS_PAGE_SIZE,
		});
		validatePageIdentity(page, scope, firstPage !== null);
		if (firstPage !== null && page.total !== firstPage.total) {
			throw requestError(
				"invalid_cursor",
				"The model session snapshot changed between pages. Retry from the beginning.",
			);
		}
		validateNextCursor(page.nextCursor, usedCursors);

		firstPage ??= page;
		latestPage = page;
		sessions = appendUniqueSessions(sessions, page.sessions);
		loadedPageCount += 1;
		cursor = page.nextCursor;
		if (cursor === null) break;
	}

	if (firstPage === null || latestPage === null) {
		throw requestError(
			"storage_error",
			"Model sessions could not be loaded. Retry this section.",
		);
	}

	return {
		data: {
			identity: firstPage.identity,
			total: firstPage.total,
			nextCursor: latestPage.nextCursor,
			sessions,
		},
		loadedPageCount,
		usedCursors: [...usedCursors],
	};
}

// @lat: [[frontend#Frontend#Custom Hooks#Model Session Detail Hooks]]
export function useModelSessions(
	selectedModel: ModelIdentity | null,
	range: ModelRange,
	refreshGeneration: number,
): UseModelSessionsResult {
	const selectedProvider = selectedModel?.provider ?? null;
	const selectedModelId = selectedModel?.modelId ?? null;
	const requestIdentity =
		selectedProvider === null || selectedModelId === null
			? null
			: JSON.stringify([range, selectedProvider, selectedModelId]);
	const scope = useMemo<ModelSessionsScope | null>(
		() =>
			requestIdentity === null ||
			selectedProvider === null ||
			selectedModelId === null
				? null
				: {
						identity: requestIdentity,
						range,
						modelProvider: selectedProvider,
						modelId: selectedModelId,
					},
		[range, requestIdentity, selectedModelId, selectedProvider],
	);

	const [state, setState] = useState<ModelSessionsState>(() =>
		emptyState(requestIdentity, scope !== null),
	);
	const [initialRequestEpoch] = useState(
		allocateModelSessionsRequestEpoch,
	);
	const stateRef = useRef(state);
	const scopeRef = useRef(scope);
	const requestEpochRef = useRef(initialRequestEpoch);
	const requestEpochIdentityRef = useRef(requestIdentity);
	const requestGenerationRef = useRef(0);
	const activeRequestRef = useRef<ActiveModelSessionsRequest | null>(null);
	const handledRefreshGenerationRef = useRef(refreshGeneration);
	const latestRefreshGenerationRef = useRef(refreshGeneration);

	const replaceState = useCallback((nextState: ModelSessionsState) => {
		stateRef.current = nextState;
		setState(nextState);
	}, []);

	const updateState = useCallback(
		(updater: (current: ModelSessionsState) => ModelSessionsState) => {
			replaceState(updater(stateRef.current));
		},
		[replaceState],
	);

	const invalidateActiveRequest = useCallback(() => {
		requestGenerationRef.current += 1;
		activeRequestRef.current = null;
	}, []);

	const advanceRequestEpoch = useCallback(() => {
		requestEpochRef.current = allocateModelSessionsRequestEpoch();
	}, []);

	const beginRequest = useCallback(
		(
			requestScope: ModelSessionsScope,
			operation: ModelSessionsOperation,
		): ActiveModelSessionsRequest => {
			const generation = requestGenerationRef.current + 1;
			requestGenerationRef.current = generation;
			const request = {
				generation,
				requestEpoch: requestEpochRef.current,
				identity: requestScope.identity,
				operation,
			};
			activeRequestRef.current = request;
			updateState((current) => ({
				...current,
				identity: requestScope.identity,
				activeOperation: operation,
				initialError:
					operation === "initial" ||
					(operation === "replay" && current.data === null)
						? null
						: current.initialError,
				loadMoreError:
					operation === "loadMore" ? null : current.loadMoreError,
				replayError:
					operation === "replay" ? null : current.replayError,
			}));
			return request;
		},
		[updateState],
	);

	const acceptsResponse = useCallback(
		(
			requestScope: ModelSessionsScope,
			request: ActiveModelSessionsRequest,
		): boolean => {
			const activeRequest = activeRequestRef.current;
			return (
				requestGenerationRef.current === request.generation &&
				requestEpochRef.current === request.requestEpoch &&
				activeRequest?.generation === request.generation &&
				activeRequest.requestEpoch === request.requestEpoch &&
				activeRequest.identity === requestScope.identity &&
				scopeRef.current?.identity === requestScope.identity
			);
		},
		[],
	);

	const finishRequest = useCallback(
		(
			requestScope: ModelSessionsScope,
			request: ActiveModelSessionsRequest,
			nextState: (current: ModelSessionsState) => ModelSessionsState,
		) => {
			if (!acceptsResponse(requestScope, request)) return;
			activeRequestRef.current = null;
			updateState((current) => ({
				...nextState(current),
				activeOperation: null,
			}));
		},
		[acceptsResponse, updateState],
	);

	const runInitial = useCallback(
		(requestScope: ModelSessionsScope) => {
			const request = beginRequest(requestScope, "initial");
			void requestSequentialPages(
				requestScope,
				1,
				request.requestEpoch,
			)
				.then((result) => {
					finishRequest(requestScope, request, (current) => ({
						...current,
						data: result.data,
						loadedPageCount: result.loadedPageCount,
						usedCursors: result.usedCursors,
						initialError: null,
						loadMoreError: null,
						replayError: null,
					}));
				})
				.catch((error: unknown) => {
					if (!acceptsResponse(requestScope, request)) return;
					console.error("Initial model sessions request failed:", error);
					const normalizedError = normalizeModelAnalyticsError(
						error,
						"Model sessions could not be loaded. Retry this section.",
					);
					finishRequest(requestScope, request, (current) => ({
						...current,
						initialError: normalizedError,
					}));
				});
		},
		[acceptsResponse, beginRequest, finishRequest],
	);

	const runSequentialReplacement = useCallback(
		(
			requestScope: ModelSessionsScope,
			targetPageCount: number,
			operation: "loadMore" | "replay",
		) => {
			const request = beginRequest(requestScope, operation);
			void requestSequentialPages(
				requestScope,
				targetPageCount,
				request.requestEpoch,
			)
				.then((result) => {
					finishRequest(requestScope, request, (current) => ({
						...current,
						data: result.data,
						loadedPageCount: result.loadedPageCount,
						usedCursors: result.usedCursors,
						initialError: null,
						loadMoreError: null,
						replayError: null,
					}));
				})
				.catch((error: unknown) => {
					if (!acceptsResponse(requestScope, request)) return;
					console.error(`Model sessions ${operation} request failed:`, error);
					const normalizedError = normalizeModelAnalyticsError(
						error,
						"Model sessions could not be refreshed. Retry this page.",
					);
					finishRequest(requestScope, request, (current) => ({
						...current,
						loadMoreError:
							operation === "loadMore"
								? normalizedError
								: current.loadMoreError,
						replayError:
							operation === "replay"
								? normalizedError
								: current.replayError,
					}));
				});
		},
		[acceptsResponse, beginRequest, finishRequest],
	);

	const runLoadMore = useCallback(() => {
		const requestScope = scopeRef.current;
		const current = stateRef.current;
		if (
			requestScope === null ||
			current.identity !== requestScope.identity ||
			current.data === null ||
			current.data.nextCursor === null ||
			activeRequestRef.current !== null
		) {
			return;
		}

		const cursor = current.data.nextCursor;
		const retainedData = current.data;
		const usedCursors = new Set(current.usedCursors);
		usedCursors.add(cursor);
		const request = beginRequest(requestScope, "loadMore");

		void requestModelSessionsPage({
			requestEpoch: request.requestEpoch,
			range: requestScope.range,
			modelProvider: requestScope.modelProvider,
			modelId: requestScope.modelId,
			cursor,
			limit: MODEL_SESSIONS_PAGE_SIZE,
		})
			.then((page) => {
				validatePageIdentity(page, requestScope, true);
				if (page.total !== retainedData.total) {
					throw requestError(
						"invalid_cursor",
						"The model session snapshot changed between pages. Retry from the beginning.",
					);
				}
				validateNextCursor(page.nextCursor, usedCursors);
				finishRequest(requestScope, request, (latest) => {
					const retained = latest.data;
					if (retained === null) return latest;
					return {
						...latest,
						data: {
							identity: page.identity,
							total: page.total,
							nextCursor: page.nextCursor,
							sessions: appendUniqueSessions(
								retained.sessions,
								page.sessions,
							),
						},
						loadedPageCount: latest.loadedPageCount + 1,
						usedCursors: [...usedCursors],
						loadMoreError: null,
					};
				});
			})
			.catch((error: unknown) => {
				if (!acceptsResponse(requestScope, request)) return;
				console.error("Load more model sessions request failed:", error);
				const normalizedError = normalizeModelAnalyticsError(
					error,
					"More model sessions could not be loaded. Retry this page.",
				);
				finishRequest(requestScope, request, (latest) => ({
					...latest,
					loadMoreError: normalizedError,
				}));
			});
	}, [acceptsResponse, beginRequest, finishRequest]);

	const retryInitial = useCallback(() => {
		const requestScope = scopeRef.current;
		if (
			requestScope === null ||
			stateRef.current.identity !== requestScope.identity ||
			stateRef.current.data !== null ||
			activeRequestRef.current !== null
		) {
			return;
		}
		runInitial(requestScope);
	}, [runInitial]);

	const retryLoadMore = useCallback(() => {
		const requestScope = scopeRef.current;
		const current = stateRef.current;
		if (
			requestScope === null ||
			current.identity !== requestScope.identity ||
			current.loadMoreError === null ||
			activeRequestRef.current !== null
		) {
			return;
		}

		if (current.loadMoreError.code === "invalid_cursor") {
			runSequentialReplacement(
				requestScope,
				Math.max(1, current.loadedPageCount + 1),
				"loadMore",
			);
			return;
		}
		runLoadMore();
	}, [runLoadMore, runSequentialReplacement]);

	const retryReplay = useCallback(() => {
		const requestScope = scopeRef.current;
		const current = stateRef.current;
		if (
			requestScope === null ||
			current.identity !== requestScope.identity ||
			current.replayError === null ||
			activeRequestRef.current !== null
		) {
			return;
		}
		runSequentialReplacement(
			requestScope,
			Math.max(1, current.loadedPageCount),
			"replay",
		);
	}, [runSequentialReplacement]);

	useLayoutEffect(() => {
		scopeRef.current = scope;
		latestRefreshGenerationRef.current = refreshGeneration;
	}, [refreshGeneration, scope]);

	useEffect(() => {
		invalidateActiveRequest();
		// Strict Mode repeats this effect without changing requestIdentity. Keep
		// that epoch for request dedupe, but never reuse it after a real scope reset.
		if (requestEpochIdentityRef.current !== requestIdentity) {
			requestEpochIdentityRef.current = requestIdentity;
			advanceRequestEpoch();
		}
		handledRefreshGenerationRef.current =
			latestRefreshGenerationRef.current;
		const requestScope = scopeRef.current;
		replaceState(emptyState(requestIdentity, requestScope !== null));
		if (requestScope !== null) runInitial(requestScope);

		return invalidateActiveRequest;
	}, [
		advanceRequestEpoch,
		invalidateActiveRequest,
		replaceState,
		requestIdentity,
		runInitial,
	]);

	useEffect(() => {
		const requestScope = scopeRef.current;
		if (
			requestScope === null ||
			requestScope.identity !== requestIdentity ||
			refreshGeneration <= handledRefreshGenerationRef.current
		) {
			return;
		}

		handledRefreshGenerationRef.current = refreshGeneration;
		// A refresh represents a newer committed backend snapshot even when all
		// IPC arguments are otherwise identical.
		advanceRequestEpoch();
		runSequentialReplacement(
			requestScope,
			Math.max(1, stateRef.current.loadedPageCount),
			"replay",
		);
	}, [
		advanceRequestEpoch,
		refreshGeneration,
		requestIdentity,
		runSequentialReplacement,
	]);

	const visibleState =
		state.identity === requestIdentity
			? state
			: emptyState(requestIdentity, scope !== null);
	const hasMore =
		visibleState.data !== null && visibleState.data.nextCursor !== null;

	return {
		data: visibleState.data,
		loadedPageCount: visibleState.loadedPageCount,
		initial: {
			loading: visibleState.activeOperation === "initial",
			error: visibleState.initialError,
			retry: retryInitial,
		},
		loadMore: {
			loading: visibleState.activeOperation === "loadMore",
			error: visibleState.loadMoreError,
			hasMore,
			run: runLoadMore,
			retry: retryLoadMore,
		},
		replay: {
			loading: visibleState.activeOperation === "replay",
			error: visibleState.replayError,
			retry: retryReplay,
		},
	};
}
