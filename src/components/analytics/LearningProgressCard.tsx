import type { LearningStatsData } from "../../types";

interface LearningProgressCardProps {
	stats: LearningStatsData;
}

const BUCKET_COLORS = ["#f85149", "#f0883e", "#d29922", "#3fb950", "#3fb950"];

function LearningProgressCard({ stats }: LearningProgressCardProps) {
	const maxBucket = Math.max(...stats.confidenceBuckets, 1);

	return (
		<div className="trends-card">
			<span className="trends-card-title">Learning</span>
			<div className="learning-stats-row">
				<div className="learning-stat">
					<div className="learning-stat-value" style={{ color: "#3fb950" }}>
						{stats.total}
					</div>
					<div className="learning-stat-label">rules</div>
				</div>
				<div className="learning-stat">
					<div className="learning-stat-value" style={{ color: "#d29922" }}>
						{stats.emerging}
					</div>
					<div className="learning-stat-label">emerging</div>
				</div>
				<div className="learning-stat">
					<div className="learning-stat-value" style={{ color: "#3fb950" }}>
						{stats.confirmed}
					</div>
					<div className="learning-stat-label">confirmed</div>
				</div>
			</div>
			<div className="learning-confidence-label">Confidence distribution</div>
			<div className="learning-confidence-chart">
				{stats.confidenceBuckets.map((count, i) => (
					<div
						key={i}
						className="learning-confidence-bar"
						style={{
							height: `${(count / maxBucket) * 100}%`,
							background: BUCKET_COLORS[i],
						}}
						title={`${i * 20}-${(i + 1) * 20}%: ${count} rules`}
					/>
				))}
			</div>
			<div className="learning-confidence-axis">
				<span>Low</span>
				<span>High</span>
			</div>
			{stats.newThisWeek > 0 && (
				<div className="learning-growth" style={{ color: "#3fb950" }}>
					+{stats.newThisWeek} new rules this week
				</div>
			)}
		</div>
	);
}

export default LearningProgressCard;
