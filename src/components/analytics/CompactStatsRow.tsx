import { formatTokenCount } from "../../utils/tokens";
import type { TokenStats, CodeStats } from "../../types";

interface CompactStatsRowProps {
	tokenStats: TokenStats | null;
	codeStats: CodeStats | null;
}

function formatNumber(n: number): string {
	if (Math.abs(n) >= 1000) {
		return (n / 1000).toFixed(1).replace(/\.0$/, "") + "k";
	}
	return String(n);
}

function CompactStatsRow({ tokenStats, codeStats }: CompactStatsRowProps) {
	return (
		<div className="compact-stats-row">
			{tokenStats && tokenStats.total_tokens > 0 && (
				<div className="compact-stats-card">
					<div className="compact-stat">
						<span className="compact-stat-label">In</span>
						<span className="compact-stat-value">
							{formatTokenCount(tokenStats.total_input)}
						</span>
					</div>
					<div className="compact-stat">
						<span className="compact-stat-label">Out</span>
						<span className="compact-stat-value">
							{formatTokenCount(tokenStats.total_output)}
						</span>
					</div>
					{tokenStats.total_input + tokenStats.total_cache_read > 0 && (
						<div className="compact-stat">
							<span className="compact-stat-label">Cache</span>
							<span
								className="compact-stat-value"
								style={{
									color:
										tokenStats.total_cache_read /
											(tokenStats.total_input + tokenStats.total_cache_read) >=
										0.6
											? "#22C55E"
											: tokenStats.total_cache_read /
														(tokenStats.total_input +
															tokenStats.total_cache_read) >=
												0.3
												? "#EAB308"
												: "#EF4444",
								}}
							>
								{Math.round(
									(tokenStats.total_cache_read /
										(tokenStats.total_input + tokenStats.total_cache_read)) *
										100,
								)}
								%
							</span>
						</div>
					)}
				</div>
			)}
			{codeStats && (
				<div className="compact-stats-card">
					<div className="compact-stat">
						<span className="compact-stat-label">Added</span>
						<span className="compact-stat-value" style={{ color: "#22c55e" }}>
							+{formatNumber(codeStats.lines_added)}
						</span>
					</div>
					<div className="compact-stat">
						<span className="compact-stat-label">Removed</span>
						<span className="compact-stat-value" style={{ color: "#f87171" }}>
							-{formatNumber(codeStats.lines_removed)}
						</span>
					</div>
					<div className="compact-stat">
						<span className="compact-stat-label">Net</span>
						<span
							className="compact-stat-value"
							style={{
								color: codeStats.net_change >= 0 ? "#22c55e" : "#f87171",
							}}
						>
							{codeStats.net_change >= 0 ? "+" : ""}
							{formatNumber(codeStats.net_change)}
						</span>
					</div>
				</div>
			)}
		</div>
	);
}

export default CompactStatsRow;
