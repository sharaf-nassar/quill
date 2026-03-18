import type { ActivityPatternData } from "../../types";

interface ActivityHeatmapProps {
	data: ActivityPatternData;
}

const HOUR_LABELS: Record<number, string> = {
	0: "12a",
	3: "3a",
	6: "6a",
	9: "9a",
	12: "12p",
	15: "3p",
	18: "6p",
	21: "9p",
};

function intensityColor(value: number, max: number): string {
	if (max === 0 || value === 0) return "rgba(255,255,255,0.03)";
	const ratio = value / max;
	if (ratio > 0.75) return "#39d353";
	if (ratio > 0.5) return "#26a641";
	if (ratio > 0.25) return "#006d32";
	return "#0e4429";
}

function formatPeakHours(start: number, end: number): string {
	const fmt = (h: number) => {
		if (h === 0) return "12am";
		if (h === 12) return "12pm";
		return h < 12 ? `${h}am` : `${h - 12}pm`;
	};
	return `Peak: ${fmt(start)} - ${fmt((end + 1) % 24)}`;
}

function ActivityHeatmap({ data }: ActivityHeatmapProps) {
	const max = Math.max(...data.hourlyTokens);

	return (
		<div className="trends-card">
			<div className="trends-card-header">
				<span className="trends-card-title">Activity Patterns</span>
				<span className="trends-card-subtitle">
					{formatPeakHours(data.peakStart, data.peakEnd)}
				</span>
			</div>
			<div className="activity-heatmap">
				{data.hourlyTokens.map((tokens, hour) => (
					<div key={hour} className="activity-heatmap-slot">
						{HOUR_LABELS[hour] !== undefined && (
							<div className="activity-heatmap-label">{HOUR_LABELS[hour]}</div>
						)}
						<div
							className="activity-heatmap-cell"
							style={{ background: intensityColor(tokens, max) }}
							title={`${hour}:00 \u2014 ${tokens.toLocaleString()} tokens`}
						/>
					</div>
				))}
			</div>
		</div>
	);
}

export default ActivityHeatmap;
