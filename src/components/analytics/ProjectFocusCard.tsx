import { formatTokenCount } from "../../utils/tokens";
import type { ProjectTokensRaw } from "../../types";

interface ProjectFocusCardProps {
	data: ProjectTokensRaw[];
}

const BAR_COLORS = ["#58a6ff", "#a78bfa", "#f0883e", "#3fb950", "#f87171"];

function projectName(path: string): string {
	const segments = path.split("/").filter(Boolean);
	return segments.length > 0 ? segments[segments.length - 1] : path;
}

function ProjectFocusCard({ data }: ProjectFocusCardProps) {
	const total = data.reduce((sum, row) => sum + row.total_tokens, 0);
	const top5 = data.slice(0, 5);
	const otherTokens = data.slice(5).reduce((sum, row) => sum + row.total_tokens, 0);

	const items = [
		...top5.map((row) => ({
			name: projectName(row.project),
			tokens: row.total_tokens,
			sessions: row.session_count,
		})),
		...(otherTokens > 0 ? [{ name: "Other", tokens: otherTokens, sessions: 0 }] : []),
	];

	return (
		<div className="trends-card">
			<span className="trends-card-title">Project Focus</span>
			<div className="project-focus-bars">
				{items.map((item, i) => {
					const pct = total > 0 ? (item.tokens / total) * 100 : 0;
					return (
						<div key={item.name} className="project-focus-item">
							<div className="project-focus-row">
								<span className="project-focus-name">
									{item.name}
									{item.sessions > 0 && (
										<span className="project-focus-sessions">
											{item.sessions} sess
										</span>
									)}
								</span>
								<span className="project-focus-stats">
									{Math.round(pct)}% &middot; {formatTokenCount(item.tokens)}
								</span>
							</div>
							<div className="project-focus-bar-bg">
								<div
									className="project-focus-bar-fill"
									style={{
										width: `${pct}%`,
										background: BAR_COLORS[i % BAR_COLORS.length],
									}}
								/>
							</div>
						</div>
					);
				})}
			</div>
		</div>
	);
}

export default ProjectFocusCard;
