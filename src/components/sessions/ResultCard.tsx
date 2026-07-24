import DOMPurify from "dompurify";
import type { SearchHit, SessionCodeStats } from "../../types";
import { providerLabel } from "../../utils/providers";
import { isPruned, PRUNED_PLACEHOLDER } from "../../utils/retention";

interface ResultCardProps {
	hit: SearchHit;
	selected: boolean;
	locStats: SessionCodeStats | null;
	/**
	 * Retention watermark, or null when nothing has been pruned. Feature 014:
	 * `get_batch_session_code_stats` reads `tool_actions`, so a hit older than
	 * the cutoff comes back all-zero. Search itself is unaffected — it reads the
	 * Tantivy index, which retention never touches — which is exactly why a hit
	 * can outlive its own line counts.
	 */
	retentionCutoff: string | null;
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

function ResultCard({
	hit,
	selected,
	locStats,
	retentionCutoff,
	onSelect,
}: ResultCardProps) {
	// Sanitize snippet HTML -- only <mark> tags allowed for search highlighting
	const sanitized = DOMPurify.sanitize(hit.snippet, {
		ALLOWED_TAGS: ["mark"],
	});
	// Only claim loss when the stats are actually empty: a pre-cutoff session
	// can still carry live rows (`source_key IS NULL`) and non-conforming
	// timestamps, both of which retention leaves in place.
	const locPruned =
		isPruned(hit.timestamp, retentionCutoff) &&
		(!locStats || (locStats.lines_added === 0 && locStats.lines_removed === 0));
	const meta = [providerLabel(hit.provider), hit.project, hit.host, hit.git_branch, timeAgo(hit.timestamp)]
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
					{providerLabel(hit.provider)}
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
				{locPruned ? (
					<>
						{" \u00B7 "}
						<span
							className="sessions-loc-pruned"
							title="Code stats for this session were pruned by retention. This is missing data, not a session that changed no code."
						>
							{PRUNED_PLACEHOLDER} code stats pruned
						</span>
					</>
				) : (
					locStats &&
					(locStats.lines_added > 0 || locStats.lines_removed > 0) && (
						<>
							{" \u00B7 "}
							<span style={{ color: "#22c55e" }}>+{locStats.lines_added}</span>
							{" "}
							<span style={{ color: "#f87171" }}>-{locStats.lines_removed}</span>
						</>
					)
				)}
			</div>
		</div>
	);
}

export default ResultCard;
