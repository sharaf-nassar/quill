import DOMPurify from "dompurify";
import type { SearchHit, SessionContext, SessionCodeStats } from "../../types";

interface DetailPanelProps {
	hit: SearchHit;
	context: SessionContext | null;
	locStats: SessionCodeStats | null;
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

function DetailPanel({ hit, context, locStats }: DetailPanelProps) {
	// Sanitize snippet HTML -- only <mark> tags allowed for search highlighting
	const sanitized = DOMPurify.sanitize(hit.snippet, {
		ALLOWED_TAGS: ["mark"],
	});
	const providerLabel = hit.provider === "claude" ? "Claude" : "Codex";

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
						{providerLabel}
					</span>
					{locStats && (locStats.lines_added > 0 || locStats.lines_removed > 0) && (
						<span className="sessions-detail-loc">
							<span style={{ color: "#22c55e" }}>+{locStats.lines_added}</span>
							{" "}
							<span style={{ color: "#f87171" }}>-{locStats.lines_removed}</span>
						</span>
					)}
				</div>
				<div
					className="sessions-detail-snippet"
					dangerouslySetInnerHTML={{ __html: sanitized }}
				/>
				<div className="sessions-detail-meta">
					{[providerLabel, hit.project, hit.host, hit.git_branch, timeAgo(hit.timestamp)]
						.filter(Boolean)
						.join(" \u00B7 ")}
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
