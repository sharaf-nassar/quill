import { useState, useMemo } from "react";
import { useAnalyticsData } from "../../hooks/useAnalyticsData";
import { useTokenData } from "../../hooks/useTokenData";
import { useCodeStats } from "../../hooks/useCodeStats";
import { useEfficiencyStats } from "../../hooks/useEfficiencyStats";
import { useVelocityStats } from "../../hooks/useVelocityStats";
import InsightCard from "./InsightCard";
import CompactStatsRow from "./CompactStatsRow";
import BreakdownPanel from "./BreakdownPanel";
import type {
	RangeType,
	UsageBucket,
	BreakdownSelection,
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

interface BucketDropdownProps {
	value: string;
	options: string[];
	onChange: (value: string) => void;
}

function BucketDropdown({ value, options, onChange }: BucketDropdownProps) {
	const [open, setOpen] = useState(false);

	return (
		<div className="bucket-dropdown-wrap">
			<button
				className="bucket-dropdown-trigger"
				onClick={() => setOpen((v) => !v)}
				aria-haspopup="listbox"
				aria-expanded={open}
				aria-label={`Select bucket: ${value}`}
			>
				{value}
				<span className="bucket-dropdown-arrow">&#9662;</span>
			</button>
			{open && (
				<div className="bucket-dropdown-menu" role="listbox" aria-label="Usage buckets">
					{options.map((opt) => (
						<button
							key={opt}
							className={`bucket-dropdown-item${opt === value ? " active" : ""}`}
							role="option"
							aria-selected={opt === value}
							onClick={() => {
								onChange(opt);
								setOpen(false);
							}}
						>
							{opt}
						</button>
					))}
				</div>
			)}
		</div>
	);
}

interface NowTabProps {
	range: RangeType;
	onRangeChange: (r: RangeType) => void;
	currentBuckets: UsageBucket[];
}

function NowTab({ range, onRangeChange, currentBuckets }: NowTabProps) {
	const [selectedBucket, setSelectedBucket] = useState(
		() => currentBuckets?.[0]?.label ?? "7 days",
	);
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

	const bucketsKey = (currentBuckets ?? [])
		.map((b) => `${b.label}:${b.utilization}:${b.resets_at ?? ""}`)
		.join(",");
	// eslint-disable-next-line react-hooks/exhaustive-deps -- bucketsKey is an intentional stabilizer
	const stableBuckets = useMemo(() => currentBuckets, [bucketsKey]);

	const { stats, loading, error } = useAnalyticsData(
		selectedBucket,
		range,
		stableBuckets,
	);

	const tokenHostname =
		breakdownSelection?.type === "host" ? breakdownSelection.key : null;
	const tokenSessionId =
		breakdownSelection?.type === "session" ? breakdownSelection.key : null;
	const tokenCwd =
		breakdownSelection?.type === "project" ? breakdownSelection.key : null;
	const { stats: tokenStats } = useTokenData(
		tokenRange,
		tokenHostname,
		tokenSessionId,
		tokenCwd,
	);

	const { stats: codeStats } = useCodeStats(range);

	const efficiencyStats = useEfficiencyStats(range);
	const velocityStats = useVelocityStats(range);

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
				<BucketDropdown
					value={selectedBucket}
					options={(currentBuckets ?? []).map((b) => b.label)}
					onChange={setSelectedBucket}
				/>
			</div>

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
					{/* Insight cards row */}
					<div className="insight-cards-row">
						<InsightCard
							label="Efficiency"
							value={
								efficiencyStats.tokensPerLoc !== null
									? String(efficiencyStats.tokensPerLoc)
									: null
							}
							subtitle="tokens per line of code"
							trend={efficiencyStats.trend}
							sparkline={efficiencyStats.sparkline}
							accentColor="#58a6ff"
						/>
						<InsightCard
							label="Velocity"
							value={
								velocityStats.locPerHour !== null
									? String(velocityStats.locPerHour)
									: null
							}
							subtitle="lines changed per hour"
							trend={velocityStats.trend}
							sparkline={velocityStats.sparkline}
							accentColor="#a78bfa"
						/>
						<InsightCard
							label="Rate Limit"
							value={
								stats
									? `${stats.avg.toFixed(0)}%`
									: null
							}
							subtitle={
								stats
									? `peak ${stats.max.toFixed(0)}% \u00b7 ${Math.round(stats.time_above_80)}m above 80%`
									: "no data"
							}
							trend={
								stats
									? {
											direction: stats.trend === "up" ? "up" : stats.trend === "down" ? "down" : "flat",
											percentage: 0,
											upIsGood: false,
										}
									: null
							}
							accentColor={
								stats
									? stats.avg >= 80
										? "#f87171"
										: stats.avg >= 50
											? "#fbbf24"
											: "#34d399"
									: "#8b949e"
							}
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
