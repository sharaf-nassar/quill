import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { LearnedRule, OperatorFeedback } from "../../types";
import { NON_ACTIVE_LIFECYCLES } from "../../types";
import {
	normalizeProviderScope,
	providerScopeClass,
	providerScopeLabel,
	PROVIDER_ASYMMETRY_DISCLOSURE,
} from "../../utils/providers";

interface RuleCardProps {
	rule: LearnedRule;
	onDelete: (name: string) => void;
	onPromote?: (name: string) => Promise<void>;
	onSubmitFeedback?: (
		name: string,
		feedback: OperatorFeedback,
		note?: string,
	) => Promise<void>;
}

function confidenceColor(confidence: number): string {
	if (confidence >= 0.7) return "#22C55E";
	if (confidence >= 0.4) return "#EAB308";
	return "#EF4444";
}

const FEEDBACK_LABEL: Record<OperatorFeedback, string> = {
	accept: "accepted",
	reject: "rejected",
	bad: "bad",
};

type ConfirmKind = "promote" | "bad";

function RuleCard({ rule, onDelete, onPromote, onSubmitFeedback }: RuleCardProps) {
	const [expanded, setExpanded] = useState(false);
	const [content, setContent] = useState<string | null>(null);
	const [loading, setLoading] = useState(false);
	const [confirmKind, setConfirmKind] = useState<ConfirmKind | null>(null);
	const [promoting, setPromoting] = useState(false);
	const [submittingBad, setSubmittingBad] = useState(false);
	const color = confidenceColor(rule.confidence);
	const pct = Math.round(rule.confidence * 100);
	// Shared/combined scope (feature 005 M-6): a rule attributed to >1 provider
	// is structurally Claude-weighted because Codex only emits Bash hooks.
	// Surface the verbatim asymmetry disclosure on the shared badge only.
	const isSharedScope = normalizeProviderScope(rule.provider_scope).length > 1;
	const isCandidate = rule.state === "candidate";
	const isAntiPattern = rule.is_anti_pattern;
	const hasFile = rule.file_path.length > 0;
	// Show the lifecycle badge for DB-only rules and for on-disk rules whose
	// lifecycle is terminal/superseded (feature 005 US3 — these must read as
	// non-active even though a `.md` still exists on disk).
	const isNonActiveLifecycle = NON_ACTIVE_LIFECYCLES.has(rule.state);
	const showStateBadge = !hasFile || isNonActiveLifecycle;

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

	const handleFeedback = (feedback: OperatorFeedback) => {
		// `accept`/`reject` are optimistic single-click (same trust level as
		// promote/delete). Toggling the active verdict clears it back to
		// neutral so the operator can revise.
		if (!onSubmitFeedback) return;
		void onSubmitFeedback(rule.name, feedback);
	};

	return (
		<div className={`learning-rule-card${isCandidate ? " learning-rule-card--candidate" : ""}${isAntiPattern ? " learning-rule-card--anti" : ""}`}>
			<div
				className="learning-rule-header"
				onClick={toggleExpand}
				style={canExpand ? undefined : { cursor: "default" }}
			>
				{canExpand ? (
					<span className="learning-rule-expand">{expanded ? "▾" : "▸"}</span>
				) : (
					<span className="learning-rule-expand">&nbsp;</span>
				)}
				<span className="learning-rule-name">
					{rule.is_anti_pattern && <span className="learning-rule-anti" title="Anti-pattern: avoid this">!</span>}
					{rule.name}
				</span>
        <span
          className={providerScopeClass(rule.provider_scope)}
          title={isSharedScope ? PROVIDER_ASYMMETRY_DISCLOSURE : undefined}
        >
          {providerScopeLabel(rule.provider_scope)}
          {isSharedScope && (
            <span
              className="learning-scope-disclosure"
              aria-label={PROVIDER_ASYMMETRY_DISCLOSURE}
            >
              ⓘ
            </span>
          )}
        </span>
				<span className="learning-rule-confidence" style={{ color }}>
					{rule.confidence.toFixed(2)}
				</span>
				{showStateBadge && (
					<span className={`learning-rule-state learning-rule-state--${rule.state}`}>
						{rule.state.replace(/_/g, " ")}
					</span>
				)}
				{confirmKind ? (
					<span className="learning-rule-confirm-row" onClick={(e) => e.stopPropagation()}>
						<span className="learning-rule-confirm-label">
							{confirmKind === "promote"
								? "Add to rules?"
								: "Mark bad? It will be suppressed."}
						</span>
						<button
							className="learning-rule-confirm-yes"
							disabled={promoting || submittingBad}
							onClick={async () => {
								if (confirmKind === "promote") {
									if (!onPromote) return;
									setPromoting(true);
									try {
										await onPromote(rule.name);
									} finally {
										setPromoting(false);
										setConfirmKind(null);
									}
								} else {
									if (!onSubmitFeedback) return;
									setSubmittingBad(true);
									try {
										await onSubmitFeedback(rule.name, "bad");
									} finally {
										setSubmittingBad(false);
										setConfirmKind(null);
									}
								}
							}}
						>
							Yes
						</button>
						<button
							className="learning-rule-confirm-no"
							onClick={() => setConfirmKind(null)}
						>
							No
						</button>
					</span>
				) : (
					<>
						{onSubmitFeedback && (
							<>
								<button
									className={`learning-rule-feedback learning-rule-feedback--accept${rule.feedback === "accept" ? " is-active" : ""}`}
									onClick={(e) => {
										e.stopPropagation();
										handleFeedback("accept");
									}}
									title={rule.feedback === "accept" ? "Clear accept" : "Accept this rule"}
									aria-label={`Accept rule ${rule.name}`}
								>
									&#x2713;
								</button>
								<button
									className={`learning-rule-feedback learning-rule-feedback--reject${rule.feedback === "reject" ? " is-active" : ""}`}
									onClick={(e) => {
										e.stopPropagation();
										handleFeedback("reject");
									}}
									title={rule.feedback === "reject" ? "Clear reject" : "Reject (down-weight)"}
									aria-label={`Reject rule ${rule.name}`}
								>
									&#x2212;
								</button>
								<button
									className={`learning-rule-feedback learning-rule-feedback--bad${rule.feedback === "bad" ? " is-active" : ""}`}
									onClick={(e) => {
										e.stopPropagation();
										setConfirmKind("bad");
									}}
									title="Mark bad (strongest negative, suppresses the rule)"
									aria-label={`Mark rule ${rule.name} bad`}
								>
									&#x2718;
								</button>
							</>
						)}
						{!hasFile && onPromote && (
							<button
								className="learning-rule-promote"
								onClick={(e) => {
									e.stopPropagation();
									setConfirmKind("promote");
								}}
								disabled={!rule.content}
								title={rule.content ? "Promote to active rule" : "No content stored — re-run analysis"}
								aria-label={`Promote rule ${rule.name}`}
							>
								&#x2713;
							</button>
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
					</>
				)}
			</div>
			<div className="learning-rule-bar-track">
				<div
					className="learning-rule-bar-fill"
					style={{ width: `${pct}%`, backgroundColor: color }}
				/>
			</div>
			{(rule.domain || rule.project || rule.feedback || (rule.source && rule.source !== "observations")) && (
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
					{rule.feedback && (
						<span className={`learning-rule-meta-tag learning-rule-meta-tag--feedback learning-rule-meta-tag--feedback-${rule.feedback}`}>
							{FEEDBACK_LABEL[rule.feedback]}
						</span>
					)}
				</div>
			)}
			{expanded && (
				<pre className="learning-rule-content">
					{loading ? "Loading…" : content}
				</pre>
			)}
		</div>
	);
}

export default RuleCard;
