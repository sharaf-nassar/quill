import { formatTokenCount } from "../../utils/tokens";
import type { SessionHealthStats } from "../../types";

interface SessionHealthCardProps {
	stats: SessionHealthStats;
}

function formatDuration(seconds: number): string {
	if (seconds < 60) return "< 1 min";
	const minutes = Math.round(seconds / 60);
	if (minutes < 60) return `${minutes} min`;
	const hours = Math.floor(minutes / 60);
	const mins = minutes % 60;
	return mins > 0 ? `${hours}h ${mins}m` : `${hours}h`;
}

function comparisonText(current: number, previous: number, formatter: (n: number) => string): string | null {
	if (previous === 0) return null;
	return `was ${formatter(previous)} last period`;
}

function statusBadge(stats: SessionHealthStats): { label: string; color: string; bg: string } | null {
	if (stats.prev.avgDurationSeconds === 0) return null;
	const durationChange =
		(stats.avgDurationSeconds - stats.prev.avgDurationSeconds) /
		stats.prev.avgDurationSeconds;

	if (durationChange > 0.15) {
		return { label: "sessions growing longer", color: "#f0883e", bg: "#3d2a1a" };
	}
	if (durationChange < -0.15) {
		return { label: "sessions getting shorter", color: "#3fb950", bg: "#0d2818" };
	}
	return { label: "session length stable", color: "#8b949e", bg: "#21262d" };
}

function SessionHealthCard({ stats }: SessionHealthCardProps) {
	const badge = statusBadge(stats);
	const durationComparison = comparisonText(
		stats.avgDurationSeconds,
		stats.prev.avgDurationSeconds,
		(s) => formatDuration(s),
	);
	const tokensComparison = comparisonText(
		stats.avgTokens,
		stats.prev.avgTokens,
		(t) => formatTokenCount(Math.round(t)),
	);

	return (
		<div className="trends-card">
			<div className="trends-card-header">
				<span className="trends-card-title">Session Health</span>
				{badge && (
					<span
						className="trends-card-badge"
						style={{ color: badge.color, background: badge.bg }}
					>
						{badge.label}
					</span>
				)}
			</div>
			<div className="session-health-metrics">
				<div className="session-health-metric">
					<div className="session-health-label">Avg Duration</div>
					<div className="session-health-value" style={{ color: "#f0883e" }}>
						{formatDuration(stats.avgDurationSeconds)}
					</div>
					{durationComparison && (
						<div className="session-health-comparison">{durationComparison}</div>
					)}
				</div>
				<div className="session-health-metric">
					<div className="session-health-label">Avg Tokens/Session</div>
					<div className="session-health-value">
						{formatTokenCount(Math.round(stats.avgTokens))}
					</div>
					{tokensComparison && (
						<div className="session-health-comparison">{tokensComparison}</div>
					)}
				</div>
				<div className="session-health-metric">
					<div className="session-health-label">Sessions/Day</div>
					<div className="session-health-value">
						{stats.sessionsPerDay.toFixed(1)}
					</div>
					{stats.prev.sessionsPerDay > 0 && (
						<div className="session-health-comparison">
							was {stats.prev.sessionsPerDay.toFixed(1)} last period
						</div>
					)}
				</div>
			</div>
		</div>
	);
}

export default SessionHealthCard;
