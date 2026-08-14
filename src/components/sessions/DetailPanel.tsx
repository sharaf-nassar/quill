import DOMPurify from "dompurify";
import type { SearchHit, SessionContext, SessionCodeStats } from "../../types";
import { providerLabel } from "../../utils/providers";
import { isPruned, PRUNED_PLACEHOLDER } from "../../utils/retention";
import { timeAgo } from "../../utils/time";
import { ParentSessionLink } from "./ResultCard";

interface DetailPanelProps {
	hit: SearchHit;
	context: SessionContext | null;
	locStats: SessionCodeStats | null;
	/** Retention watermark, or null when nothing has been pruned (feature 014). */
	retentionCutoff: string | null;
	onNavigateSession: (sessionId: string) => void;
}

function DetailPanel({
	hit,
	context,
	locStats,
	retentionCutoff,
	onNavigateSession,
}: DetailPanelProps) {
	// Sanitize snippet HTML -- only <mark> tags allowed for search highlighting
	const sanitized = DOMPurify.sanitize(hit.snippet, {
		ALLOWED_TAGS: ["mark"],
	});
	const providerLabelText = providerLabel(hit.provider);
	// Feature 014: an absent line count below the retention cutoff is missing
	// data, so the drilldown says so instead of rendering nothing at all.
	const locPruned =
		isPruned(hit.timestamp, retentionCutoff) &&
		(!locStats || (locStats.lines_added === 0 && locStats.lines_removed === 0));

	return (
		<div className="sessions-detail">
			<div className="sessions-detail-header">
				<div className="sessions-detail-header-row">
					<span
						className={`sessions-role-icon ${hit.role === "user" ? "user" : "assistant"}`}
					>
						{hit.role === "user" ? "\u2191" : "\u2193"}
					</span>
					<span className="sessions-detail-role">
						{hit.role}
					</span>
					<span className={`sessions-provider-badge ${hit.provider}`}>
						{providerLabelText}
					</span>
					{locPruned ? (
						<span
							className="sessions-detail-loc sessions-loc-pruned"
							title="Code stats for this session were pruned by retention. This is missing data, not a session that changed no code."
						>
							{PRUNED_PLACEHOLDER} pruned
						</span>
					) : (
						locStats &&
						(locStats.lines_added > 0 || locStats.lines_removed > 0) && (
							<span className="sessions-detail-loc">
								<span style={{ color: "#22c55e" }}>+{locStats.lines_added}</span>
								{" "}
								<span style={{ color: "#f87171" }}>-{locStats.lines_removed}</span>
							</span>
						)
					)}
				</div>
				<div
					className="sessions-detail-snippet"
					dangerouslySetInnerHTML={{ __html: sanitized }}
				/>
				<div className="sessions-detail-meta">
					{[providerLabelText, hit.project, hit.host, hit.git_branch, timeAgo(hit.timestamp)]
						.filter(Boolean)
						.join(" \u00B7 ")}
					{hit.provider === "pi" && hit.parent_session_id && (
						<>
							{" \u00B7 "}
							<ParentSessionLink
								parentSessionId={hit.parent_session_id}
								onNavigateSession={onNavigateSession}
							/>
						</>
					)}
				</div>
			</div>

			{context ? (
				<div className="sessions-detail-context">
					{context.messages.map((msg) => (
						<div
							key={msg.message_id}
							className={`sessions-context-msg${msg.is_match ? " match" : ""}`}
						>
							<div className="sessions-context-msg-header">
								<span className="sessions-context-role">{msg.role}</span>
								{msg.tools_used && (
									<span className="sessions-context-tools">
										{msg.tools_used.split(" ").filter(Boolean).map((tool) => (
											<span key={tool} className="sessions-context-tool-badge">
												{tool}
											</span>
										))}
									</span>
								)}
							</div>
							{msg.content ? (
								<span className="sessions-context-text">{msg.content}</span>
							) : msg.tool_summary ? (
								<span className="sessions-context-tool-summary">
									{msg.tool_summary}
								</span>
							) : null}
						</div>
					))}
				</div>
			) : (
				<div className="sessions-detail-loading">Loading context...</div>
			)}
		</div>
	);
}

export default DetailPanel;
