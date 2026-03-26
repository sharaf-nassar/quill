import { useMemo } from "react";
import { LineChart, Line, YAxis, ResponsiveContainer } from "recharts";
import { formatNumber } from "../../utils/format";
import type { CodeStatsHistoryPoint, RangeType } from "../../types";

const RANGE_DISPLAY: Record<RangeType, string> = {
	"1h": "1h",
	"24h": "24h",
	"7d": "7d",
	"30d": "30d",
};

interface CodeSparklineProps {
	data: CodeStatsHistoryPoint[];
	range: RangeType;
}

function CodeSparkline({ data, range }: CodeSparklineProps) {
	const sampled = useMemo(() => {
		if (data.length <= 30) return data;
		const step = Math.ceil(data.length / 30);
		return data.filter((_, i) => i % step === 0);
	}, [data]);

	if (sampled.length < 2) return null;

	const total = sampled.reduce((s, d) => s + d.total_changed, 0);

	return (
		<div className="code-sparkline-row">
			<span className="code-sparkline-label">
				{formatNumber(total)} lines ({RANGE_DISPLAY[range]})
			</span>
			<div className="code-sparkline-chart">
				<ResponsiveContainer width="100%" height={16}>
					<LineChart
						data={sampled}
						margin={{ top: 2, right: 2, bottom: 2, left: 2 }}
					>
						<YAxis domain={["dataMin", "dataMax"]} hide />
						<Line
							type="monotone"
							dataKey="total_changed"
							stroke="#a78bfa"
							strokeWidth={1}
							dot={false}
							animationDuration={200}
						/>
					</LineChart>
				</ResponsiveContainer>
			</div>
		</div>
	);
}

export default CodeSparkline;
