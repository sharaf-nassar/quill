import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { LearnedRule } from "../../types";
import { providerScopeClass, providerScopeLabel } from "../../utils/providers";

interface RuleCardProps {
	rule: LearnedRule;
	onDelete: (name: string) => void;
	onPromote?: (name: string) => Promise<void>;
}

function confidenceColor(confidence: number): string {
	if (confidence >= 0.7) return "#22C55E";
	if (confidence >= 0.4) return "#EAB308";
	return "#EF4444";
}

function RuleCard({ rule, onDelete, onPromote }: RuleCardProps) {
	const [expanded, setExpanded] = useState(false);
	const [content, setContent] = useState<string | null>(null);
	const [loading, setLoading] = useState(false);
	const [confirming, setConfirming] = useState(false);
	const [promoting, setPromoting] = useState(false);
	const color = confidenceColor(rule.confidence);
	const pct = Math.round(rule.confidence * 100);
	const isCandidate = rule.state === "candidate";
	const isAntiPattern = rule.is_anti_pattern;
	const hasFile = rule.file_path.length > 0;

	const canExpand = hasFile || !!rule.content;

	const toggleExpand = async () => {
		if (!canExpand) return;
		if (expanded) {
			setExpanded(false);
			return;
		}
		if (content === null) {
			if (hasFile) {
				setLoading(true);
				try {
					const text = await invoke<string>("read_rule_content", {
						filePath: rule.file_path,
					});
					setContent(text);
				} catch {
					setContent("(Failed to load rule content)");
				} finally {
					setLoading(false);
				}
			} else if (rule.content) {
				setContent(rule.content);
			}
		}
		setExpanded(true);
	};

	return (
		<div className={`learning-rule-card${isCandidate ? " learning-rule-card--candidate" : ""}${isAntiPattern ? " learning-rule-card--anti" : ""}`}>
			<div
				className="learning-rule-header"
				onClick={toggleExpand}
				style={canExpand ? undefined : { cursor: "default" }}
			>
				{canExpand ? (
					<span className="learning-rule-expand">{expanded ? "\u25BE" : "\u25B8"}</span>
				) : (
					<span className="learning-rule-expand">&nbsp;</span>
				)}
				<span className="learning-rule-name">
					{rule.is_anti_pattern && <span className="learning-rule-anti" title="Anti-pattern: avoid this">!</span>}
					{rule.name}
				</span>
        <span className={providerScopeClass(rule.provider_scope)}>
          {providerScopeLabel(rule.provider_scope)}
        </span>
				<span className="learning-rule-confidence" style={{ color }}>
					{rule.confidence.toFixed(2)}
				</span>
				{!hasFile && (
					<span className={`learning-rule-state learning-rule-state--${rule.state}`}>
						{rule.state}
					</span>
				)}
				{!hasFile && onPromote && (
					confirming ? (
						<span className="learning-rule-confirm-row" onClick={(e) => e.stopPropagation()}>
							<span className="learning-rule-confirm-label">Add to rules?</span>
							<button
								className="learning-rule-confirm-yes"
								disabled={promoting}
							onClick={async () => {
								setPromoting(true);
								try {
									await onPromote(rule.name);
								} finally {
									setPromoting(false);
									setConfirming(false);
								}
							}}
							>
								Yes
							</button>
							<button
								className="learning-rule-confirm-no"
								onClick={() => setConfirming(false)}
							>
								No
							</button>
						</span>
					) : (
						<button
							className="learning-rule-promote"
							onClick={(e) => {
								e.stopPropagation();
								setConfirming(true);
							}}
							disabled={!rule.content}
							title={rule.content ? "Promote to active rule" : "No content stored — re-run analysis"}
							aria-label={`Promote rule ${rule.name}`}
						>
							&#x2713;
						</button>
					)
				)}
				<button
					className="learning-rule-delete"
					onClick={(e) => {
						e.stopPropagation();
						onDelete(rule.name);
					}}
					aria-label={`Delete rule ${rule.name}`}
				>
					&times;
				</button>
			</div>
			<div className="learning-rule-bar-track">
				<div
					className="learning-rule-bar-fill"
					style={{ width: `${pct}%`, backgroundColor: color }}
				/>
			</div>
			{(rule.domain || rule.project || (rule.source && rule.source !== "observations")) && (
				<div className="learning-rule-meta">
					{rule.domain && (
						<span className="learning-rule-meta-tag">{rule.domain}</span>
					)}
					{rule.source && rule.source !== "observations" && (
						<span className="learning-rule-meta-tag learning-rule-meta-tag--source">{rule.source}</span>
					)}
					{rule.project && (
						<span className="learning-rule-meta-tag learning-rule-meta-tag--project">{rule.project}</span>
					)}
				</div>
			)}
			{expanded && (
				<pre className="learning-rule-content">
					{loading ? "Loading\u2026" : content}
				</pre>
			)}
		</div>
	);
}

export default RuleCard;
