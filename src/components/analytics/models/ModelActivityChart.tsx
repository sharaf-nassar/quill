import { memo, useMemo } from "react";
import {
	Bar,
	BarChart,
	CartesianGrid,
	LabelList,
	ResponsiveContainer,
	Tooltip,
	XAxis,
	YAxis,
} from "recharts";
import type {
	ModelActivity,
	ModelIdentity,
	ModelIdentityKey,
	ModelUsageOverviewRow,
} from "../../../types";
import { modelIdentityKey } from "../../../types";
import {
	COUNT_FORMATTER,
	type ModelShadeMap,
	ModelSection,
	OTHER_SERIES_COLOR,
	formatClockTime,
	formatDateTime,
	modelShade,
	providerGroupRank,
	providerLabel,
} from "./modelFormat";

const AXIS_COLOR = "rgba(255, 255, 255, 0.28)";
const GRID_COLOR = "rgba(255, 255, 255, 0.06)";
const LABEL_COLOR = "#d4d4d4";
const MAX_NAMED_SERIES = 4;
const DAY_SECONDS = 24 * 60 * 60;

const WEEKDAY_FORMATTER = new Intl.DateTimeFormat(undefined, {
	weekday: "narrow",
});
const DAY_FORMATTER = new Intl.DateTimeFormat(undefined, { day: "numeric" });

interface ActivitySeriesView {
	key: string;
	dataKey: string;
	identity: ModelIdentity | null;
	label: string;
	color: string;
	values: number[];
}

interface ActivityChartRow {
	label: string;
	bucketStart: string;
	total: number;
	[seriesKey: `s${number}`]: number;
}

export interface ModelActivityChartProps {
	activity: ModelActivity;
	models: readonly ModelUsageOverviewRow[];
	shadeMap: ModelShadeMap;
}

function bucketLabel(bucketStart: string, bucketSeconds: number): string {
	const timestamp = new Date(bucketStart);
	if (!Number.isFinite(timestamp.getTime())) return bucketStart;
	return bucketSeconds >= DAY_SECONDS
		? `${WEEKDAY_FORMATTER.format(timestamp)} ${DAY_FORMATTER.format(timestamp)}`
		: formatClockTime(bucketStart);
}

interface ActivityTooltipProps {
	active?: boolean;
	payload?: readonly { payload?: ActivityChartRow }[];
	series: readonly ActivitySeriesView[];
	bucketIndexOf: (row: ActivityChartRow) => number;
}

function ActivityTooltip({
	active,
	payload,
	series,
	bucketIndexOf,
}: ActivityTooltipProps) {
	if (!active) return null;
	const row = payload?.find((entry) => entry.payload)?.payload;
	if (!row) return null;
	const bucketIndex = bucketIndexOf(row);

	return (
		<div className="chart-tooltip model-activity-tooltip">
			<div className="chart-tooltip-time">
				{formatDateTime(row.bucketStart)}
			</div>
			{series.map((entry) => {
				const value = entry.values[bucketIndex] ?? 0;
				if (value === 0) return null;
				return (
					<div key={entry.key} className="model-activity-tooltip__row">
						<span className="model-activity-tooltip__name">
							<span
								className="model-swatch"
								style={{ background: entry.color }}
								aria-hidden="true"
							/>
							<code translate="no">{entry.label}</code>
						</span>
						<strong>
							{COUNT_FORMATTER.format(value)}{" "}
							{value === 1 ? "session" : "sessions"}
						</strong>
					</div>
				);
			})}
			<div className="model-activity-tooltip__row model-activity-tooltip__row--total">
				<span className="model-activity-tooltip__name">Total</span>
				<strong>{COUNT_FORMATTER.format(row.total)}</strong>
			</div>
		</div>
	);
}

/** Round up to a multiple of 4 so quarter ticks stay integers. */
function sessionAxisMax(value: number): number {
	return Math.max(4, Math.ceil(value / 4) * 4);
}

/**
 * Sessions per model per bucket: the top models (delivered order) as
 * provider-grouped stacked series, remainder folded into a neutral "other".
 */
function ModelActivityChart({
	activity,
	models,
	shadeMap,
}: ModelActivityChartProps) {
	const chart = useMemo(() => {
		if (activity.bucketStarts.length === 0 || activity.series.length === 0) {
			return null;
		}

		const bucketCount = activity.bucketStarts.length;
		const seriesByKey = new Map(
			activity.series.map((entry) => [
				modelIdentityKey(entry.identity),
				entry,
			]),
		);

		// Named series: first MAX_NAMED_SERIES delivered models that have
		// activity, kept adjacent per provider in the stack.
		const namedKeys: ModelIdentityKey[] = [];
		for (const row of models) {
			if (namedKeys.length >= MAX_NAMED_SERIES) break;
			const key = modelIdentityKey(row.identity);
			if (seriesByKey.has(key)) namedKeys.push(key);
		}
		const named = namedKeys
			.map((key, deliveredIndex) => ({ key, deliveredIndex }))
			.sort((left, right) => {
				const leftSeries = seriesByKey.get(left.key);
				const rightSeries = seriesByKey.get(right.key);
				if (leftSeries === undefined || rightSeries === undefined) return 0;
				const groupOrder =
					providerGroupRank(leftSeries.identity.provider) -
					providerGroupRank(rightSeries.identity.provider);
				return groupOrder !== 0
					? groupOrder
					: left.deliveredIndex - right.deliveredIndex;
			});

		const namedKeySet = new Set(namedKeys);
		const remainder = activity.series.filter(
			(entry) => !namedKeySet.has(modelIdentityKey(entry.identity)),
		);
		const otherValues = Array.from({ length: bucketCount }, (_, index) =>
			remainder.reduce(
				(sum, entry) => sum + (entry.sessionsPerBucket[index] ?? 0),
				0,
			),
		);

		const series: ActivitySeriesView[] = named.map(({ key }, index) => {
			const entry = seriesByKey.get(key);
			if (entry === undefined) throw new Error("unreachable");
			return {
				key,
				dataKey: `s${index}`,
				identity: entry.identity,
				label: entry.identity.modelId,
				color: modelShade(shadeMap, entry.identity),
				values: Array.from(
					{ length: bucketCount },
					(_, bucketIndex) => entry.sessionsPerBucket[bucketIndex] ?? 0,
				),
			};
		});
		if (remainder.length > 0) {
			series.push({
				key: "__other",
				dataKey: `s${series.length}`,
				identity: null,
				label: `other (${COUNT_FORMATTER.format(remainder.length)} ${remainder.length === 1 ? "model" : "models"})`,
				color: OTHER_SERIES_COLOR,
				values: otherValues,
			});
		}

		const rows: ActivityChartRow[] = activity.bucketStarts.map(
			(bucketStart, index) => {
				const row: ActivityChartRow = {
					label: bucketLabel(bucketStart, activity.bucketSeconds),
					bucketStart,
					total: series.reduce(
						(sum, entry) => sum + (entry.values[index] ?? 0),
						0,
					),
				};
				series.forEach((entry, seriesIndex) => {
					row[`s${seriesIndex}`] = entry.values[index] ?? 0;
				});
				return row;
			},
		);

		const maxTotal = rows.reduce((max, row) => Math.max(max, row.total), 0);
		if (maxTotal === 0) return null;
		const yMax = sessionAxisMax(maxTotal);
		const yTicks = [0, 0.25, 0.5, 0.75, 1].map(
			(fraction) => fraction * yMax,
		);
		const bucketIndexByStart = new Map(
			activity.bucketStarts.map((start, index) => [start, index]),
		);

		return {
			series,
			rows,
			yMax,
			yTicks,
			bucketIndexOf: (row: ActivityChartRow) =>
				bucketIndexByStart.get(row.bucketStart) ?? 0,
		};
	}, [activity, models, shadeMap]);

	const meta =
		activity.bucketSeconds >= DAY_SECONDS
			? "sessions per model per day"
			: "sessions per model per bucket";

	return (
		<ModelSection label="Activity" meta={meta}>
			{chart === null ? (
				<p className="model-section__empty">
					No session activity in this range.
				</p>
			) : (
				<>
					<div
						className="model-activity-chart"
						role="img"
						aria-label="Stacked bar chart of sessions per model per bucket. Stacks group by provider; the neutral segment sums remaining models."
					>
						<ResponsiveContainer width="100%" height={212}>
							<BarChart
								data={chart.rows}
								margin={{ top: 18, right: 8, bottom: 2, left: 0 }}
								barCategoryGap="24%"
							>
								<CartesianGrid
									stroke={GRID_COLOR}
									strokeDasharray="3 3"
									vertical={false}
								/>
								<XAxis
									dataKey="label"
									stroke={AXIS_COLOR}
									fontSize={9}
									tickLine={false}
									axisLine={false}
									interval="preserveStartEnd"
									minTickGap={16}
								/>
								<YAxis
									domain={[0, chart.yMax]}
									ticks={chart.yTicks}
									stroke={AXIS_COLOR}
									fontSize={9}
									tickLine={false}
									axisLine={false}
									allowDecimals={false}
									width={30}
								/>
								<Tooltip
									content={
										<ActivityTooltip
											series={chart.series}
											bucketIndexOf={chart.bucketIndexOf}
										/>
									}
									cursor={{ fill: "rgba(255, 255, 255, 0.04)" }}
								/>
								{chart.series.map((entry, index) => (
									<Bar
										key={entry.key}
										dataKey={entry.dataKey}
										name={entry.label}
										stackId="sessions"
										fill={entry.color}
										isAnimationActive={false}
									>
										{index === chart.series.length - 1 ? (
											<LabelList
												dataKey="total"
												position="top"
												fill={LABEL_COLOR}
												fontSize={10}
												fontWeight={600}
												formatter={(value: unknown) =>
													typeof value === "number" && value > 0
														? COUNT_FORMATTER.format(value)
														: ""
												}
											/>
										) : null}
									</Bar>
								))}
							</BarChart>
						</ResponsiveContainer>
					</div>

					<div className="model-activity-legend" aria-label="Chart series">
						{chart.series.map((entry) => (
							<span key={entry.key} className="model-activity-legend__item">
								<span
									className="model-swatch"
									style={{ background: entry.color }}
									aria-hidden="true"
								/>
								<code translate="no">{entry.label}</code>
								{entry.identity !== null ? (
									<span className="model-activity-legend__provider">
										{providerLabel(entry.identity.provider)}
									</span>
								) : null}
							</span>
						))}
					</div>
					<p className="model-section__foot">
						A session counts once per model it used in a bucket. Hover a
						segment for model and count.
					</p>

					<table className="models-visually-hidden">
						<caption>Sessions per model per time bucket</caption>
						<thead>
							<tr>
								<th scope="col">Bucket start</th>
								{chart.series.map((entry) => (
									<th key={entry.key} scope="col">
										{entry.label} sessions
									</th>
								))}
								<th scope="col">Total sessions</th>
							</tr>
						</thead>
						<tbody>
							{chart.rows.map((row, rowIndex) => (
								<tr key={row.bucketStart}>
									<td>
										<time dateTime={row.bucketStart}>{row.bucketStart}</time>
									</td>
									{chart.series.map((entry) => (
										<td key={entry.key}>{entry.values[rowIndex] ?? 0}</td>
									))}
									<td>{row.total}</td>
								</tr>
							))}
						</tbody>
					</table>
				</>
			)}
		</ModelSection>
	);
}

export default memo(ModelActivityChart);
