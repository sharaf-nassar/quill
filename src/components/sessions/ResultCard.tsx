import DOMPurify from "dompurify";
import type { SearchHit, SessionCodeStats } from "../../types";

interface ResultCardProps {
	hit: SearchHit;
	selected: boolean;
	locStats: SessionCodeStats | null;
	onSelect: () => void;
}

function timeAgo(timestamp: string): string {
	const diff = Date.now() - new Date(timestamp).getTime();
	const minutes = Math.floor(diff / 60_000);
	if (minutes < 1) return "just now";
	if (minutes < 60) return `${minutes}m ago`;
	const hours = Math.floor(minutes / 60);
	if (hours < 24) return `${hours}h ago`;
	const days = Math.floor(hours / 24);
	return `${days}d ago`;
}

function ResultCard({ hit, selected, locStats, onSelect }: ResultCardProps) {
	// Sanitize snippet HTML -- only <mark> tags allowed for search highlighting
	const sanitized = DOMPurify.sanitize(hit.snippet, {
		ALLOWED_TAGS: ["mark"],
	});
	const providerLabel = hit.provider === "claude" ? "Claude" : "Codex";

	const meta = [providerLabel, hit.project, hit.host, hit.git_branch, timeAgo(hit.timestamp)]
		.filter(Boolean)
		.join(" \u00B7 ");

	return (
		<div
			className={`sessions-result-card${selected ? " sessions-result-card--selected" : ""}`}
			onClick={onSelect}
		>
			<div className="sessions-result-header-row">
				<span
					className={`sessions-role-icon ${hit.role === "user" ? "user" : "assistant"}`}
					aria-label={hit.role}
				>
					{hit.role === "user" ? "\u2191" : "\u2193"}
				</span>
				<span className={`sessions-provider-badge ${hit.provider}`}>
					{providerLabel}
				</span>
				<span
					className="sessions-result-snippet"
					dangerouslySetInnerHTML={{ __html: sanitized }}
				/>
				{locStats && locStats.net_change !== 0 && (
					<span
						className={`sessions-loc-pill${locStats.net_change >= 0 ? " positive" : " negative"}`}
					>
						{locStats.net_change >= 0 ? "+" : ""}{locStats.net_change}
					</span>
				)}
			</div>
			<div className="sessions-result-meta">
				{meta}
				{locStats && (locStats.lines_added > 0 || locStats.lines_removed > 0) && (
					<>
						{" \u00B7 "}
						<span style={{ color: "#22c55e" }}>+{locStats.lines_added}</span>
						{" "}
						<span style={{ color: "#f87171" }}>-{locStats.lines_removed}</span>
					</>
				)}
			</div>
		</div>
	);
}

export default ResultCard;
