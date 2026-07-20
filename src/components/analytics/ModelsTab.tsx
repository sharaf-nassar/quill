import {
	useCallback,
	useEffect,
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
	ModelBackfillStatus as ModelBackfillStatusData,
	ModelIdentity,
	ModelRange,
	ModelUsageOverviewResponse,
} from "../../types";
import { modelIdentityKey } from "../../types";
import { formatTokenCount } from "../../utils/tokens";
import ModelActivityChart from "./models/ModelActivityChart";
import ModelBackfillStatus from "./models/ModelBackfillStatus";
import ModelCombinations from "./models/ModelCombinations";
import ModelDelegation from "./models/ModelDelegation";
import ModelDetailPanel from "./models/ModelDetailPanel";
import { buildModelShadeMap, providerLabel } from "./models/modelFormat";
import ModelProjectMatrix from "./models/ModelProjectMatrix";
import ModelRunningNow from "./models/ModelRunningNow";
import ModelUsageSpine from "./models/ModelUsageSpine";

const MODEL_RANGES: readonly ModelRange[] = ["1h", "24h", "7d", "30d"];

const RANGE_LABELS: Record<ModelRange, string> = {
	"1h": "1H",
	"24h": "24H",
	"7d": "7D",
	"30d": "30D",
};

const COUNT_FORMATTER = new Intl.NumberFormat("en-US");
const GENERATED_AT_FORMATTER = new Intl.DateTimeFormat(undefined, {
	hour: "2-digit",
	minute: "2-digit",
	second: "2-digit",
});
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
	overview: ModelUsageOverviewResponse,
	trustworthyFinal: boolean,
): ModelScopeNotice | null {
	if (!trustworthyFinal) return null;

	// Backend scope facts already exclude suppressed sources. Keep this order
	// authoritative instead of inferring emptiness from model rows or history.
	if (overview.scope.globalSessionCount === 0) {
		return {
			kind: "global-empty",
			title: "No retained sessions",
			description:
				"No retained session activity is available for model analytics. Model usage will appear after Quill processes session activity.",
		};
	}

	if (overview.scope.scopedSessionCount === 0) {
		const providerScope =
			overview.provider === null
				? "all providers"
				: providerLabel(overview.provider);
		return {
			kind: "filtered-empty",
			title: "No sessions match this scope",
			description: `Retained session activity exists, but none was active for ${providerScope} in ${RANGE_LABELS[overview.range]}. Change provider or range to inspect other activity.`,
		};
	}

	if (overview.scope.scopedEvidenceCount === 0) {
		const sessionCount = overview.scope.scopedSessionCount;
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
	/** Whether the Models tab is the visible analytics panel. */
	active: boolean;
}

// @lat: [[frontend#Frontend#Components#Models Composition]]
function ModelsTab({ range, onRangeChange, active }: ModelsTabProps) {
	const controlId = useId();
	const rangeLabelId = `${controlId}-model-range-label`;
	const providerLabelId = `${controlId}-model-provider-label`;
	const scopeNoticeTitleId = `${controlId}-model-scope-notice-title`;
	const [provider, setProvider] = useState<string | null>(null);
	const [selectedModel, setSelectedModel] =
		useState<ModelIdentity | null>(null);
	const { overview, backfill, refreshGeneration } = useModelAnalytics(
		range,
		provider,
		active,
	);
	const modelSessions = useModelSessions(
		selectedModel,
		range,
		refreshGeneration,
	);
	const sessionHistory = useSessionModelHistory(range, refreshGeneration);
	const overviewData = overview.data;

	// Dim the page only while a user-initiated scope change (range/provider)
	// is loading — never for silent background refreshes (poll, ingest events),
	// which previously made the whole page flash periodically.
	const scopeKey = `${range}|${provider ?? ""}`;
	const [userScopeChange, setUserScopeChange] = useState(false);
	const prevScopeKeyRef = useRef(scopeKey);
	useLayoutEffect(() => {
		if (prevScopeKeyRef.current !== scopeKey) {
			prevScopeKeyRef.current = scopeKey;
			setUserScopeChange(true);
		}
	}, [scopeKey]);
	const overviewSettled =
		overviewData !== null && !overview.refreshing && !overview.initialLoading;
	useEffect(() => {
		if (overviewSettled) setUserScopeChange(false);
	}, [overviewSettled]);
	const showRefreshDim = overview.refreshing && userScopeChange;
	const providerOptions =
		overviewData !== null
			? overviewData.representedProviders
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
		if (overviewData === null) return;

		if (
			provider !== null &&
			!overviewData.representedProviders.includes(provider)
		) {
			setProvider(null);
		}

		if (selectedModel === null) return;
		const selectedKey = modelIdentityKey(selectedModel);
		const selectionStillRepresented = overviewData.models.some(
			(row) => modelIdentityKey(row.identity) === selectedKey,
		);
		if (!selectionStillRepresented) setSelectedModel(null);
	}, [overviewData, provider, selectedModel]);

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
		overviewData !== null &&
		(backfill.status === null ||
			overviewData.backfill.generation > backfill.status.generation)
			? overviewData.backfill
			: backfill.status;
	const trustworthyFinalScope =
		overviewData !== null &&
		overviewData.scope.scopeFinal &&
		overviewData.scope.inventoryComplete &&
		!backfill.isRetrying &&
		backfillSupportsFinalScope(effectiveBackfillStatus);
	const overviewProvisional = overviewData !== null && !trustworthyFinalScope;
	const emptyScopeNotice =
		overviewData === null
			? null
			: finalScopeNotice(overviewData, trustworthyFinalScope);
	const incompleteScopeTitle =
		effectiveBackfillStatus?.status === "partial" ||
		effectiveBackfillStatus?.status === "failed"
			? "Model scope is incomplete"
			: "Model scope is provisional";

	const shadeMap = useMemo(
		() => buildModelShadeMap(overviewData?.models ?? []),
		[overviewData],
	);

	const selectedRow = useMemo(() => {
		if (selectedModel === null || overviewData === null) return null;
		const selectedKey = modelIdentityKey(selectedModel);
		return (
			overviewData.models.find(
				(row) => modelIdentityKey(row.identity) === selectedKey,
			) ?? null
		);
	}, [overviewData, selectedModel]);

	const deselectModel = useCallback(() => setSelectedModel(null), []);

	const generatedAtMs =
		overviewData !== null ? Date.parse(overviewData.generatedAt) : Number.NaN;

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
									data-provider={providerOption}
									aria-pressed={pressed}
									onClick={() => selectProvider(providerOption)}
								>
									{providerLabel(providerOption)}
								</button>
							);
						})}
					</div>
				</div>

				{overviewData !== null ? (
					<span className="models-tab__sources">
						{COUNT_FORMATTER.format(overviewData.totals.sessions)} sessions ·{" "}
						{COUNT_FORMATTER.format(overviewData.totals.projects)} projects
					</span>
				) : null}
			</div>

			{overview.error ? (
				<div
					className="models-tab__request-error models-tab__request-error--overview"
					role="alert"
				>
					<span>
						{overview.error.message}
						{overviewData !== null
							? " Last loaded usage overview remains visible."
							: null}
					</span>
					<button
						type="button"
						className="models-tab__retry"
						onClick={overview.retry}
					>
						Retry usage overview
					</button>
				</div>
			) : null}

			<ModelBackfillStatus
				status={effectiveBackfillStatus}
				isRetrying={backfill.isRetrying}
				retryError={backfill.retryError}
				onRetry={backfill.retry}
			/>

			{overviewProvisional ? (
				<section
					className="models-tab__scope-notice models-tab__scope-notice--provisional"
					aria-labelledby={scopeNoticeTitleId}
				>
					<h2 id={scopeNoticeTitleId}>{incompleteScopeTitle}</h2>
					<p>
						Retained history has not reached a trustworthy final state.
						 Recovered usage sections remain available; zero counts do not
						 yet prove that sessions or model evidence are absent.
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

			{overviewData === null && overview.initialLoading ? (
				<div
					className="models-tab__loading"
					role="status"
					aria-label="Loading model usage overview"
				>
					<div className="chart-skeleton models-tab__skeleton models-tab__skeleton--short" />
					<div className="chart-skeleton models-tab__skeleton" />
					<div className="chart-skeleton models-tab__skeleton models-tab__skeleton--tall" />
					<div className="chart-skeleton models-tab__skeleton" />
				</div>
			) : null}

			{overviewData !== null ? (
				<div
					className={
						showRefreshDim
							? "models-tab__content models-tab__content--refreshing"
							: "models-tab__content"
					}
					aria-busy={showRefreshDim || undefined}
				>
					{showRefreshDim ? (
						<p
							className="models-visually-hidden"
							role="status"
							aria-live="polite"
							aria-atomic="true"
						>
							Refreshing model usage overview.
						</p>
					) : null}

					<ModelRunningNow
						entries={overviewData.runningNow}
						shadeMap={shadeMap}
					/>

					<ModelUsageSpine
						rows={overviewData.models}
						range={range}
						shadeMap={shadeMap}
						selectedModel={selectedModel}
						onSelectModel={setSelectedModel}
					/>

					<ModelActivityChart
						activity={overviewData.activity}
						models={overviewData.models}
						shadeMap={shadeMap}
					/>

					<ModelProjectMatrix
						matrix={overviewData.projectMatrix}
						models={overviewData.models}
						shadeMap={shadeMap}
					/>

					<ModelCombinations
						combinations={overviewData.combinations}
						shadeMap={shadeMap}
					/>

					<ModelDelegation
						delegation={overviewData.delegation}
						shadeMap={shadeMap}
					/>

					<section className="model-section" aria-label="Inspect">
						<div className="model-section__head">
							<h2 className="model-section__title">Inspect</h2>
						</div>
						{selectedModel !== null ? (
							<ModelDetailPanel
								selectedModel={selectedModel}
								selectedRow={selectedRow}
								shadeMap={shadeMap}
								modelSessions={visibleModelSessions}
								sessionHistory={sessionHistory}
								onStaleSession={hideStaleSession}
								onDeselect={deselectModel}
							/>
						) : (
							<p className="models-tab__inspect-hint">
								Select a model row to open its sessions and per-chain
								timelines.
							</p>
						)}
					</section>

					<p className="models-tab__generated">
						{formatTokenCount(overviewData.totals.attributedTokens)} of{" "}
						{formatTokenCount(overviewData.totals.totalTokens)} attributed ·{" "}
						{COUNT_FORMATTER.format(overviewData.totals.distinctModels)}{" "}
						models · generated{" "}
						<time dateTime={overviewData.generatedAt}>
							{Number.isFinite(generatedAtMs)
								? GENERATED_AT_FORMATTER.format(generatedAtMs)
								: overviewData.generatedAt}
						</time>
					</p>
				</div>
			) : null}
		</div>
	);
}

export default ModelsTab;
