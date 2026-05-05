import type { CSSProperties } from "react";
import { useContextSavingsStats } from "../../hooks/useContextSavingsStats";
import { formatNumber } from "../../utils/format";
import type {
	ContextSavingsBreakdownRow,
	ContextSavingsEvent,
	ContextSavingsSummary,
	ContextSavingsTimeSeriesPoint,
	RangeType,
} from "../../types";

const RANGES: RangeType[] = ["1h", "24h", "7d", "30d"];
const RANGE_LABELS: Record<RangeType, string> = {
	"1h": "1H",
	"24h": "24H",
	"7d": "7D",
	"30d": "30D",
};

interface ContextSavingsTabProps {
	range: RangeType;
	onRangeChange: (r: RangeType) => void;
}

interface ContextStatProps {
	label: string;
	value: string;
	subtitle: string;
	accent: string;
}

function labelize(value: string | null | undefined): string {
	if (!value) return "unknown";
	return value
		.replace(/[_-]+/g, " ")
		.replace(/\b\w/g, (char) => char.toUpperCase());
}

function formatCompact(value: number | null | undefined): string {
	if (value === null || value === undefined) return "-";
	return new Intl.NumberFormat("en-US", {
		notation: "compact",
		maximumFractionDigits: value >= 1000 ? 1 : 0,
	}).format(value);
}

function formatTokens(value: number | null | undefined): string {
	if (value === null || value === undefined) return "-";
	return `${formatCompact(value)} tok`;
}

function formatBytes(value: number | null | undefined): string {
	if (value === null || value === undefined) return "-";
	if (value < 1024) return `${formatNumber(value)} B`;
	const units = ["KB", "MB", "GB", "TB"];
	let scaled = value / 1024;
	let unitIndex = 0;
	while (scaled >= 1024 && unitIndex < units.length - 1) {
		scaled /= 1024;
		unitIndex += 1;
	}
	return `${scaled >= 10 ? scaled.toFixed(0) : scaled.toFixed(1)} ${units[unitIndex]}`;
}

function formatTime(timestamp: string): string {
	const date = new Date(timestamp);
	if (Number.isNaN(date.getTime())) return timestamp;
	return date.toLocaleString(undefined, {
		month: "short",
		day: "numeric",
		hour: "numeric",
		minute: "2-digit",
	});
}

function formatConfidence(
	confidence: ContextSavingsEvent["estimateConfidence"],
): string | null {
	if (confidence === null || confidence === undefined) return null;
	if (typeof confidence === "number") {
		const pct = Math.round(confidence * 100);
		return pct === 100 ? null : `${pct}%`;
	}
	const lower = String(confidence).toLowerCase();
	if (lower === "exact" || lower === "high") return null;
	return labelize(confidence);
}

function formatTimeShort(timestamp: string): string {
	const date = new Date(timestamp);
	if (Number.isNaN(date.getTime())) return timestamp;
	return date.toLocaleTimeString(undefined, {
		hour: "2-digit",
		minute: "2-digit",
		hour12: false,
	});
}

const CATEGORY_COLORS: Record<string, string> = {
	capture: "#34d399",
	source: "#58a6ff",
	router: "#fbbf24",
	decision: "#a78bfa",
	provider: "#f472b6",
};

function categoryColor(eventType: string): string {
	const prefix = eventType.split(/[._\-\s]/)[0]?.toLowerCase() ?? "";
	return CATEGORY_COLORS[prefix] ?? "#6e7681";
}

function primaryBytes(event: ContextSavingsEvent): {
	value: number | null;
	dir: "→" | "←" | "·";
} {
	if (event.indexedBytes && event.indexedBytes > 0) {
		return { value: event.indexedBytes, dir: "→" };
	}
	if (event.returnedBytes && event.returnedBytes > 0) {
		return { value: event.returnedBytes, dir: "←" };
	}
	if (event.inputBytes && event.inputBytes > 0) {
		return { value: event.inputBytes, dir: "·" };
	}
	return { value: null, dir: "·" };
}

function preservedTokens(summary: ContextSavingsSummary): number {
	// Intentionally does NOT fall back to tokensPreservedEst — that legacy
	// column included telemetry events and is the value this fix removes
	// from the headline. A stale backend should display 0 until upgraded.
	return summary.tokensPreserved ?? 0;
}

function retrievedTokens(summary: ContextSavingsSummary): number {
	return summary.tokensRetrieved ?? 0;
}

function routingTokens(summary: ContextSavingsSummary): number {
	return summary.tokensRouting ?? 0;
}

function routingEventCount(summary: ContextSavingsSummary): number {
	// Prefer the category-scoped count (router + capture.guidance + search +
	// bounded mcp.execute results) so it matches the headline tokens.  Fall
	// back to the event-type-only routerEventCount for older backends.
	return summary.routingEventCount ?? summary.routerEventCount;
}

function formatRetention(summary: ContextSavingsSummary): string {
	const sourcesPreserved = summary.sourcesPreserved ?? 0;
	const sourcesRetrieved = summary.sourcesRetrieved ?? 0;
	if (sourcesPreserved === 0) return "no sources yet";
	const ratio = summary.retentionRatio ?? sourcesRetrieved / sourcesPreserved;
	const pct = Math.round(ratio * 100);
	return `${pct}% reused · ${formatCompact(sourcesRetrieved)}/${formatCompact(sourcesPreserved)} sources`;
}

function ContextStat({ label, value, subtitle, accent }: ContextStatProps) {
	return (
		<div className="context-stat">
			<div className="context-stat-label">
				<span
					className="context-stat-swatch"
					style={{ background: accent }}
					aria-hidden="true"
				/>
				{label}
			</div>
			<div className="context-stat-value">{value}</div>
			<div className="context-stat-subtitle">{subtitle}</div>
		</div>
	);
}

function ContextTrend({ points }: { points: ContextSavingsTimeSeriesPoint[] }) {
	const visiblePoints = points.slice(-36);
	// Preserved is the superset; Saved is the in-window subset that has not
	// yet been retrieved.  Use max(preserved, returned) as the bar scale so
	// retrieval-heavy buckets stay visible alongside write-heavy ones.
	const maxValue = Math.max(
		1,
		...visiblePoints.map(
			(point) => Math.max(point.tokensPreservedEst, point.tokensReturnedEst),
		),
	);

	if (visiblePoints.length === 0) {
		return <div className="context-empty-panel">No trend data yet</div>;
	}

	return (
		<div className="context-trend-panel">
			<div className="context-section-header">
				<span className="section-title">Trend</span>
				<div className="context-trend-legend">
					<span><i className="context-legend-preserved" />Preserved</span>
					<span><i className="context-legend-saved" />Still saved</span>
					<span><i className="context-legend-returned" />Returned</span>
				</div>
			</div>
			<div className="context-trend-bars" aria-label="Context savings trend">
				{visiblePoints.map((point) => {
					const preserved = point.tokensPreservedEst;
					const saved = Math.min(point.tokensSavedEst, preserved);
					const returnedFromPreserved = Math.max(0, preserved - saved);
					const returned = point.tokensReturnedEst;
					const preservedHeight = Math.max(0, (preserved / maxValue) * 100);
					const savedHeight = preserved > 0
						? (saved / preserved) * preservedHeight
						: 0;
					const returnedFromPreservedHeight = preserved > 0
						? (returnedFromPreserved / preserved) * preservedHeight
						: 0;
					const returnedHeight = Math.max(2, (returned / maxValue) * 100);
					const key = `${point.timestamp}-${point.eventCount}`;

					return (
						<div
							key={key}
							className="context-trend-column"
							title={`${formatTime(point.timestamp)}: ${formatTokens(preserved)} preserved (${formatTokens(saved)} still saved), ${formatTokens(returned)} returned`}
						>
							<div className="context-trend-stack">
								<div
									className="context-trend-segment context-trend-segment--preserved"
									style={{ height: `${returnedFromPreservedHeight}%` }}
								/>
								<div
									className="context-trend-segment context-trend-segment--saved"
									style={{ height: `${savedHeight}%` }}
								/>
							</div>
							<div
								className="context-trend-returned"
								style={{ height: `${returnedHeight}%` }}
							/>
						</div>
					);
				})}
			</div>
		</div>
	);
}

function BreakdownTable({ rows }: { rows: ContextSavingsBreakdownRow[] }) {
	if (rows.length === 0) {
		return <div className="context-empty-panel">No breakdown data yet</div>;
	}

	const visibleRows = rows.slice(0, 10);
	const maxEventCount = Math.max(1, ...visibleRows.map((r) => r.eventCount));

	return (
		<div className="context-table-panel">
			<div className="context-section-header">
				<span className="section-title">Breakdown</span>
				<span className="context-table-count">{rows.length} kinds</span>
			</div>
			<div className="context-breakdown-table">
				<div className="context-breakdown-row context-breakdown-row--header">
					<span>Event / Source</span>
					<span>Events</span>
					<span className="context-breakdown-numeric--indexed">Indexed</span>
					<span className="context-breakdown-numeric--returned">Returned</span>
					<span>Estimate</span>
				</div>
				{visibleRows.map((row) => {
					const tokens = row.tokensSavedEst + row.tokensPreservedEst;
					const accent = categoryColor(row.eventType);
					const fillPct = (row.eventCount / maxEventCount) * 100;
					const confidence = formatConfidence(row.estimateConfidence);
					return (
						<div
							key={`${row.provider ?? "all"}-${row.eventType}-${row.source}`}
							className="context-breakdown-row"
							style={
								{
									"--bar-width": `${fillPct}%`,
									"--bar-bg": `linear-gradient(90deg, ${accent}26, transparent)`,
								} as CSSProperties
							}
							title={`${labelize(row.eventType)} · ${row.source}\nIndexed ${formatBytes(row.indexedBytes)} · Returned ${formatBytes(row.returnedBytes)}`}
						>
							<div className="context-breakdown-name">
								<span
									className="context-breakdown-dot"
									style={{ background: accent }}
									aria-hidden="true"
								/>
								<span className="context-breakdown-label">
									{labelize(row.eventType)}
								</span>
								{row.provider && (
									<em className={`breakdown-provider-tag ${row.provider}`}>
										{row.provider}
									</em>
								)}
								<span className="context-breakdown-source">{row.source}</span>
							</div>
							<span className="context-breakdown-numeric">
								{formatCompact(row.eventCount)}
							</span>
							<span className="context-breakdown-numeric context-breakdown-numeric--indexed">
								{formatBytes(row.indexedBytes)}
							</span>
							<span className="context-breakdown-numeric context-breakdown-numeric--returned">
								{formatBytes(row.returnedBytes)}
							</span>
							<span className="context-breakdown-numeric context-breakdown-tokens">
								{formatTokens(tokens)}
								{confidence && <small>{confidence}</small>}
							</span>
						</div>
					);
				})}
			</div>
		</div>
	);
}

function RecentEventFeed({ events }: { events: ContextSavingsEvent[] }) {
	if (events.length === 0) {
		return <div className="context-empty-panel">No recent context events</div>;
	}

	return (
		<div className="context-feed-panel">
			<div className="context-section-header">
				<span className="section-title">Recent Events</span>
				<span className="context-table-count">{events.length} events</span>
			</div>
			<div className="context-event-feed">
				{events.map((event) => {
					const estimate =
						(event.tokensSavedEst ?? 0) + (event.tokensPreservedEst ?? 0);
					const reason = event.reason ?? event.decision ?? event.eventType;
					const ref = event.sourceRef ?? event.snapshotRef;
					const accent = categoryColor(event.eventType);
					const bytes = primaryBytes(event);
					const confidence = formatConfidence(event.estimateConfidence);
					const detail =
						`${formatTime(event.timestamp)}\n` +
						`Input ${formatBytes(event.inputBytes)} · ` +
						`Indexed ${formatBytes(event.indexedBytes)} · ` +
						`Returned ${formatBytes(event.returnedBytes)}` +
						(ref ? `\n${ref}` : "");

					return (
						<div
							key={event.eventId}
							className="context-event-line"
							title={detail}
						>
							<span
								className="context-event-dot"
								style={{ background: accent }}
								aria-hidden="true"
							/>
							<time className="context-event-time">
								{formatTimeShort(event.timestamp)}
							</time>
							<span className={`breakdown-provider-tag ${event.provider}`}>
								{event.provider}
							</span>
							<span className="context-event-source">{event.source}</span>
							<span className="context-event-reason">{labelize(reason)}</span>
							<span className="context-event-bytes">
								{bytes.value !== null ? (
									<>
										<em>{bytes.dir}</em>
										{formatBytes(bytes.value)}
									</>
								) : (
									"—"
								)}
							</span>
							<span className="context-event-tokens">
								{formatTokens(estimate)}
								{confidence && <small>{confidence}</small>}
							</span>
						</div>
					);
				})}
			</div>
		</div>
	);
}

function ContextSavingsTab({ range, onRangeChange }: ContextSavingsTabProps) {
	const { data, loading, error } = useContextSavingsStats(range, 40);
	const summary = data?.summary;

	return (
		<>
			<div className="analytics-controls context-controls">
				<div className="range-tabs">
					{RANGES.map((r) => (
						<button
							key={r}
							className={`range-tab${range === r ? " active" : ""}`}
							aria-pressed={range === r}
							onClick={() => onRangeChange(r)}
						>
							{RANGE_LABELS[r]}
						</button>
					))}
				</div>
				{data?.generatedAt && (
					<span className="context-updated-at">
						Updated {formatTime(data.generatedAt)}
					</span>
				)}
			</div>

			{error && (
				<div className="analytics-error" role="alert">
					Failed to load context analytics
				</div>
			)}

			{loading && !data ? (
				<>
					<div className="context-stats-strip context-stats-strip--skeleton">
						<div className="context-stat-skeleton" />
						<div className="context-stat-skeleton" />
						<div className="context-stat-skeleton" />
						<div className="context-stat-skeleton" />
					</div>
					<div className="chart-skeleton" />
				</>
			) : summary ? (
				<>
					<div className="context-stats-strip">
						<ContextStat
							label="Preserved"
							value={formatTokens(preservedTokens(summary))}
							subtitle={formatRetention(summary)}
							accent="#34d399"
						/>
						<ContextStat
							label="Retrieved"
							value={formatTokens(retrievedTokens(summary))}
							subtitle={`${formatBytes(summary.returnedBytes)} returned`}
							accent="#58a6ff"
						/>
						<ContextStat
							label="Routing cost"
							value={formatTokens(routingTokens(summary))}
							subtitle={`${formatCompact(routingEventCount(summary))} guidance events`}
							accent="#fbbf24"
						/>
						<ContextStat
							label="Telemetry"
							value={formatCompact(
								summary.telemetryEventCount ?? summary.continuityEventCount,
							)}
							subtitle={`${formatCompact(summary.eventCount)} total events`}
							accent="#a78bfa"
						/>
					</div>

					<ContextTrend points={data.timeSeries} />
					<BreakdownTable rows={data.breakdowns} />
					<RecentEventFeed events={data.recentEvents} />
				</>
			) : (
				<div className="analytics-empty-state context-empty-state">
					<div className="analytics-empty-title">No context events yet</div>
					<div className="analytics-empty-desc">
						Context analytics will appear after hooks or MCP tools report events.
					</div>
				</div>
			)}
		</>
	);
}

export default ContextSavingsTab;
