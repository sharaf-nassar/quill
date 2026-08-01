import { useMemo } from "react";
import { LineChart, Line, YAxis, ResponsiveContainer } from "recharts";
import { formatTokenCount } from "../../utils/tokens";
import type { TokenDataPoint, RangeType } from "../../types";

const RANGE_DISPLAY: Record<RangeType, string> = {
	"1h": "1h",
	"6h": "6h",
	"24h": "24h",
	"7d": "7d",
	"30d": "30d",
};

interface TokenSparklineProps {
	data: TokenDataPoint[];
	range: RangeType;
}

function TokenSparkline({ data, range }: TokenSparklineProps) {
	const sampled = useMemo(() => {
		if (data.length <= 30) return data;
		const step = Math.ceil(data.length / 30);
		return data.filter((_, i) => i % step === 0);
	}, [data]);

	if (sampled.length < 2) return null;

	const total = sampled.reduce((s, d) => s + d.total_tokens, 0);

	return (
		<div className="token-sparkline-row">
			<span className="token-sparkline-label">
				{formatTokenCount(total)} tokens ({RANGE_DISPLAY[range]})
			</span>
			<div className="token-sparkline-chart">
				<ResponsiveContainer width="100%" height={16}>
					<LineChart
						data={sampled}
						margin={{ top: 2, right: 2, bottom: 2, left: 2 }}
					>
						<YAxis domain={["dataMin", "dataMax"]} hide />
						<Line
							type="monotone"
							dataKey="total_tokens"
							stroke="#60a5fa"
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

export default TokenSparkline;
