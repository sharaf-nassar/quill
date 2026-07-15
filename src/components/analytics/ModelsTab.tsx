import {
	useCallback,
	useId,
	useLayoutEffect,
	useMemo,
	useRef,
	useState,
} from "react";
import { useModelAnalytics } from "../../hooks/useModelAnalytics";
import {
	type UseModelSessionsResult,
	useModelSessions,
} from "../../hooks/useModelSessions";
import { useSessionModelHistory } from "../../hooks/useSessionModelHistory";
import type {
	ModelAnalyticsResponse,
	ModelBackfillStatus as ModelBackfillStatusData,
	ModelIdentity,
	ModelRange,
} from "../../types";
import { modelIdentityKey } from "../../types";
import ModelBackfillStatus from "./models/ModelBackfillStatus";
import ModelDetailPanel from "./models/ModelDetailPanel";
import ModelSummaryStrip from "./models/ModelSummaryStrip";
import ModelUsageHistory from "./models/ModelUsageHistory";
import ModelUsageTable from "./models/ModelUsageTable";

const MODEL_RANGES: readonly ModelRange[] = ["1h", "24h", "7d", "30d"];

const RANGE_LABELS: Record<ModelRange, string> = {
	"1h": "1H",
	"24h": "24H",
	"7d": "7D",
	"30d": "30D",
};

const COUNT_FORMATTER = new Intl.NumberFormat("en-US");
const EMPTY_SESSION_KEYS: ReadonlySet<string> = new Set();

interface HiddenModelSessions {
	scopeKey: string | null;
	sessionKeys: ReadonlySet<string>;
}

interface ActiveModelDetailScope {
	scopeKey: string | null;
	pageSessionKeys: ReadonlySet<string>;
}

interface ModelScopeNotice {
	kind: "global-empty" | "filtered-empty" | "evidence-empty";
	title: string;
	description: string;
}

function providerLabel(provider: string): string {
	if (provider === "claude") return "Claude";
	if (provider === "codex") return "Codex";
	if (provider === "mini_max") return "MiniMax";
	return provider;
}

function detailScopeKey(
	range: ModelRange,
	selectedModel: ModelIdentity | null,
): string | null {
	return selectedModel === null
		? null
		: JSON.stringify([range, selectedModel.provider, selectedModel.modelId]);
}

function sessionIdentityKey(provider: string, sessionId: string): string {
	return JSON.stringify([provider, sessionId]);
}

function backfillSupportsFinalScope(
	status: ModelBackfillStatusData | null,
): boolean {
	return (
		status !== null &&
		status.status === "complete" &&
		status.inventoryComplete &&
		status.failedRoots === 0 &&
		status.failedSources === 0 &&
		status.remainingSources === 0
	);
}

function finalScopeNotice(
	analytics: ModelAnalyticsResponse,
	trustworthyFinal: boolean,
): ModelScopeNotice | null {
	if (!trustworthyFinal) return null;

	// Backend scope facts already exclude suppressed sources. Keep this order
	// authoritative instead of inferring emptiness from model rows or history.
	if (analytics.scope.globalSessionCount === 0) {
		return {
			kind: "global-empty",
			title: "No retained sessions",
			description:
				"No retained session activity is available for model analytics. Model usage will appear after Quill processes session activity.",
		};
	}

	if (analytics.scope.scopedSessionCount === 0) {
		const providerScope =
			analytics.provider === null
				? "all providers"
				: providerLabel(analytics.provider);
		return {
			kind: "filtered-empty",
			title: "No sessions match this scope",
			description: `Retained session activity exists, but none was active for ${providerScope} in ${RANGE_LABELS[analytics.range]}. Change provider or range to inspect other activity.`,
		};
	}

	if (analytics.scope.scopedEvidenceCount === 0) {
		const sessionCount = analytics.scope.scopedSessionCount;
		return {
			kind: "evidence-empty",
			title: "No reliable model evidence",
			description: `${COUNT_FORMATTER.format(sessionCount)} in-scope ${sessionCount === 1 ? "session was" : "sessions were"} found, but retained activity contains no reliable model identifier. Quill leaves that activity unattributed instead of inventing a model.`,
		};
	}

	return null;
}

interface ModelsTabProps {
	range: ModelRange;
	onRangeChange: (range: ModelRange) => void;
}

// @lat: [[frontend#Frontend#Components#Models Composition]]
function ModelsTab({ range, onRangeChange }: ModelsTabProps) {
	const controlId = useId();
	const rangeLabelId = `${controlId}-model-range-label`;
	const providerLabelId = `${controlId}-model-provider-label`;
	const scopeNoticeTitleId = `${controlId}-model-scope-notice-title`;
	const [provider, setProvider] = useState<string | null>(null);
	const [selectedModel, setSelectedModel] =
		useState<ModelIdentity | null>(null);
	const { aggregate, history, backfill, refreshGeneration } = useModelAnalytics(
		range,
		provider,
		selectedModel,
	);
	const modelSessions = useModelSessions(
		selectedModel,
		range,
		refreshGeneration,
	);
	const sessionHistory = useSessionModelHistory(range, refreshGeneration);
	const aggregateData = aggregate.data;
	const providerOptions =
		aggregateData !== null
			? aggregateData.representedProviders
			: provider !== null
				? [provider]
				: [];
	const selectedDetailScopeKey = detailScopeKey(range, selectedModel);
	const [hiddenModelSessions, setHiddenModelSessions] =
		useState<HiddenModelSessions>({
			scopeKey: null,
			sessionKeys: EMPTY_SESSION_KEYS,
		});
	const pageMatchesSelectedModel =
		selectedModel !== null &&
		modelSessions.data !== null &&
		modelSessions.data.identity.provider === selectedModel.provider &&
		modelSessions.data.identity.modelId === selectedModel.modelId;
	const pageSessionKeys = useMemo<ReadonlySet<string>>(() => {
		if (!pageMatchesSelectedModel || modelSessions.data === null) {
			return EMPTY_SESSION_KEYS;
		}

		return new Set(
			modelSessions.data.sessions.map((session) =>
				sessionIdentityKey(session.provider, session.sessionId),
			),
		);
	}, [modelSessions.data, pageMatchesSelectedModel]);
	const activeDetailScopeRef = useRef<ActiveModelDetailScope>({
		scopeKey: selectedDetailScopeKey,
		pageSessionKeys,
	});

	useLayoutEffect(() => {
		activeDetailScopeRef.current = {
			scopeKey: selectedDetailScopeKey,
			pageSessionKeys,
		};
		setHiddenModelSessions((current) => {
			if (current.scopeKey !== selectedDetailScopeKey) {
				return {
					scopeKey: selectedDetailScopeKey,
					sessionKeys: EMPTY_SESSION_KEYS,
				};
			}
			if (
				selectedDetailScopeKey === null ||
				!pageMatchesSelectedModel ||
				current.sessionKeys.size === 0
			) {
				return current;
			}

			const retainedKeys = new Set(
				[...current.sessionKeys].filter((key) => pageSessionKeys.has(key)),
			);
			return retainedKeys.size === current.sessionKeys.size
				? current
				: { scopeKey: selectedDetailScopeKey, sessionKeys: retainedKeys };
		});
	}, [pageMatchesSelectedModel, pageSessionKeys, selectedDetailScopeKey]);

	const hideStaleSession = useCallback(
		(provider: string, sessionId: string) => {
			const activeScope = activeDetailScopeRef.current;
			if (
				activeScope.scopeKey === null ||
				activeScope.scopeKey !== selectedDetailScopeKey
			) {
				return;
			}

			const key = sessionIdentityKey(provider, sessionId);
			if (!activeScope.pageSessionKeys.has(key)) return;

			setHiddenModelSessions((current) => {
				const sessionKeys =
					current.scopeKey === activeScope.scopeKey
						? new Set(current.sessionKeys)
						: new Set<string>();
				if (sessionKeys.has(key)) return current;
				sessionKeys.add(key);
				return { scopeKey: activeScope.scopeKey, sessionKeys };
			});
		},
		[selectedDetailScopeKey],
	);

	const hiddenSessionKeys =
		hiddenModelSessions.scopeKey === selectedDetailScopeKey
			? hiddenModelSessions.sessionKeys
			: EMPTY_SESSION_KEYS;
	const visibleModelSessions = useMemo<UseModelSessionsResult>(() => {
		if (modelSessions.data === null || hiddenSessionKeys.size === 0) {
			return modelSessions;
		}

		const sessions = modelSessions.data.sessions.filter(
			(session) =>
				!hiddenSessionKeys.has(
					sessionIdentityKey(session.provider, session.sessionId),
				),
		);
		return sessions.length === modelSessions.data.sessions.length
			? modelSessions
			: {
					...modelSessions,
					data: { ...modelSessions.data, sessions },
				};
	}, [hiddenSessionKeys, modelSessions]);

	useLayoutEffect(() => {
		if (aggregateData === null) return;

		if (
			provider !== null &&
			!aggregateData.representedProviders.includes(provider)
		) {
			setProvider(null);
		}

		if (selectedModel === null) return;
		const selectedKey = modelIdentityKey(selectedModel);
		const selectionStillRepresented = aggregateData.models.some(
			(row) => modelIdentityKey(row.identity) === selectedKey,
		);
		if (!selectionStillRepresented) setSelectedModel(null);
	}, [aggregateData, provider, selectedModel]);

	const selectProvider = (nextProvider: string | null) => {
		setProvider(nextProvider);
		if (
			nextProvider !== null &&
			selectedModel !== null &&
			selectedModel.provider !== nextProvider
		) {
			setSelectedModel(null);
		}
	};

	const effectiveBackfillStatus =
		aggregateData !== null &&
		(backfill.status === null ||
			aggregateData.backfill.generation > backfill.status.generation)
			? aggregateData.backfill
			: backfill.status;
	const trustworthyFinalScope =
		aggregateData !== null &&
		aggregateData.scope.scopeFinal &&
		aggregateData.scope.inventoryComplete &&
		!backfill.isRetrying &&
		backfillSupportsFinalScope(effectiveBackfillStatus);
	const aggregateProvisional =
		aggregateData !== null && !trustworthyFinalScope;
	const emptyScopeNotice =
		aggregateData === null
			? null
			: finalScopeNotice(aggregateData, trustworthyFinalScope);
	const incompleteScopeTitle =
		effectiveBackfillStatus?.status === "partial" ||
		effectiveBackfillStatus?.status === "failed"
			? "Model scope is incomplete"
			: "Model scope is provisional";

	return (
		<div className="models-tab">
			<div className="models-tab__controls">
				<div
					className="models-filter-group models-filter-group--range"
					role="group"
					aria-labelledby={rangeLabelId}
				>
					<span id={rangeLabelId} className="models-filter-group__label">
						Range
					</span>
					<div className="models-filter-group__buttons">
						{MODEL_RANGES.map((rangeOption) => {
							const pressed = range === rangeOption;
							return (
								<button
									key={rangeOption}
									type="button"
									className={`models-filter-button${pressed ? " models-filter-button--active" : ""}`}
									aria-pressed={pressed}
									onClick={() => onRangeChange(rangeOption)}
								>
									{RANGE_LABELS[rangeOption]}
								</button>
							);
						})}
					</div>
				</div>

				<div
					className="models-filter-group models-filter-group--provider"
					role="group"
					aria-labelledby={providerLabelId}
				>
					<span id={providerLabelId} className="models-filter-group__label">
						Provider
					</span>
					<div className="models-filter-group__buttons">
						<button
							type="button"
							className={`models-filter-button${provider === null ? " models-filter-button--active" : ""}`}
							aria-pressed={provider === null}
							onClick={() => selectProvider(null)}
						>
							All
						</button>
						{providerOptions.map((providerOption) => {
							const pressed = provider === providerOption;
							return (
								<button
									key={providerOption}
									type="button"
									className={`models-filter-button models-filter-button--provider${pressed ? " models-filter-button--active" : ""}`}
									aria-pressed={pressed}
									onClick={() => selectProvider(providerOption)}
								>
									{providerLabel(providerOption)}
								</button>
							);
						})}
					</div>
				</div>
			</div>

			{aggregate.error ? (
				<div
					className="models-tab__request-error models-tab__request-error--aggregate"
					role="alert"
				>
					<span>
						{aggregate.error.message}
						{aggregateData !== null
							? " Last loaded summary and table remain visible."
							: null}
					</span>
					<button
						type="button"
						className="models-tab__retry"
						onClick={aggregate.retry}
					>
						Retry summary and table
					</button>
				</div>
			) : null}

			<ModelBackfillStatus
				status={effectiveBackfillStatus}
				isRetrying={backfill.isRetrying}
				retryError={backfill.retryError}
				onRetry={backfill.retry}
			/>

			{aggregateProvisional ? (
				<section
					className="models-tab__scope-notice models-tab__scope-notice--provisional"
					aria-labelledby={scopeNoticeTitleId}
				>
					<h2 id={scopeNoticeTitleId}>{incompleteScopeTitle}</h2>
					<p>
						Retained history has not reached a trustworthy final state.
						 Recovered summary, history, and model rows remain available;
						 zero counts do not yet prove that sessions or model evidence
						 are absent.
					</p>
				</section>
			) : emptyScopeNotice ? (
				<section
					className={`models-tab__scope-notice models-tab__scope-notice--${emptyScopeNotice.kind}`}
					aria-labelledby={scopeNoticeTitleId}
					role="status"
					aria-live="polite"
					aria-atomic="true"
				>
					<h2 id={scopeNoticeTitleId}>{emptyScopeNotice.title}</h2>
					<p>{emptyScopeNotice.description}</p>
				</section>
			) : null}

			<div className="models-tab__region models-tab__region--summary">
				<ModelSummaryStrip
					summary={aggregateData?.summary ?? null}
					isLoading={aggregate.initialLoading}
					isRefreshing={aggregate.refreshing}
				/>
			</div>

			<div className="models-tab__region models-tab__region--history">
				<ModelUsageHistory
					history={history.data}
					initialLoading={history.initialLoading}
					refreshing={history.refreshing}
					error={history.error}
					onRetry={history.retry}
				/>
			</div>

			<div className="models-tab__region models-tab__region--table">
				<ModelUsageTable
					rows={aggregateData?.models ?? null}
					selectedModel={selectedModel}
					initialLoading={aggregate.initialLoading}
					refreshing={aggregate.refreshing}
					onRetry={aggregate.retry}
					onSelectModel={setSelectedModel}
				/>
			</div>

			<div className="models-tab__region models-tab__region--detail">
				<ModelDetailPanel
					selectedModel={selectedModel}
					modelSessions={visibleModelSessions}
					sessionHistory={sessionHistory}
					onStaleSession={hideStaleSession}
				/>
			</div>
		</div>
	);
}

export default ModelsTab;
