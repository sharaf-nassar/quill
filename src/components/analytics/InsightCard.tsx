import type { InsightTrend, SparklinePoint } from "../../types";

interface InsightCardProps {
	label: string;
	value: string | null;
	unit?: string;
	subtitle: string;
	trend: InsightTrend | null;
	sparkline?: SparklinePoint[];
	accentColor?: string;
	description?: string;
}

function trendColor(trend: InsightTrend): string {
	if (trend.direction === "flat") return "#8b949e";
	const isGood =
		trend.upIsGood === null
			? null
			: trend.direction === "up"
				? trend.upIsGood
				: !trend.upIsGood;
	if (isGood === null) return "#8b949e";
	return isGood ? "#3fb950" : "#f85149";
}

function trendBgColor(trend: InsightTrend): string {
	if (trend.direction === "flat") return "#21262d";
	const isGood =
		trend.upIsGood === null
			? null
			: trend.direction === "up"
				? trend.upIsGood
				: !trend.upIsGood;
	if (isGood === null) return "#21262d";
	return isGood ? "#0d2818" : "#3d1a1a";
}

function trendLabel(trend: InsightTrend): string {
	if (trend.direction === "flat") return "\u2192 steady";
	const arrow = trend.direction === "up" ? "\u25B2" : "\u25BC";
	return `${arrow} ${trend.percentage}%`;
}

function InsightCard({
	label,
	value,
	unit,
	subtitle,
	trend,
	sparkline,
	accentColor = "#58a6ff",
	description,
}: InsightCardProps) {
	const maxVal = sparkline
		? Math.max(...sparkline.map((p) => p.value), 1)
		: 1;

	return (
		<div className="insight-card">
			{description && (
				<>
					<button
						type="button"
						className="insight-card-help"
						aria-label={`About ${label}: ${description}`}
					>
						?
					</button>
					<span
						className="insight-card-tooltip"
						role="tooltip"
						aria-hidden="true"
					>
						{description}
					</span>
				</>
			)}
			<div className="insight-card-header">
				<span className="insight-card-label">{label}</span>
				{trend && (
					<span
						className="insight-card-trend"
						style={{
							color: trendColor(trend),
							background: trendBgColor(trend),
						}}
					>
						{trendLabel(trend)}
					</span>
				)}
			</div>
			<div className="insight-card-value" style={{ color: value ? accentColor : "#484f58" }}>
				{value ?? "\u2014"}
				{value && unit && <span className="insight-card-value-unit">{unit}</span>}
			</div>
			<div className="insight-card-subtitle">{subtitle}</div>
			{sparkline && sparkline.length > 0 && (
				<div className="insight-card-sparkline">
					{sparkline.map((point, i) => (
						<div
							key={i}
							className="insight-card-sparkline-bar"
							style={{
								height: `${(point.value / maxVal) * 100}%`,
								background:
									i === sparkline.length - 1
										? accentColor
										: `${accentColor}33`,
							}}
						/>
					))}
				</div>
			)}
		</div>
	);
}

export default InsightCard;
