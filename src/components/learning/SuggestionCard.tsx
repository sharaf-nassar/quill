import { useState } from "react";
import type { OptimizationSuggestion } from "../../hooks/useMemoryData";

interface SuggestionCardProps {
  suggestion: OptimizationSuggestion;
  onApprove: (id: number) => void;
  onDeny: (id: number) => void;
  onUndeny?: (id: number) => void;
  onUndo?: (id: number) => void;
  inGroup?: boolean;
}

const ACTION_COLORS: Record<string, string> = {
  delete: "#EF4444",
  update: "#3B82F6",
  merge: "#8B5CF6",
  create: "#22C55E",
  flag: "#EAB308",
};

function DiffView({ diff }: { diff: string }) {
  return (
    <pre className="learning-rule-content" style={{ fontSize: 10 }}>
      {diff.split("\n").map((line, i) => {
        let bg = "transparent";
        let color = "rgba(255,255,255,0.7)";
        if (line.startsWith("+") && !line.startsWith("+++")) {
          bg = "rgba(34,197,94,0.12)";
          color = "#22C55E";
        } else if (line.startsWith("-") && !line.startsWith("---")) {
          bg = "rgba(239,68,68,0.12)";
          color = "#EF4444";
        } else if (line.startsWith("@@")) {
          color = "#3B82F6";
        }
        return (
          <div key={i} style={{ background: bg, color, padding: "0 4px" }}>
            {line}
          </div>
        );
      })}
    </pre>
  );
}

export function SuggestionCard({
  suggestion,
  onApprove,
  onDeny,
  onUndeny,
  onUndo,
  inGroup,
}: SuggestionCardProps) {
  const [expanded, setExpanded] = useState(false);
  const [showFull, setShowFull] = useState(false);
  const color = ACTION_COLORS[suggestion.action_type] || "#888";
  const isPending =
    suggestion.status === "pending" || suggestion.status === "undone";
  const isDenied = suggestion.status === "denied";
  const isApproved = suggestion.status === "approved";
  const isUndone = suggestion.status === "undone";
  const isFlag = suggestion.action_type === "flag";
  const hasDiff = !!suggestion.diff_summary;
  const hasOriginal = !!suggestion.original_content;
  const canUndo =
    isApproved && (hasOriginal || suggestion.action_type === "create");

  return (
    <div
      className="learning-rule-card"
      style={{
        borderColor: isPending ? `${color}40` : "rgba(255,255,255,0.1)",
        opacity: isDenied ? 0.6 : 1,
      }}
    >
      <div
        className="learning-rule-header"
        onClick={() => setExpanded(!expanded)}
      >
        <span className="learning-rule-expand">
          {expanded ? "\u25BE" : "\u25B8"}
        </span>
        <span
          style={{
            fontSize: 9,
            fontWeight: 700,
            padding: "1px 5px",
            borderRadius: 3,
            background: `${color}20`,
            color,
            textTransform: "uppercase",
            letterSpacing: 0.5,
          }}
        >
          {suggestion.action_type}
        </span>
        <span className="learning-rule-name">
          {suggestion.target_file || "(new file)"}
        </span>
        {suggestion.status !== "pending" && (
          <span
            className={`learning-rule-state learning-rule-state--${
              isApproved || isUndone ? "confirmed" : "invalidated"
            }`}
          >
            {suggestion.status}
          </span>
        )}
        {isPending && !inGroup && (
          <span style={{ display: "flex", gap: 4, marginLeft: "auto" }}>
            <button
              className="learning-analyze-btn"
              style={{
                borderColor: color,
                color,
                fontSize: 9,
                padding: "2px 8px",
              }}
              onClick={(e) => {
                e.stopPropagation();
                onApprove(suggestion.id);
              }}
            >
              {isFlag ? "Dismiss" : isUndone ? "Re-approve" : "Approve"}
            </button>
            {!isFlag && (
              <button
                className="learning-rule-delete"
                style={{ fontSize: 11, color: "#EF4444" }}
                onClick={(e) => {
                  e.stopPropagation();
                  onDeny(suggestion.id);
                }}
              >
                Deny
              </button>
            )}
          </span>
        )}
        {canUndo && onUndo && (
          <button
            className="learning-analyze-btn"
            style={{
              borderColor: "#EAB308",
              color: "#EAB308",
              fontSize: 9,
              padding: "2px 8px",
              marginLeft: "auto",
            }}
            onClick={(e) => {
              e.stopPropagation();
              onUndo(suggestion.id);
            }}
          >
            Undo
          </button>
        )}
        {isDenied && onUndeny && (
          <button
            className="learning-rule-delete"
            style={{ fontSize: 10, color: "#888" }}
            onClick={(e) => {
              e.stopPropagation();
              onUndeny(suggestion.id);
            }}
          >
            Undeny
          </button>
        )}
      </div>
      <div className="learning-rule-bar-track">
        <div
          className="learning-rule-bar-fill"
          style={{ width: "100%", background: color }}
        />
      </div>
      <span className="learning-rule-domain">{suggestion.reasoning}</span>
      {suggestion.error && (
        <div className="learning-run-detail-error" style={{ marginTop: 4 }}>
          {suggestion.error}
        </div>
      )}
      {expanded && hasDiff && !showFull && (
        <>
          <div style={{ display: "flex", gap: 4, marginTop: 4 }}>
            <button
              className="learning-analyze-btn"
              style={{
                fontSize: 8,
                padding: "1px 6px",
                borderColor: `${color}40`,
                color,
              }}
              onClick={() => setShowFull(true)}
            >
              Full
            </button>
            <span
              style={{
                fontSize: 8,
                color: "rgba(255,255,255,0.3)",
                alignSelf: "center",
              }}
            >
              Diff view
            </span>
          </div>
          <DiffView diff={suggestion.diff_summary!} />
        </>
      )}
      {expanded && (showFull || !hasDiff) && suggestion.proposed_content && (
        <>
          {hasDiff && (
            <div style={{ display: "flex", gap: 4, marginTop: 4 }}>
              <button
                className="learning-analyze-btn"
                style={{
                  fontSize: 8,
                  padding: "1px 6px",
                  borderColor: `${color}40`,
                  color,
                }}
                onClick={() => setShowFull(false)}
              >
                Diff
              </button>
              <span
                style={{
                  fontSize: 8,
                  color: "rgba(255,255,255,0.3)",
                  alignSelf: "center",
                }}
              >
                Full view
              </span>
            </div>
          )}
          <pre className="learning-rule-content">
            {suggestion.proposed_content}
          </pre>
        </>
      )}
      {expanded &&
        suggestion.merge_sources &&
        suggestion.merge_sources.length > 0 && (
          <div
            style={{
              marginTop: 4,
              fontSize: 10,
              color: "rgba(255,255,255,0.45)",
            }}
          >
            Merging: {suggestion.merge_sources.join(", ")}
          </div>
        )}
    </div>
  );
}
