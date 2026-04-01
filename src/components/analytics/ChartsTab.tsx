import { useMemo, useEffect, useState, useCallback } from "react";
import {
	AreaChart,
	Area,
	BarChart,
	Bar,
	XAxis,
	YAxis,
	CartesianGrid,
	ReferenceLine,
	ResponsiveContainer,
} from "recharts";
import { useAnalyticsData } from "../../hooks/useAnalyticsData";
import { useTokenData } from "../../hooks/useTokenData";
import { useCodeStats } from "../../hooks/useCodeStats";
import { useCacheEfficiency } from "../../hooks/useCacheEfficiency";
import { formatTokenCount } from "../../utils/tokens";
import {
	formatTime,
	dedupeTickLabels,
	anchorToNow,
	getAreaColor,
} from "../../utils/chartHelpers";
import { CrosshairProvider, useCrosshair } from "./ChartCrosshairContext";
import MiniChart from "./MiniChart";
import type { RangeType, UsageBucket } from "../../types";

const RANGES: RangeType[] = ["1h", "24h", "7d", "30d"];
const RANGE_LABELS: Record<RangeType, string> = {
	"1h": "1H",
	"24h": "24H",
	"7d": "7D",
	"30d": "30D",
};

/**
 * Unified tooltip that reads crosshair position and shows all 4 values.
 *
 * Uses useState intentionally — tooltip content changes on every mouse move
 * and must trigger a re-render. Only this component re-renders on hover;
 * the 4 MiniCharts use ref-based DOM updates and do NOT re-render.
 */
interface UnifiedTooltipProps {
	utilData: { timestamp: string; utilization: number }[];
	tokenData: { timestamp: string; total_tokens: number }[];
	codeData: { timestamp: string; lines_added: number; lines_removed: number }[];
	cacheData: { timestamp: string; hitRate: number }[];
}

function UnifiedTooltip({ utilData, tokenData, codeData, cacheData }: UnifiedTooltipProps) {
	const { subscribe } = useCrosshair();
	const [values, setValues] = useState<{
		time: string; util: string; tokens: string; code: string; cache: string;
	} | null>(null);
	const [xPct, setXPct] = useState<number | null>(null);

	const getValueAtPct = useCallback(
		(pct: number) => {
			const idx = (arr: { timestamp: string }[]) =>
				arr.length > 0 ? Math.round(pct * (arr.length - 1)) : -1;

			const ui = idx(utilData);
			const ti = idx(tokenData);
			const ci = idx(codeData);
			const ki = idx(cacheData);

			const ts = utilData[ui]?.timestamp ?? tokenData[ti]?.timestamp ?? "";
			return {
				time: ts ? new Date(ts).toLocaleString([], { month: "short", day: "numeric", hour: "2-digit", minute: "2-digit" }) : "",
				util: ui >= 0 ? `${utilData[ui].utilization.toFixed(0)}%` : "\u2014",
				tokens: ti >= 0 ? formatTokenCount(tokenData[ti].total_tokens) : "\u2014",
				code: ci >= 0 ? `+${codeData[ci].lines_added} / -${codeData[ci].lines_removed}` : "\u2014",
				cache: ki >= 0 ? `${cacheData[ki].hitRate.toFixed(0)}%` : "\u2014",
			};
		},
		[utilData, tokenData, codeData, cacheData],
	);

	useEffect(() => {
		return subscribe((pct) => {
			if (pct === null) {
				setXPct(null);
				setValues(null);
			} else {
				setXPct(pct);
				setValues(getValueAtPct(pct));
			}
		});
	}, [subscribe, getValueAtPct]);

	if (!values || xPct === null) return null;

	return (
		<div
			className="charts-unified-tooltip chart-tooltip"
			style={{ left: `${xPct * 100}%` }}
		>
			<div className="chart-tooltip-time">{values.time}</div>
			<div className="chart-tooltip-value" style={{ color: "#34d399" }}>Util {values.util}</div>
			<div className="chart-tooltip-value" style={{ color: "#60a5fa" }}>Tokens {values.tokens}</div>
			<div className="chart-tooltip-value" style={{ color: "#a78bfa" }}>Code {values.code}</div>
			<div className="chart-tooltip-value" style={{ color: "#fbbf24" }}>Cache {values.cache}</div>
		</div>
	);
}

interface ChartsTabProps {
	range: RangeType;
	onRangeChange: (r: RangeType) => void;
	currentBucket: UsageBucket | null;
}

function ChartsTab({ range, onRangeChange, currentBucket }: ChartsTabProps) {
	const { history: utilHistory, loading: utilLoading, error: utilError } = useAnalyticsData(
		currentBucket,
		range,
	);

	const { history: tokenHistory, loading: tokenLoading, error: tokenError } = useTokenData(
		range,
		null,
		null,
		null,
		null,
	);

	const { history: codeHistory, loading: codeLoading, error: codeError } = useCodeStats(range);

	const cacheData = useCacheEfficiency(tokenHistory);

	// Anchor data to "now" so idle gaps are visible
	const anchoredUtil = useMemo(
		() => anchorToNow(utilHistory, { utilization: 0 }),
		[utilHistory],
	);

	const anchoredTokens = useMemo(
		() =>
			anchorToNow(tokenHistory, {
				input_tokens: 0,
				output_tokens: 0,
				cache_creation_input_tokens: 0,
				cache_read_input_tokens: 0,
				total_tokens: 0,
			}),
		[tokenHistory],
	);

	const anchoredCode = useMemo(() => {
		const anchored = anchorToNow(codeHistory, {
			lines_added: 0,
			lines_removed: 0,
			total_changed: 0,
		});
		// Negate lines_removed for diverging bar chart
		return anchored.map((d) => ({
			...d,
			lines_removed_neg: -d.lines_removed,
		}));
	}, [codeHistory]);

	const anchoredCache = useMemo(
		() => anchorToNow(cacheData, { hitRate: 0 }),
		[cacheData],
	);

	// Current values for display
	const utilColor = getAreaColor(anchoredUtil);
	const utilValue =
		anchoredUtil.length > 0
			? `${anchoredUtil[anchoredUtil.length - 1].utilization.toFixed(0)}%`
			: "\u2014";

	const tokenValue =
		tokenHistory.length > 0
			? formatTokenCount(
				tokenHistory.reduce((sum, d) => sum + d.total_tokens, 0),
			)
			: "\u2014";

	const codeNet =
		codeHistory.length > 0
			? codeHistory.reduce((sum, d) => sum + d.lines_added - d.lines_removed, 0)
			: 0;
	const codeValue =
		codeHistory.length > 0
			? `${codeNet >= 0 ? "+" : ""}${codeNet}`
			: "\u2014";

	const cacheValue =
		cacheData.length > 0
			? `${cacheData[cacheData.length - 1].hitRate.toFixed(0)}%`
			: "\u2014";

	// Shared axis formatting
	const formatter = (v: string) => formatTime(v, range);

	// Compute ticks from the longest dataset for the shared axis
	const longestData = [anchoredUtil, anchoredTokens, anchoredCode, anchoredCache]
		.reduce((a, b) => (a.length >= b.length ? a : b), []);
	const axisTicks = dedupeTickLabels(longestData, formatter);
	const axisTimestamps = longestData
		.filter((_, i) => axisTicks.has(i))
		.map((d) => d.timestamp);

	const isLoading = utilLoading || tokenLoading || codeLoading;

	// Chart grid config — shared across all charts
	const gridProps = {
		strokeDasharray: "3 3",
		stroke: "rgba(255,255,255,0.06)",
		vertical: false,
	};

	const xAxisProps = {
		dataKey: "timestamp" as const,
		tickFormatter: formatter,
		stroke: "rgba(255,255,255,0.2)",
		fontSize: 9,
		tickLine: false,
		axisLine: false,
		minTickGap: 50,
		hide: true,
	};

	return (
		<>
			<div className="analytics-controls">
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
			</div>

			{isLoading ? (
				<div className="charts-stack">
					<div className="chart-skeleton" style={{ height: 80 }} />
					<div className="chart-skeleton" style={{ height: 80 }} />
					<div className="chart-skeleton" style={{ height: 100 }} />
					<div className="chart-skeleton" style={{ height: 80 }} />
				</div>
			) : (
				<CrosshairProvider>
					<div className="charts-stack">
						{/* 1. Utilization */}
						<MiniChart
							label="Utilization"
							currentValue={utilValue}
							color={utilColor}
							height={80}
							isEmpty={anchoredUtil.length === 0}
							emptyText="No utilization data"
							error={utilError}
						>
							<ResponsiveContainer width="100%" height="100%">
								<AreaChart
									data={anchoredUtil}
									margin={{ top: 16, right: 0, left: 0, bottom: 0 }}
								>
									<defs>
										<linearGradient id="grad-util" x1="0" y1="0" x2="0" y2="1">
											<stop offset="5%" stopColor={utilColor} stopOpacity={0.15} />
											<stop offset="95%" stopColor={utilColor} stopOpacity={0.01} />
										</linearGradient>
									</defs>
									<CartesianGrid {...gridProps} />
									<XAxis {...xAxisProps} />
									<YAxis
										domain={[0, 100]}
										ticks={[0, 50, 100]}
										stroke="rgba(255,255,255,0.2)"
										fontSize={9}
										tickLine={false}
										axisLine={false}
										tickFormatter={(v) => `${v}%`}
										width={0}
										hide
									/>
									<ReferenceLine y={80} stroke="rgba(248,113,113,0.3)" strokeDasharray="4 4" />
									<ReferenceLine y={50} stroke="rgba(251,191,36,0.2)" strokeDasharray="4 4" />
									<Area
										type="monotone"
										dataKey="utilization"
										stroke={utilColor}
										strokeWidth={1.5}
										fill="url(#grad-util)"
										dot={false}
										isAnimationActive={false}
									/>
								</AreaChart>
							</ResponsiveContainer>
						</MiniChart>

						{/* 2. Token Breakdown */}
						<MiniChart
							label="Tokens"
							currentValue={tokenValue}
							color="#60a5fa"
							height={80}
							isEmpty={anchoredTokens.length === 0}
							emptyText="No token data"
							error={tokenError}
						>
							<ResponsiveContainer width="100%" height="100%">
								<AreaChart
									data={anchoredTokens}
									margin={{ top: 16, right: 0, left: 0, bottom: 0 }}
									stackOffset="none"
								>
									<defs>
										<linearGradient id="grad-tok-out" x1="0" y1="0" x2="0" y2="1">
											<stop offset="5%" stopColor="#60a5fa" stopOpacity={0.2} />
											<stop offset="95%" stopColor="#60a5fa" stopOpacity={0.02} />
										</linearGradient>
										<linearGradient id="grad-tok-in" x1="0" y1="0" x2="0" y2="1">
											<stop offset="5%" stopColor="#818cf8" stopOpacity={0.15} />
											<stop offset="95%" stopColor="#818cf8" stopOpacity={0.02} />
										</linearGradient>
									</defs>
									<CartesianGrid {...gridProps} />
									<XAxis {...xAxisProps} />
									<YAxis width={0} hide />
									<Area
										type="monotone"
										dataKey="output_tokens"
										stackId="tokens"
										stroke="#60a5fa"
										strokeWidth={1.2}
										fill="url(#grad-tok-out)"
										dot={false}
										isAnimationActive={false}
									/>
									<Area
										type="monotone"
										dataKey="input_tokens"
										stackId="tokens"
										stroke="#818cf8"
										strokeWidth={1.2}
										fill="url(#grad-tok-in)"
										dot={false}
										isAnimationActive={false}
									/>
								</AreaChart>
							</ResponsiveContainer>
						</MiniChart>

						{/* 3. Code Changes */}
						<MiniChart
							label="Code"
							currentValue={codeValue}
							color="#a78bfa"
							height={100}
							isEmpty={anchoredCode.length === 0}
							emptyText="No code changes"
							error={codeError}
						>
							<ResponsiveContainer width="100%" height="100%">
								<BarChart
									data={anchoredCode}
									margin={{ top: 16, right: 0, left: 0, bottom: 0 }}
								>
									<CartesianGrid {...gridProps} />
									<XAxis {...xAxisProps} />
									<YAxis width={0} hide />
									<ReferenceLine y={0} stroke="rgba(255,255,255,0.1)" />
									<Bar
										dataKey="lines_added"
										fill="rgba(52,211,153,0.4)"
										stroke="rgba(52,211,153,0.6)"
										strokeWidth={0.5}
										radius={[2, 2, 0, 0]}
										isAnimationActive={false}
									/>
									<Bar
										dataKey="lines_removed_neg"
										fill="rgba(248,113,113,0.3)"
										stroke="rgba(248,113,113,0.5)"
										strokeWidth={0.5}
										radius={[0, 0, 2, 2]}
										isAnimationActive={false}
									/>
								</BarChart>
							</ResponsiveContainer>
						</MiniChart>

						{/* 4. Cache Efficiency */}
						<MiniChart
							label="Cache"
							currentValue={cacheValue}
							color="#fbbf24"
							height={80}
							isEmpty={anchoredCache.length === 0}
							emptyText="No cache data"
							error={tokenError}
						>
							<ResponsiveContainer width="100%" height="100%">
								<AreaChart
									data={anchoredCache}
									margin={{ top: 16, right: 0, left: 0, bottom: 0 }}
								>
									<defs>
										<linearGradient id="grad-cache" x1="0" y1="0" x2="0" y2="1">
											<stop offset="5%" stopColor="#fbbf24" stopOpacity={0.15} />
											<stop offset="95%" stopColor="#fbbf24" stopOpacity={0.01} />
										</linearGradient>
									</defs>
									<CartesianGrid {...gridProps} />
									<XAxis {...xAxisProps} />
									<YAxis domain={[0, 100]} width={0} hide />
									<Area
										type="monotone"
										dataKey="hitRate"
										stroke="#fbbf24"
										strokeWidth={1.5}
										fill="url(#grad-cache)"
										dot={false}
										isAnimationActive={false}
									/>
								</AreaChart>
							</ResponsiveContainer>
						</MiniChart>

						{/* Shared X-axis */}
						<div className="charts-shared-axis">
							{axisTimestamps.map((ts) => (
								<span key={ts}>{formatter(ts)}</span>
							))}
						</div>

						{/* Unified tooltip — inside charts-stack for correct absolute positioning */}
						<UnifiedTooltip
							utilData={anchoredUtil}
							tokenData={anchoredTokens}
							codeData={anchoredCode}
							cacheData={anchoredCache}
						/>
					</div>
				</CrosshairProvider>
			)}
		</>
	);
}

export default ChartsTab;
