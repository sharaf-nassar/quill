import { useEffect, useId, useMemo, useState } from "react";
import type { UseModelSessionsResult } from "../../../hooks/useModelSessions";
import type { UseSessionModelHistoryResult } from "../../../hooks/useSessionModelHistory";
import type {
	ModelIdentity,
	ModelSessionRow,
	SessionModelChain,
	SessionModelHistoryResponse,
	SessionModelSegment,
} from "../../../types";

const COUNT_FORMATTER = new Intl.NumberFormat("en-US");
const DATE_TIME_FORMATTER = new Intl.DateTimeFormat(undefined, {
	year: "numeric",
	month: "short",
	day: "2-digit",
	hour: "2-digit",
	minute: "2-digit",
	second: "2-digit",
	timeZoneName: "short",
});
const STALE_NOTICE_MS = 3_000;

export interface ModelDetailPanelProps {
	selectedModel: ModelIdentity | null;
	modelSessions: UseModelSessionsResult;
	sessionHistory: UseSessionModelHistoryResult;
	onStaleSession: (provider: string, sessionId: string) => void;
}

interface ModelSessionDisclosureProps {
	disclosureKey: string;
	session: ModelSessionRow;
	expanded: boolean;
	sessionHistory: UseSessionModelHistoryResult;
	onExpandedChange: (disclosureKey: string, expanded: boolean) => void;
	onStaleSession: (provider: string, sessionId: string) => void;
}

function formatDateTime(value: string): string {
	const timestamp = new Date(value);
	return Number.isFinite(timestamp.getTime())
		? DATE_TIME_FORMATTER.format(timestamp)
		: value;
}

function modelIdentityKey(identity: ModelIdentity): string {
	return JSON.stringify([identity.provider, identity.modelId]);
}

function sessionIdentityKey(session: ModelSessionRow): string {
	return JSON.stringify([session.provider, session.sessionId]);
}

function disclosureIdentityKey(
	selectedModel: ModelIdentity,
	session: ModelSessionRow,
): string {
	return JSON.stringify([
		selectedModel.provider,
		selectedModel.modelId,
		session.provider,
		session.sessionId,
	]);
}

function Identity({ identity }: { identity: ModelIdentity }) {
	return (
		<span className="model-detail-panel__identity" translate="no">
			<span
				className="model-detail-panel__provider"
				data-provider={identity.provider}
			>
				<bdi dir="ltr">{identity.provider}</bdi>
			</span>
			<span className="model-detail-panel__identity-separator" aria-hidden="true">
				/
			</span>
			<code className="model-detail-panel__model-id">
				<bdi dir="ltr">{identity.modelId}</bdi>
			</code>
		</span>
	);
}

function Timestamp({ value }: { value: string }) {
	return (
		<time dateTime={value} title={value}>
			{formatDateTime(value)}
		</time>
	);
}

function Segment({ segment }: { segment: SessionModelSegment }) {
	return (
		<li
			className={`model-detail-panel__segment model-detail-panel__segment--${segment.kind}`}
		>
			<div className="model-detail-panel__segment-main">
				{segment.kind === "model" ? (
					<Identity identity={segment.identity} />
				) : (
					<span className="model-detail-panel__model-gap">
						Model identity unavailable
					</span>
				)}
				<span className="model-detail-panel__segment-window">
					<Timestamp value={segment.startedAt} />
					<span aria-hidden="true">–</span>
					<Timestamp value={segment.endedAt} />
				</span>
			</div>
			<dl className="model-detail-panel__segment-metrics">
				<div>
					<dt>Turns</dt>
					<dd>{COUNT_FORMATTER.format(segment.turnCount)}</dd>
				</div>
				<div>
					<dt>Attributed tokens</dt>
					<dd>
						{segment.kind === "model"
							? COUNT_FORMATTER.format(segment.attributedTokens)
							: "Not attributed"}
					</dd>
				</div>
			</dl>
		</li>
	);
}

function Chain({ chain }: { chain: SessionModelChain }) {
	return (
		<li
			className={`model-detail-panel__chain model-detail-panel__chain--${chain.kind}`}
		>
			<div className="model-detail-panel__chain-header">
				<div className="model-detail-panel__chain-identity">
					<strong>
						{chain.kind === "parent" ? "Parent chain" : "Subagent chain"}
					</strong>
					<code>
						<bdi dir="ltr" translate="no">
							{chain.chainId}
						</bdi>
					</code>
				</div>
				<dl className="model-detail-panel__chain-metrics">
					<div>
						<dt>Switches</dt>
						<dd>{COUNT_FORMATTER.format(chain.switchCount)}</dd>
					</div>
					<div>
						<dt>Attributed tokens</dt>
						<dd>{COUNT_FORMATTER.format(chain.attributedTokens)}</dd>
					</div>
					<div>
						<dt>Unattributed tokens</dt>
						<dd>{COUNT_FORMATTER.format(chain.unattributedTokens)}</dd>
					</div>
				</dl>
			</div>

			{chain.parentChainId !== null || chain.agentId !== null ? (
				<dl className="model-detail-panel__chain-relations">
					{chain.parentChainId !== null ? (
						<div>
							<dt>Parent chain</dt>
							<dd>
								<code>
									<bdi dir="ltr" translate="no">
										{chain.parentChainId}
									</bdi>
								</code>
							</dd>
						</div>
					) : null}
					{chain.agentId !== null ? (
						<div>
							<dt>Agent</dt>
							<dd>
								<code>
									<bdi dir="ltr" translate="no">
										{chain.agentId}
									</bdi>
								</code>
							</dd>
						</div>
					) : null}
				</dl>
			) : null}

			{chain.segments.length > 0 ? (
				<ol className="model-detail-panel__segments">
					{chain.segments.map((segment, index) => (
						<Segment
							key={`${segment.kind}\u0000${segment.startedAt}\u0000${segment.endedAt}\u0000${index}`}
							segment={segment}
						/>
					))}
				</ol>
			) : (
				<p className="model-detail-panel__empty-chain">
					No model-bearing turns were observed in this chain.
				</p>
			)}
		</li>
	);
}

function SessionHistory({ history }: { history: SessionModelHistoryResponse }) {
	return (
		<div className="model-detail-panel__history-data">
			<dl className="model-detail-panel__history-summary">
				<div>
					<dt>Primary model</dt>
					<dd>
						{history.primaryModel === null ? (
							<span>Unavailable</span>
						) : (
							<Identity identity={history.primaryModel} />
						)}
					</dd>
				</div>
				<div>
					<dt>Distinct models</dt>
					<dd>{COUNT_FORMATTER.format(history.distinctModels)}</dd>
				</div>
				<div>
					<dt>Switches</dt>
					<dd>{COUNT_FORMATTER.format(history.switchCount)}</dd>
				</div>
				<div>
					<dt>Attributed tokens</dt>
					<dd>{COUNT_FORMATTER.format(history.attributedTokens)}</dd>
				</div>
				<div>
					<dt>Unattributed tokens</dt>
					<dd>{COUNT_FORMATTER.format(history.unattributedTokens)}</dd>
				</div>
			</dl>

			{history.chains.length > 0 ? (
				<ol className="model-detail-panel__chains">
					{history.chains.map((chain) => (
						<Chain key={chain.chainId} chain={chain} />
					))}
				</ol>
			) : (
				<p className="model-detail-panel__empty-history">
					No model chain history is available for this range.
				</p>
			)}
		</div>
	);
}

function ModelSessionDisclosure({
	disclosureKey,
	session,
	expanded,
	sessionHistory,
	onExpandedChange,
	onStaleSession,
}: ModelSessionDisclosureProps) {
	const reactId = useId().replace(/[^a-zA-Z0-9_-]/g, "");
	const panelId = `model-session-history-${reactId}`;
	const buttonId = `model-session-disclosure-${reactId}`;
	const { stateFor, setExpanded, retry } = sessionHistory;
	const history = stateFor(session.provider, session.sessionId);

	useEffect(() => {
		if (expanded) {
			setExpanded(session.provider, session.sessionId, true);
		}
		return () => setExpanded(session.provider, session.sessionId, false);
	}, [expanded, session.provider, session.sessionId, setExpanded]);

	useEffect(() => {
		if (!expanded || history.stale === null) {
			return;
		}

		const timeout = window.setTimeout(() => {
			onStaleSession(session.provider, session.sessionId);
		}, STALE_NOTICE_MS);
		return () => window.clearTimeout(timeout);
	}, [
		expanded,
		history.stale,
		onStaleSession,
		session.provider,
		session.sessionId,
	]);

	const toggle = () => {
		const nextExpanded = !expanded;
		onExpandedChange(disclosureKey, nextExpanded);
	};

	return (
		<li className="model-detail-panel__session">
			<button
				id={buttonId}
				type="button"
				className="model-detail-panel__disclosure"
				aria-expanded={expanded}
				aria-controls={panelId}
				onClick={toggle}
			>
				<span className="model-detail-panel__disclosure-marker" aria-hidden="true">
					{expanded ? "−" : "+"}
				</span>
				<span className="model-detail-panel__session-identity">
					<strong>
						<bdi dir="auto" translate="no">
							{session.displayName}
						</bdi>
					</strong>
					<span className="model-detail-panel__session-ref" translate="no">
						<bdi
							className="model-detail-panel__session-provider"
							data-provider={session.provider}
							dir="ltr"
						>
							{session.provider}
						</bdi>
						<span aria-hidden="true"> / </span>
						<code>
							<bdi dir="ltr">{session.sessionId}</bdi>
						</code>
					</span>
				</span>

				<span className="model-detail-panel__session-location">
					{session.cwd !== null ? (
						<code title={session.cwd}>
							<bdi dir="ltr" translate="no">
								{session.cwd}
							</bdi>
						</code>
					) : (
						<span>Working directory unavailable</span>
					)}
					{session.hostname === null ? (
						<span>Host unavailable</span>
					) : (
						<bdi
							className="model-detail-panel__session-hostname"
							dir="auto"
							translate="no"
						>
							{session.hostname}
						</bdi>
					)}
				</span>

				<span className="model-detail-panel__session-usage">
					<span>
						{COUNT_FORMATTER.format(session.selectedModelTokens)} selected-model
						tokens
					</span>
					<span>
						{COUNT_FORMATTER.format(session.selectedModelTurns)} selected-model
						turns
					</span>
					<span>
						Last activity <Timestamp value={session.lastActivityAt} />
					</span>
				</span>

				<span className="model-detail-panel__session-models">
					<span>
						Primary <Identity identity={session.primaryModel} />
					</span>
					<span>
						{COUNT_FORMATTER.format(session.distinctModels)} distinct models ·{" "}
						{COUNT_FORMATTER.format(session.chainCount)} chains
					</span>
					{session.hasWithinChainSwitches ? (
						<span className="model-detail-panel__switch-indicator">
							Within-chain switches
						</span>
					) : (
						<span>No within-chain switches</span>
					)}
				</span>
			</button>

			<div
				id={panelId}
				className="model-detail-panel__history"
				role="region"
				aria-labelledby={buttonId}
				aria-busy={history.initialLoading || history.refreshing || undefined}
				hidden={!expanded}
			>
				{expanded ? (
					<>
						{history.initialLoading && history.data === null ? (
							<p className="model-detail-panel__row-status" role="status">
								Loading session model history…
							</p>
						) : null}

						{history.refreshing ? (
							<p className="model-detail-panel__row-status" role="status">
								Refreshing session model history…
							</p>
						) : null}

						{history.stale !== null ? (
							<p className="model-detail-panel__stale" role="status">
								{history.stale.message} This stale session will be removed from
								this list.
							</p>
						) : null}

						{history.error !== null ? (
							<div className="model-detail-panel__row-error" role="alert">
								<span>
									{history.error.message}
									{history.data !== null
										? " Last loaded session history remains visible."
										: ""}
								</span>
								<button
									type="button"
									className="model-detail-panel__retry"
									aria-label={`Retry session model history for ${session.provider} session ${session.sessionId}`}
									onClick={() => retry(session.provider, session.sessionId)}
								>
									Retry session history
								</button>
							</div>
						) : null}

						{history.data !== null ? (
							<SessionHistory history={history.data} />
						) : null}
					</>
				) : null}
			</div>
		</li>
	);
}

/**
 * Presentational selected-model drill-down. Async ownership stays in the page
 * and history hooks; this component owns only which session disclosures are open.
 */
// @lat: [[frontend#Frontend#Components#Model Session Detail Panel]]
export function ModelDetailPanel({
	selectedModel,
	modelSessions,
	sessionHistory,
	onStaleSession,
}: ModelDetailPanelProps) {
	const reactId = useId().replace(/[^a-zA-Z0-9_-]/g, "");
	const titleId = `model-detail-panel-title-${reactId}`;
	const [expandedKeys, setExpandedKeys] = useState<ReadonlySet<string>>(
		() => new Set(),
	);
	const selectedKey = selectedModel ? modelIdentityKey(selectedModel) : null;
	const selectedProvider = selectedModel?.provider ?? null;
	const selectedModelId = selectedModel?.modelId ?? null;
	const listedSessions = modelSessions.data?.sessions ?? null;
	const representedDisclosureKeys = useMemo(() => {
		if (
			selectedProvider === null ||
			selectedModelId === null ||
			listedSessions === null
		) {
			return new Set<string>();
		}

		const identity = {
			provider: selectedProvider,
			modelId: selectedModelId,
		};
		return new Set(
			listedSessions.map((session) =>
				disclosureIdentityKey(identity, session),
			),
		);
	}, [listedSessions, selectedModelId, selectedProvider]);

	useEffect(() => {
		setExpandedKeys((current) => {
			if (current.size === 0) return current;
			const retained = new Set(
				[...current].filter((key) => representedDisclosureKeys.has(key)),
			);
			return retained.size === current.size ? current : retained;
		});
	}, [representedDisclosureKeys]);

	const setDisclosureExpanded = (
		disclosureKey: string,
		expanded: boolean,
	) => {
		setExpandedKeys((current) => {
			const next = new Set(current);
			if (expanded) next.add(disclosureKey);
			else next.delete(disclosureKey);
			return next;
		});
	};

	if (selectedModel === null) {
		return (
		<section className="model-detail-panel model-detail-panel--idle">
			<p className="model-detail-panel__empty">
				Select a model to inspect its recent sessions.
			</p>
		</section>
		);
	}

	const sessions = modelSessions.data?.sessions ?? null;
	const hasSessions = sessions !== null && sessions.length > 0;
	const busy =
		modelSessions.initial.loading ||
		modelSessions.loadMore.loading ||
		modelSessions.replay.loading;

	return (
		<section
			className="model-detail-panel"
			aria-labelledby={titleId}
			aria-busy={busy || undefined}
		>
			<div className="model-detail-panel__header">
				<div>
					<h2 id={titleId}>Sessions</h2>
					<Identity identity={selectedModel} />
				</div>
				{modelSessions.data !== null ? (
					<span className="model-detail-panel__count">
						{COUNT_FORMATTER.format(modelSessions.data.sessions.length)} of{" "}
						{COUNT_FORMATTER.format(modelSessions.data.total)} sessions
					</span>
				) : null}
			</div>

			{modelSessions.initial.loading && modelSessions.data === null ? (
				<p className="model-detail-panel__status" role="status">
					Loading selected model sessions…
				</p>
			) : null}

			{modelSessions.initial.error !== null && modelSessions.data === null ? (
				<div className="model-detail-panel__error" role="alert">
					<span>{modelSessions.initial.error.message}</span>
					<button
						type="button"
						className="model-detail-panel__retry"
						aria-label="Retry loading selected model sessions"
						onClick={modelSessions.initial.retry}
						disabled={busy}
					>
						Retry loading sessions
					</button>
				</div>
			) : null}

			{modelSessions.replay.loading && modelSessions.data !== null ? (
				<p className="model-detail-panel__status models-visually-hidden" role="status">
					Refreshing selected model sessions…
				</p>
			) : null}
			{modelSessions.replay.loading && modelSessions.data === null ? (
				<p className="model-detail-panel__status" role="status">
					Refreshing selected model sessions…
				</p>
			) : null}

			{modelSessions.replay.error !== null ? (
				<div className="model-detail-panel__error" role="alert">
					<span>
						{modelSessions.replay.error.message}
						{modelSessions.data === null
							? ""
							: " Last loaded sessions remain visible."}
					</span>
					<button
						type="button"
						className="model-detail-panel__retry"
						aria-label="Retry refreshing selected model sessions"
						onClick={modelSessions.replay.retry}
						disabled={busy}
					>
						Retry refreshing sessions
					</button>
				</div>
			) : null}

			{sessions !== null && sessions.length === 0 ? (
				<p className="model-detail-panel__empty">
					No sessions used this model in the selected range.
				</p>
			) : null}

			{hasSessions ? (
				<ol className="model-detail-panel__sessions">
					{sessions.map((session) => {
						const disclosureKey = disclosureIdentityKey(
							selectedModel,
							session,
						);
						return (
							<ModelSessionDisclosure
								key={`${selectedKey}\u0000${sessionIdentityKey(session)}`}
								disclosureKey={disclosureKey}
								session={session}
								expanded={expandedKeys.has(disclosureKey)}
								sessionHistory={sessionHistory}
								onExpandedChange={setDisclosureExpanded}
								onStaleSession={onStaleSession}
							/>
						);
					})}
				</ol>
			) : null}

			{modelSessions.loadMore.error !== null ? (
				<div className="model-detail-panel__error" role="alert">
					<span>
						{modelSessions.loadMore.error.message}
						{modelSessions.data === null
							? ""
							: " Loaded sessions remain visible."}
					</span>
					<button
						type="button"
						className="model-detail-panel__retry"
						aria-label="Retry loading more selected model sessions"
						onClick={modelSessions.loadMore.retry}
						disabled={busy}
					>
						Retry loading more sessions
					</button>
				</div>
			) : null}

			{modelSessions.loadMore.hasMore ? (
				<button
					type="button"
					className="model-detail-panel__load-more"
					aria-label="Load more selected model sessions"
					onClick={modelSessions.loadMore.run}
					disabled={busy}
				>
					{modelSessions.loadMore.loading ? "Loading more…" : "Load more"}
				</button>
			) : null}
		</section>
	);
}

export default ModelDetailPanel;
