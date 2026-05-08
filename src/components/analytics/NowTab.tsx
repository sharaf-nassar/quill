import { useState } from "react";
import { useAnalyticsData } from "../../hooks/useAnalyticsData";
import { useTokenData } from "../../hooks/useTokenData";
import { useCodeStats } from "../../hooks/useCodeStats";
import { useCodeInsights } from "../../hooks/useCodeInsights";
import { useLlmRuntimeStats } from "../../hooks/useLlmRuntimeStats";
import { useContextSavingsStats } from "../../hooks/useContextSavingsStats";
import { formatNumber, formatDurationSecs } from "../../utils/format";
import InsightCard from "./InsightCard";
import CompactStatsRow from "./CompactStatsRow";
import BreakdownPanel from "./BreakdownPanel";
import TokenSparkline from "./TokenSparkline";
import CodeSparkline from "./CodeSparkline";
import type {
	RangeType,
	BreakdownSelection,
	ContextSavingsSummary,
} from "../../types";

const RANGES: RangeType[] = ["1h", "24h", "7d", "30d"];
const RANGE_LABELS: Record<RangeType, string> = {
	"1h": "1H",
	"24h": "24H",
	"7d": "7D",
	"30d": "30D",
};
const RANGE_DAYS: Record<RangeType, number> = {
	"1h": 1,
	"24h": 1,
	"7d": 7,
	"30d": 30,
};
const DAYS_TO_RANGE: Record<number, RangeType> = {
	1: "24h",
	7: "7d",
	30: "30d",
};

const BREAKDOWN_COLLAPSED_KEY = "quill-breakdown-collapsed";

function formatCompactTokens(value: number | null | undefined): string | null {
	if (value === null || value === undefined) return null;
	return new Intl.NumberFormat("en-US", {
		notation: "compact",
		maximumFractionDigits: value >= 1000 ? 1 : 0,
	}).format(value);
}

function formatCompactCount(value: number | null | undefined): string {
	if (value === null || value === undefined) return "—";
	return new Intl.NumberFormat("en-US", {
		notation: "compact",
		maximumFractionDigits: value >= 1000 ? 1 : 0,
	}).format(value);
}

function formatBytes(value: number | null | undefined): string {
	if (value === null || value === undefined) return "—";
	if (value < 1024) return `${value} B`;
	const units = ["KB", "MB", "GB", "TB"];
	let scaled = value / 1024;
	let unitIndex = 0;
	while (scaled >= 1024 && unitIndex < units.length - 1) {
		scaled /= 1024;
		unitIndex += 1;
	}
	return `${scaled >= 10 ? scaled.toFixed(0) : scaled.toFixed(1)} ${units[unitIndex]}`;
}

function preservedRetention(summary: ContextSavingsSummary): string {
	const sourcesPreserved = summary.sourcesPreserved ?? 0;
	const sourcesRetrieved = summary.sourcesRetrieved ?? 0;
	if (sourcesPreserved === 0) return "no sources yet";
	const ratio = summary.retentionRatio ?? sourcesRetrieved / sourcesPreserved;
	const pct = Math.round(ratio * 100);
	return `${pct}% reused · ${formatCompactCount(sourcesRetrieved)}/${formatCompactCount(sourcesPreserved)} sources`;
}

interface NowTabProps {
	range: RangeType;
	onRangeChange: (r: RangeType) => void;
}

function NowTab({ range, onRangeChange }: NowTabProps) {
	const [breakdownSelection, setBreakdownSelection] =
		useState<BreakdownSelection | null>(null);
	const [breakdownCollapsed, setBreakdownCollapsed] = useState(() => {
		try {
			return localStorage.getItem(BREAKDOWN_COLLAPSED_KEY) === "true";
		} catch {
			return false;
		}
	});
	const breakdownDays = RANGE_DAYS[range] ?? 1;
	const hasSelection = breakdownSelection !== null;
	const tokenRange: RangeType = hasSelection
		? (DAYS_TO_RANGE[breakdownDays] ?? "24h")
		: range;

	const { loading, error } = useAnalyticsData(
		null,
		range,
	);

	const runtimeStats = useLlmRuntimeStats(range);
	const contextSavings = useContextSavingsStats(range, 1);
	const contextSummary = contextSavings.data?.summary ?? null;

	const tokenHostname =
		breakdownSelection?.type === "host" ? breakdownSelection.key : null;
	const tokenSessionId =
		breakdownSelection?.type === "session"
			? breakdownSelection.sessionId ?? null
			: null;
	const tokenProvider =
		breakdownSelection?.type === "session"
			? breakdownSelection.provider ?? null
			: null;
	const tokenCwd =
		breakdownSelection?.type === "project" ? breakdownSelection.key : null;
	const { history: tokenHistory, stats: tokenStats } = useTokenData(
		tokenRange,
		tokenProvider,
		tokenHostname,
		tokenSessionId,
		tokenCwd,
	);

	const { stats: codeStats, history: codeHistory } = useCodeStats(range);
	const { efficiency: efficiencyStats, velocity: velocityStats } = useCodeInsights(range);

	return (
		<>
			<div className="analytics-controls">
				<div className={`range-tabs${hasSelection ? " dimmed" : ""}`}>
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
			</div>

			<TokenSparkline data={tokenHistory} range={tokenRange} />
			<CodeSparkline data={codeHistory} range={range} />

			{error && (
				<div className="analytics-error" role="alert">
					Failed to load analytics
				</div>
			)}

			{loading ? (
				<>
					<div className="chart-skeleton" />
					<div className="breakdown-skeleton">
						<div className="breakdown-skeleton-row" />
						<div className="breakdown-skeleton-row" />
						<div className="breakdown-skeleton-row" />
					</div>
				</>
			) : (
				<>
					{/*
					 * Insight cards grid \u2014 interleaved row-by-row so CSS Grid
					 * stretches each row pair to equal height. Source order:
					 * row 1 (LLM Runtime, Preserved), row 2 (Efficiency,
					 * Retrieved), row 3 (Velocity, Routing cost).
					 */}
					<div className="insight-cards-row">
						<InsightCard
							label="LLM Runtime"
							value={
								runtimeStats.totalRuntimeSecs !== null
									? formatDurationSecs(runtimeStats.totalRuntimeSecs)
									: null
							}
							subtitle={
								runtimeStats.totalRuntimeSecs !== null
									? `${runtimeStats.sessionCount} sessions \u00b7 ${runtimeStats.turnCount} turns \u00b7 avg ${formatDurationSecs(runtimeStats.avgPerTurnSecs)}`
									: "no data"
							}
							trend={null}
							sparkline={runtimeStats.sparkline}
							accentColor="#34d399"
							description="Cumulative wall-clock time the LLM spent generating responses across every turn in this window. Subtitle breaks the total into session count, turn count, and average duration per turn."
						/>
						<InsightCard
							label="Preserved"
							value={formatCompactTokens(contextSummary?.tokensPreserved)}
							unit="tok"
							subtitle={
								contextSummary
									? preservedRetention(contextSummary)
									: "no data"
							}
							trend={null}
							accentColor="#34d399"
							description="Tokens written to local Quill storage instead of staying in the live LLM transcript. Subtitle shows how many indexed sources were later read back at least once."
						/>
						<InsightCard
							label="Efficiency"
							value={
								efficiencyStats.tokensPerLoc !== null
									? formatNumber(efficiencyStats.tokensPerLoc)
									: null
							}
							subtitle="tokens per line of code"
							trend={efficiencyStats.trend}
							sparkline={efficiencyStats.sparkline}
							accentColor="#58a6ff"
							description="Total input + output tokens divided by lines of code added or modified — lower is better. The percent badge compares this window to the prior equal-length window."
						/>
						<InsightCard
							label="Retrieved"
							value={formatCompactTokens(contextSummary?.tokensRetrieved)}
							unit="tok"
							subtitle={
								contextSummary
									? `${formatBytes(contextSummary.returnedBytes)} returned`
									: "no data"
							}
							trend={null}
							accentColor="#58a6ff"
							description="Tokens read back from the context store on demand via quill_get_context_source. Bytes returned reflects the raw payload size of those reads."
						/>
						<InsightCard
							label="Velocity"
							value={
								velocityStats.locPerHour !== null
									? formatNumber(velocityStats.locPerHour)
									: null
							}
							subtitle="lines changed per hour"
							trend={velocityStats.trend}
							sparkline={velocityStats.sparkline}
							accentColor="#a78bfa"
							description="Lines of code added or modified per hour of wall-clock time in this window — higher is better. The percent badge compares this window to the prior equal-length window."
						/>
						<InsightCard
							label="Routing cost"
							value={formatCompactTokens(contextSummary?.tokensRouting)}
							unit="tok"
							subtitle={
								contextSummary
									? `${formatCompactCount(contextSummary.routingEventCount ?? contextSummary.routerEventCount)} guidance events`
									: "no data"
							}
							trend={null}
							accentColor="#fbbf24"
							description="Transcript tokens spent on router nudges, capture guidance, search snippets, and bounded MCP results \u2014 overhead Quill adds to keep larger payloads out of the live transcript."
						/>
					</div>

					{/* Compact tokens + code row */}
					<CompactStatsRow tokenStats={tokenStats} codeStats={codeStats} />

					{/* Breakdown */}
					<div className="breakdown-collapsible">
						<button
							className="breakdown-collapse-toggle"
							onClick={() => {
								const next = !breakdownCollapsed;
								setBreakdownCollapsed(next);
								try {
									localStorage.setItem(BREAKDOWN_COLLAPSED_KEY, String(next));
								} catch { /* ignore */ }
							}}
							aria-expanded={!breakdownCollapsed}
							aria-label={breakdownCollapsed ? "Show breakdown" : "Hide breakdown"}
						>
							<span className="breakdown-collapse-chevron">
								{breakdownCollapsed ? "\u25B8" : "\u25BE"}
							</span>
							<span className="section-title" style={{ marginBottom: 0 }}>Breakdown</span>
						</button>
						{!breakdownCollapsed && (
							<BreakdownPanel
								days={RANGE_DAYS[range] ?? 1}
								selection={breakdownSelection}
								onSelect={setBreakdownSelection}
							/>
						)}
					</div>
				</>
			)}
		</>
	);
}

export default NowTab;
