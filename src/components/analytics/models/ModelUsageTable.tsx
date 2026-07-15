import {
	memo,
	useCallback,
	useEffect,
	useId,
	useMemo,
	useRef,
	useState,
} from "react";
import type {
	ModelAnalyticsError,
	ModelIdentity,
	ModelUsageRow,
} from "../../../types";
import { modelIdentityKey } from "../../../types";

const COUNT_FORMATTER = new Intl.NumberFormat("en-US");
const PERCENT_FORMATTER = new Intl.NumberFormat("en-US", {
	maximumFractionDigits: 1,
});
const DATE_TIME_FORMATTER = new Intl.DateTimeFormat(undefined, {
	year: "numeric",
	month: "short",
	day: "2-digit",
	hour: "2-digit",
	minute: "2-digit",
	timeZoneName: "short",
});

type SortDirection = "ascending" | "descending";
type SortKey =
	| "provider"
	| "modelId"
	| "attributedTokens"
	| "attributedSharePercent"
	| "inputTokens"
	| "outputTokens"
	| "cacheCreationTokens"
	| "cacheReadTokens"
	| "observedTurns"
	| "sessionCount"
	| "cacheReadSharePercent"
	| "firstSeen"
	| "lastSeen";

interface SortState {
	key: SortKey;
	direction: SortDirection;
}

interface ModelUsageRowView {
	row: ModelUsageRow;
	key: string;
	originalIndex: number;
	firstSeenMs: number;
	lastSeenMs: number;
	providerLabel: string;
	providerClass: string;
	attributedTokens: string;
	attributedShare: string | null;
	inputTokens: string;
	outputTokens: string;
	cacheCreationTokens: string;
	cacheReadTokens: string;
	observedTurns: string;
	sessionCount: string;
	cacheReadShare: string | null;
	firstSeen: string;
	lastSeen: string;
}

interface ColumnDefinition {
	key: SortKey;
	label: string;
	numeric?: boolean;
}

const COLUMNS: readonly ColumnDefinition[] = [
	{ key: "provider", label: "Provider" },
	{ key: "modelId", label: "Model ID" },
	{ key: "attributedTokens", label: "Attributed tokens", numeric: true },
	{ key: "attributedSharePercent", label: "Attributed share", numeric: true },
	{ key: "inputTokens", label: "Input tokens", numeric: true },
	{ key: "outputTokens", label: "Output tokens", numeric: true },
	{ key: "cacheCreationTokens", label: "Cache creation tokens", numeric: true },
	{ key: "cacheReadTokens", label: "Cache read tokens", numeric: true },
	{ key: "observedTurns", label: "Turns", numeric: true },
	{ key: "sessionCount", label: "Sessions", numeric: true },
	{ key: "cacheReadSharePercent", label: "Cache-read share", numeric: true },
	{ key: "firstSeen", label: "First seen" },
	{ key: "lastSeen", label: "Last seen" },
];

export interface ModelUsageTableProps {
	rows: readonly ModelUsageRow[] | null;
	selectedModel: ModelIdentity | null;
	initialLoading?: boolean;
	refreshing?: boolean;
	error?: ModelAnalyticsError | null;
	onRetry: () => void;
	onSelectModel: (identity: ModelIdentity | null) => void;
}

function compareUnicodeScalars(left: string, right: string): number {
	let leftIndex = 0;
	let rightIndex = 0;

	while (leftIndex < left.length && rightIndex < right.length) {
		const leftPoint = left.codePointAt(leftIndex);
		const rightPoint = right.codePointAt(rightIndex);
		if (leftPoint === undefined || rightPoint === undefined) break;
		if (leftPoint !== rightPoint) return leftPoint < rightPoint ? -1 : 1;
		leftIndex += leftPoint > 0xffff ? 2 : 1;
		rightIndex += rightPoint > 0xffff ? 2 : 1;
	}

	if (leftIndex === left.length && rightIndex === right.length) return 0;
	return leftIndex === left.length ? -1 : 1;
}

function compareIdentity(left: ModelIdentity, right: ModelIdentity): number {
	const providerOrder = compareUnicodeScalars(left.provider, right.provider);
	return providerOrder !== 0
		? providerOrder
		: compareUnicodeScalars(left.modelId, right.modelId);
}

function compareNumbers(left: number, right: number): number {
	if (left === right) return 0;
	return left < right ? -1 : 1;
}

function compareNullableNumbers(
	left: number | null,
	right: number | null,
	direction: SortDirection,
): number {
	if (left === null && right === null) return 0;
	if (left === null) return 1;
	if (right === null) return -1;
	const comparison = compareNumbers(left, right);
	return direction === "ascending" ? comparison : -comparison;
}

function compareDateValues(
	leftMs: number,
	rightMs: number,
	leftRaw: string,
	rightRaw: string,
): number {
	if (Number.isFinite(leftMs) && Number.isFinite(rightMs)) {
		return compareNumbers(leftMs, rightMs);
	}
	return compareUnicodeScalars(leftRaw, rightRaw);
}

function compareByColumn(
	left: ModelUsageRowView,
	right: ModelUsageRowView,
	sort: SortState,
): number {
	const leftRow = left.row;
	const rightRow = right.row;
	let comparison = 0;

	switch (sort.key) {
		case "provider":
			comparison = compareUnicodeScalars(
				leftRow.identity.provider,
				rightRow.identity.provider,
			);
			break;
		case "modelId":
			comparison = compareUnicodeScalars(
				leftRow.identity.modelId,
				rightRow.identity.modelId,
			);
			break;
		case "attributedTokens":
		case "inputTokens":
		case "outputTokens":
		case "cacheCreationTokens":
		case "cacheReadTokens":
		case "observedTurns":
		case "sessionCount":
			comparison = compareNumbers(leftRow[sort.key], rightRow[sort.key]);
			break;
		case "attributedSharePercent":
		case "cacheReadSharePercent":
			return compareNullableNumbers(
				leftRow[sort.key],
				rightRow[sort.key],
				sort.direction,
			);
		case "firstSeen":
			comparison = compareDateValues(
				left.firstSeenMs,
				right.firstSeenMs,
				leftRow.firstSeen,
				rightRow.firstSeen,
			);
			break;
		case "lastSeen":
			comparison = compareDateValues(
				left.lastSeenMs,
				right.lastSeenMs,
				leftRow.lastSeen,
				rightRow.lastSeen,
			);
			break;
	}

	return sort.direction === "ascending" ? comparison : -comparison;
}

function defaultDirection(key: SortKey): SortDirection {
	return key === "provider" || key === "modelId"
		? "ascending"
		: "descending";
}

function providerLabel(provider: string): string {
	if (provider === "claude") return "Claude";
	if (provider === "codex") return "Codex";
	if (provider === "mini_max") return "MiniMax";
	return provider;
}

function providerClass(provider: string): string {
	if (provider === "claude") return "model-provider-badge--claude";
	if (provider === "codex") return "model-provider-badge--codex";
	if (provider === "mini_max") return "model-provider-badge--mini-max";
	return "model-provider-badge--other";
}

function formatPercent(value: number | null): string | null {
	return value === null ? null : `${PERCENT_FORMATTER.format(value)}%`;
}

function formatDateTime(value: string): string {
	const timestamp = new Date(value);
	return Number.isFinite(timestamp.getTime())
		? DATE_TIME_FORMATTER.format(timestamp)
		: value;
}

function UnavailableValue() {
	return (
		<span className="model-usage-table__unavailable" aria-label="Unavailable">
			—
		</span>
	);
}

function createRowView(
	row: ModelUsageRow,
	originalIndex: number,
): ModelUsageRowView {
	return {
		row,
		key: modelIdentityKey(row.identity),
		originalIndex,
		firstSeenMs: Date.parse(row.firstSeen),
		lastSeenMs: Date.parse(row.lastSeen),
		providerLabel: providerLabel(row.identity.provider),
		providerClass: providerClass(row.identity.provider),
		attributedTokens: COUNT_FORMATTER.format(row.attributedTokens),
		attributedShare: formatPercent(row.attributedSharePercent),
		inputTokens: COUNT_FORMATTER.format(row.inputTokens),
		outputTokens: COUNT_FORMATTER.format(row.outputTokens),
		cacheCreationTokens: COUNT_FORMATTER.format(row.cacheCreationTokens),
		cacheReadTokens: COUNT_FORMATTER.format(row.cacheReadTokens),
		observedTurns: COUNT_FORMATTER.format(row.observedTurns),
		sessionCount: COUNT_FORMATTER.format(row.sessionCount),
		cacheReadShare: formatPercent(row.cacheReadSharePercent),
		firstSeen: formatDateTime(row.firstSeen),
		lastSeen: formatDateTime(row.lastSeen),
	};
}

interface ModelUsageTableRowProps {
	view: ModelUsageRowView;
	selected: boolean;
	onSelectModel: (identity: ModelIdentity | null) => void;
}

const ModelUsageTableRow = memo(function ModelUsageTableRow({
	view,
	selected,
	onSelectModel,
}: ModelUsageTableRowProps) {
	const reactId = useId().replace(/[^a-zA-Z0-9_-]/g, "");
	const providerId = `model-provider-${reactId}`;
	const identityId = `model-identity-${reactId}`;
	const actionId = `model-inspect-action-${reactId}`;
	const { row } = view;

	return (
		<tr
			className={
				selected
					? "model-usage-table__row model-usage-table__row--selected"
					: "model-usage-table__row"
			}
		>
			<td>
				<span
					id={providerId}
					className={`model-provider-badge ${view.providerClass}`}
				>
					{view.providerLabel}
				</span>
			</td>
			<td className="model-usage-table__identity-cell">
				<code className="model-usage-table__model-id">
					<bdi id={identityId} dir="ltr" translate="no">
						{row.identity.modelId}
					</bdi>
				</code>
				<button
					type="button"
					className="model-usage-table__inspect"
					aria-pressed={selected}
					aria-labelledby={`${actionId} ${providerId} ${identityId}`}
					onClick={() => onSelectModel(selected ? null : row.identity)}
				>
					<span id={actionId} className="models-visually-hidden">
						{selected ? "Stop inspecting" : "Inspect"}
					</span>
					<span aria-hidden="true">{selected ? "Selected" : "Inspect"}</span>
				</button>
			</td>
			<td className="model-usage-table__number">{view.attributedTokens}</td>
			<td className="model-usage-table__number">
				{view.attributedShare ?? <UnavailableValue />}
			</td>
			<td className="model-usage-table__number">{view.inputTokens}</td>
			<td className="model-usage-table__number">{view.outputTokens}</td>
			<td className="model-usage-table__number">
				{view.cacheCreationTokens}
			</td>
			<td className="model-usage-table__number">{view.cacheReadTokens}</td>
			<td className="model-usage-table__number">{view.observedTurns}</td>
			<td className="model-usage-table__number">{view.sessionCount}</td>
			<td className="model-usage-table__number">
				{view.cacheReadShare ?? <UnavailableValue />}
			</td>
			<td className="model-usage-table__time">
				<time dateTime={row.firstSeen} title={row.firstSeen}>
					{view.firstSeen}
				</time>
			</td>
			<td className="model-usage-table__time">
				<time dateTime={row.lastSeen} title={row.lastSeen}>
					{view.lastSeen}
				</time>
			</td>
		</tr>
	);
});

function ModelUsageTable({
	rows,
	selectedModel,
	initialLoading = false,
	refreshing = false,
	error = null,
	onRetry,
	onSelectModel,
}: ModelUsageTableProps) {
	const reactId = useId();
	const titleId = `model-usage-table-title-${reactId.replace(/[^a-zA-Z0-9_-]/g, "")}`;
	const [sort, setSort] = useState<SortState>({
		key: "attributedTokens",
		direction: "descending",
	});
	const selectedKey = selectedModel ? modelIdentityKey(selectedModel) : null;
	const onSelectModelRef = useRef(onSelectModel);

	useEffect(() => {
		onSelectModelRef.current = onSelectModel;
	}, [onSelectModel]);

	const selectModel = useCallback((identity: ModelIdentity | null) => {
		onSelectModelRef.current(identity);
	}, []);

	const rowViews = useMemo(() => {
		if (rows === null) return null;
		return rows.map(createRowView);
	}, [rows]);

	const sortedRows = useMemo(() => {
		if (rowViews === null) return null;
		const sortableRows = [...rowViews];

		sortableRows.sort((left, right) => {
			const columnOrder = compareByColumn(left, right, sort);
			if (columnOrder !== 0) return columnOrder;

			const identityOrder = compareIdentity(left.row.identity, right.row.identity);
			if (identityOrder !== 0) return identityOrder;
			return left.originalIndex - right.originalIndex;
		});

		return sortableRows;
	}, [rowViews, sort]);

	const changeSort = (key: SortKey) => {
		setSort((current) =>
			current.key === key
				? {
						key,
						direction:
							current.direction === "ascending"
								? "descending"
								: "ascending",
					}
				: { key, direction: defaultDirection(key) },
		);
	};

	const hasRows = sortedRows !== null && sortedRows.length > 0;
	const busy = initialLoading || refreshing;

	return (
		<section className="model-usage-table" aria-labelledby={titleId}>
			<div className="model-usage-table__header">
				<h2 id={titleId} className="model-usage-table__title">
					Model usage
				</h2>
				{sortedRows !== null ? (
					<span className="model-usage-table__count">
						{COUNT_FORMATTER.format(sortedRows.length)}{" "}
						{sortedRows.length === 1 ? "model" : "models"}
					</span>
				) : null}
			</div>

			{refreshing && sortedRows !== null ? (
				<p
					className="model-usage-table__status models-visually-hidden"
					role="status"
					aria-live="polite"
				>
					Refreshing model usage table.
				</p>
			) : null}

			{error ? (
				<div className="model-usage-table__error" role="alert">
					<span>{error.message}</span>
					<button
						type="button"
						className="model-usage-table__retry"
						onClick={onRetry}
					>
						Retry model table
					</button>
				</div>
			) : null}

			{initialLoading && rows === null ? (
				<p className="model-usage-table__status" role="status">
					Loading model usage table…
				</p>
			) : null}

			<div className="model-usage-table__data" aria-busy={busy}>
				{sortedRows !== null && sortedRows.length === 0 && !error ? (
					<p className="model-usage-table__empty">
						No model usage rows match this scope.
					</p>
				) : null}

				{hasRows ? (
					<div
						className="model-usage-table__scroller"
						role="region"
						aria-label="Model usage table; scroll horizontally for all columns"
						tabIndex={0}
					>
					<table className="model-usage-table__table">
						<caption className="models-visually-hidden">
							Complete model usage. Use column headings to change sorting.
						</caption>
						<thead>
							<tr>
								{COLUMNS.map((column) => {
									const active = sort.key === column.key;
									const nextDirection =
										active && sort.direction === "ascending"
											? "descending"
											: active
												? "ascending"
												: defaultDirection(column.key);

									return (
										<th
											key={column.key}
											scope="col"
											className={
												column.numeric
													? "model-usage-table__head-cell model-usage-table__head-cell--numeric"
													: "model-usage-table__head-cell"
											}
											aria-sort={active ? sort.direction : "none"}
										>
											<button
												type="button"
												className="model-usage-table__sort"
												onClick={() => changeSort(column.key)}
												aria-label={`${column.label}, sort ${nextDirection}`}
											>
												<span>{column.label}</span>
												<span
													className="model-usage-table__sort-indicator"
													aria-hidden="true"
												>
													{active
														? sort.direction === "ascending"
															? "↑"
															: "↓"
														: "↕"}
												</span>
											</button>
										</th>
									);
								})}
							</tr>
						</thead>
						<tbody>
							{sortedRows?.map((view) => (
								<ModelUsageTableRow
									key={view.key}
									view={view}
									selected={view.key === selectedKey}
									onSelectModel={selectModel}
								/>
							))}
						</tbody>
					</table>
					</div>
				) : null}
			</div>
		</section>
	);
}

export default ModelUsageTable;
