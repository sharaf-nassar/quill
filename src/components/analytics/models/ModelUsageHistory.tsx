import { useId, useMemo } from "react";
import {
	Bar,
	CartesianGrid,
	ComposedChart,
	Line,
	ResponsiveContainer,
	Tooltip,
	XAxis,
	YAxis,
} from "recharts";
import type {
	ModelAnalyticsError,
	ModelHistoryPoint,
	ModelHistoryResponse,
	ModelRange,
} from "../../../types";
import { formatTokenCount } from "../../../utils/tokens";

const ATTRIBUTED_COLOR = "rgba(212, 212, 212, 0.72)";
const UNATTRIBUTED_COLOR = "rgba(110, 118, 129, 0.72)";
const SELECTED_COLOR = "#60a5fa";
const AXIS_COLOR = "rgba(255, 255, 255, 0.28)";
const GRID_COLOR = "rgba(255, 255, 255, 0.06)";

const AXIS_TIME_FORMATTERS: Record<ModelRange, Intl.DateTimeFormat> = {
	"1h": new Intl.DateTimeFormat(undefined, {
		hour: "2-digit",
		minute: "2-digit",
		timeZoneName: "short",
	}),
	"24h": new Intl.DateTimeFormat(undefined, {
		month: "short",
		day: "numeric",
		hour: "2-digit",
		minute: "2-digit",
		timeZoneName: "short",
	}),
	"7d": new Intl.DateTimeFormat(undefined, {
		month: "short",
		day: "numeric",
		hour: "2-digit",
		minute: "2-digit",
		timeZoneName: "short",
	}),
	"30d": new Intl.DateTimeFormat(undefined, {
		month: "short",
		day: "numeric",
	}),
};

const BUCKET_TIME_FORMATTER = new Intl.DateTimeFormat(undefined, {
	year: "numeric",
	month: "short",
	day: "numeric",
	hour: "2-digit",
	minute: "2-digit",
	second: "2-digit",
	timeZoneName: "short",
});

interface ChartHistoryPoint extends ModelHistoryPoint {
	bucketCenterMs: number;
}

interface HistoryTooltipPayload {
	payload?: ChartHistoryPoint;
}

interface HistoryTooltipProps {
	active?: boolean;
	payload?: readonly HistoryTooltipPayload[];
}

export interface ModelUsageHistoryProps {
	history: ModelHistoryResponse | null;
	initialLoading: boolean;
	refreshing: boolean;
	error: ModelAnalyticsError | null;
	onRetry: () => void;
}

function toTimestamp(value: string): number | null {
	const timestamp = new Date(value).getTime();
	return Number.isFinite(timestamp) ? timestamp : null;
}

function formatAxisTime(timestamp: number, range: ModelRange): string {
	return AXIS_TIME_FORMATTERS[range].format(timestamp);
}

function formatBucketTime(value: string): string {
	const timestamp = toTimestamp(value);
	if (timestamp === null) return value;
	return BUCKET_TIME_FORMATTER.format(timestamp);
}

function axisTicks(start: number, end: number): number[] {
	if (end <= start) return [start];
	const interval = (end - start) / 4;
	return Array.from({ length: 5 }, (_, index) => start + interval * index);
}

function HistoryTooltip({ active, payload }: HistoryTooltipProps) {
	if (!active) return null;
	const point = payload?.find((entry) => entry.payload)?.payload;
	if (!point) return null;

	return (
		<div className="chart-tooltip model-history-tooltip">
			<div className="chart-tooltip-time">
				{formatBucketTime(point.bucketStart)}–{formatBucketTime(point.bucketEnd)}
			</div>
			<div className="model-history-tooltip-row model-history-tooltip-row--attributed">
				<span>Attributed</span>
				<strong>{formatTokenCount(point.attributedTokens)}</strong>
			</div>
			<div className="model-history-tooltip-row model-history-tooltip-row--unattributed">
				<span>Unattributed</span>
				<strong>{formatTokenCount(point.unattributedTokens)}</strong>
			</div>
			{point.selectedModelTokens !== null ? (
				<div className="model-history-tooltip-row model-history-tooltip-row--selected">
					<span>Selected model</span>
					<strong>{formatTokenCount(point.selectedModelTokens)}</strong>
				</div>
			) : null}
		</div>
	);
}

function HistoryDataTable({ history }: { history: ModelHistoryResponse }) {
	return (
		<table className="model-history-data-table models-visually-hidden">
			<caption>Model token history by time bucket</caption>
			<thead>
				<tr>
					<th scope="col">Bucket start</th>
					<th scope="col">Bucket end</th>
					<th scope="col">Attributed tokens</th>
					<th scope="col">Unattributed tokens</th>
					<th scope="col">Selected model tokens</th>
				</tr>
			</thead>
			<tbody>
				{history.points.map((point) => (
					<tr key={`${point.bucketStart}\u0000${point.bucketEnd}`}>
						<td>
							<time dateTime={point.bucketStart}>{point.bucketStart}</time>
						</td>
						<td>
							<time dateTime={point.bucketEnd}>{point.bucketEnd}</time>
						</td>
						<td>{point.attributedTokens}</td>
						<td>{point.unattributedTokens}</td>
						<td>
							{point.selectedModelTokens === null
								? "No model selected"
								: point.selectedModelTokens}
						</td>
					</tr>
				))}
			</tbody>
		</table>
	);
}

function ModelUsageHistory({
	history,
	initialLoading,
	refreshing,
	error,
	onRetry,
}: ModelUsageHistoryProps) {
	const reactId = useId();
	const titleId = `model-history-title-${reactId.replace(/[^a-zA-Z0-9_-]/g, "")}`;

	const chart = useMemo(() => {
		if (!history || history.points.length === 0) return null;

		const points = history.points.flatMap((point) => {
			const start = toTimestamp(point.bucketStart);
			const end = toTimestamp(point.bucketEnd);
			if (start === null || end === null || end <= start) return [];
			return [{ ...point, bucketCenterMs: start + (end - start) / 2 }];
		});
		if (points.length === 0) return null;

		const rangeStart = toTimestamp(history.points[0].bucketStart);
		const rangeEnd = toTimestamp(
			history.points[history.points.length - 1]?.bucketEnd ?? "",
		);
		if (rangeStart === null || rangeEnd === null || rangeEnd <= rangeStart) {
			return null;
		}

		let aggregateMax = 0;
		for (const point of points) {
			aggregateMax = Math.max(
				aggregateMax,
				point.attributedTokens + point.unattributedTokens,
			);
		}

		return {
			points,
			rangeStart,
			rangeEnd,
			ticks: axisTicks(rangeStart, rangeEnd),
			yMax: Math.max(1, aggregateMax),
		};
	}, [history]);

	if (initialLoading && history === null) {
		return (
			<section
				className="model-history model-history--loading"
				aria-labelledby={titleId}
				aria-busy="true"
			>
				<div className="model-history-heading">
					<h2 id={titleId}>Token history</h2>
				</div>
				<div className="chart-skeleton model-history-skeleton" role="status">
					Loading model token history…
				</div>
			</section>
		);
	}

	if (history === null) {
		return (
			<section className="model-history" aria-labelledby={titleId}>
				<div className="model-history-heading">
					<h2 id={titleId}>Token history</h2>
				</div>
				{error ? (
					<div className="model-history-error" role="alert">
						<span>{error.message}</span>
						<button type="button" onClick={onRetry}>
							Retry history
						</button>
					</div>
				) : (
					<p className="model-history-empty">No model history is available.</p>
				)}
			</section>
		);
	}

	const hasSelectedModel = history.selectedModel !== null;
	const selectedIdentity = history.selectedModel;
	const chartLabel = hasSelectedModel
		? "Model token history. Attributed and unattributed tokens are stacked. The blue line shows the selected model and does not represent model identity."
		: "Model token history. Attributed and unattributed tokens are stacked.";

	return (
		<section
			className="model-history"
			aria-labelledby={titleId}
			aria-busy={refreshing || initialLoading}
		>
			<div className="model-history-heading">
				<h2 id={titleId}>Token history</h2>
				<div className="model-history-legend" aria-label="Chart series">
					<span className="model-history-legend-item model-history-legend-item--attributed">
						<span className="model-history-legend-swatch" aria-hidden="true" />
						Attributed
					</span>
					<span className="model-history-legend-item model-history-legend-item--unattributed">
						<span className="model-history-legend-swatch" aria-hidden="true" />
						Unattributed
					</span>
					{hasSelectedModel ? (
						<span className="model-history-legend-item model-history-legend-item--selected">
							<span className="model-history-legend-swatch" aria-hidden="true" />
							Selected model
							{selectedIdentity ? (
								<span className="model-history-selected-identity" translate="no">
									{selectedIdentity.provider} / {selectedIdentity.modelId}
								</span>
							) : null}
						</span>
					) : null}
				</div>
			</div>

			{error ? (
				<div className="model-history-error model-history-error--retained" role="alert">
					<span>{error.message} Last loaded history remains visible.</span>
					<button type="button" onClick={onRetry}>
						Retry history
					</button>
				</div>
			) : null}
			{refreshing ? (
				<span className="model-history-refreshing" role="status">
					Refreshing model token history…
				</span>
			) : null}

			{chart ? (
				<div className="model-history-chart" role="img" aria-label={chartLabel}>
					<ResponsiveContainer width="100%" height={220}>
						<ComposedChart
							data={chart.points}
							margin={{ top: 10, right: 8, bottom: 2, left: 0 }}
							barCategoryGap="12%"
						>
							<CartesianGrid
								stroke={GRID_COLOR}
								strokeDasharray="3 3"
								vertical={false}
							/>
							<XAxis
								type="number"
								dataKey="bucketCenterMs"
								domain={[chart.rangeStart, chart.rangeEnd]}
								ticks={chart.ticks}
								tickFormatter={(value: number) => formatAxisTime(value, history.range)}
								stroke={AXIS_COLOR}
								fontSize={9}
								tickLine={false}
								axisLine={false}
								interval="preserveStartEnd"
								minTickGap={24}
							/>
							<YAxis
								domain={[0, chart.yMax]}
								allowDataOverflow
								stroke={AXIS_COLOR}
								fontSize={9}
								tickLine={false}
								axisLine={false}
								tickFormatter={formatTokenCount}
								width={38}
							/>
							<Tooltip content={<HistoryTooltip />} cursor={{ fill: "rgba(255, 255, 255, 0.04)" }} />
							<Bar
								dataKey="attributedTokens"
								name="Attributed"
								stackId="coverage"
								fill={ATTRIBUTED_COLOR}
								isAnimationActive={false}
							/>
							<Bar
								dataKey="unattributedTokens"
								name="Unattributed"
								stackId="coverage"
								fill={UNATTRIBUTED_COLOR}
								isAnimationActive={false}
							/>
							{hasSelectedModel ? (
								<Line
									className="model-history-selected-line"
									type="linear"
									dataKey="selectedModelTokens"
									name="Selected model"
									stroke={SELECTED_COLOR}
									strokeWidth={2}
									dot={false}
									activeDot={{ r: 3, fill: SELECTED_COLOR, strokeWidth: 0 }}
									connectNulls={false}
									isAnimationActive={false}
								/>
							) : null}
						</ComposedChart>
					</ResponsiveContainer>
				</div>
			) : (
				<p className="model-history-empty">No valid history buckets were returned.</p>
			)}

			<HistoryDataTable history={history} />
		</section>
	);
}

export default ModelUsageHistory;
