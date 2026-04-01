import { useState, useRef, useEffect } from "react";
import type { LearningRun, RunPhase } from "../../types";
import { providerScopeClass, providerScopeLabel } from "../../utils/providers";
import { timeAgo } from "../../utils/time";

interface RunHistoryProps {
  runs: LearningRun[];
  liveLogs: Record<number, string[]>;
}

function formatDuration(ms: number | null): string {
  if (ms === null) return "\u2014";
  if (ms < 1000) return `${ms}ms`;
  return `${(ms / 1000).toFixed(1)}s`;
}

function statusIcon(status: string): { icon: string; className: string } {
  switch (status) {
    case "running":
      return { icon: "", className: "learning-run-icon--live" };
    case "completed":
      return { icon: "\u2713", className: "learning-run-icon--ok" };
    case "interrupted":
      return { icon: "\u2014", className: "learning-run-icon--interrupted" };
    default:
      return { icon: "\u2717", className: "learning-run-icon--fail" };
  }
}

function phaseStatusDot(status: string): { color: string; label: string } {
	switch (status) {
		case "completed":
			return { color: "#22C55E", label: "\u2713" };
		case "running":
			return { color: "#3B82F6", label: "\u25CF" };
		case "skipped":
			return { color: "#6B7280", label: "\u2014" };
		case "failed":
			return { color: "#EF4444", label: "\u2717" };
		default:
			return { color: "#9CA3AF", label: "\u25CB" };
	}
}

function RunHistory({ runs, liveLogs }: RunHistoryProps) {
  const [selectedId, setSelectedId] = useState<number | null>(null);
  const logRef = useRef<HTMLPreElement>(null);
  const prevRunsRef = useRef<LearningRun[]>([]);

  // Auto-select when a new running run appears
  useEffect(() => {
    const prevIds = new Set(prevRunsRef.current.map((r) => r.id));
    const newRunning = runs.find(
      (r) => r.status === "running" && !prevIds.has(r.id),
    );
    if (newRunning) {
      setSelectedId(newRunning.id);
    }
    prevRunsRef.current = runs;
  }, [runs]);

  // Auto-scroll logs to bottom when new entries arrive
  const selectedLogs = selectedId !== null ? liveLogs[selectedId] : undefined;
  useEffect(() => {
    if (logRef.current) {
      logRef.current.scrollTop = logRef.current.scrollHeight;
    }
  }, [selectedLogs?.length, selectedId]);

  const selected = runs.find((r) => r.id === selectedId) ?? null;

  return (
    <div className="learning-section">
      <div className="learning-section-header">RECENT RUNS</div>

      {runs.length === 0 ? (
        <div className="learning-empty">No analysis runs yet</div>
      ) : (
        <div className="learning-runs-list">
          {runs.map((run) => {
            const { icon, className } = statusIcon(run.status);
            return (
              <div
                key={run.id}
                className={`learning-run-row${run.id === selectedId ? " learning-run-row--selected" : ""}`}
                onClick={() =>
                  setSelectedId(run.id === selectedId ? null : run.id)
                }
              >
                <span className={`learning-run-icon ${className}`}>
                  {run.status === "running" ? (
                    <span className="learning-run-live-dot" />
                  ) : (
                    icon
                  )}
                </span>
                <span className="learning-run-trigger">
                  {run.trigger_mode}
                </span>
                <span className={providerScopeClass(run.provider_scope)}>
                  {providerScopeLabel(run.provider_scope)}
                </span>
                <span className="learning-run-result">
                  {run.status === "running"
                    ? "running\u2026"
                    : run.status === "completed"
                      ? `+${run.rules_created} rule${run.rules_created !== 1 ? "s" : ""}`
                      : run.status === "interrupted"
                        ? "interrupted"
                        : "failed"}
                </span>
                <span className="learning-run-time">
                  {run.status === "running" ? "now" : timeAgo(run.created_at)}
                </span>
              </div>
            );
          })}
        </div>
      )}

      {selected && (
        <div className="learning-run-detail">
          <div className="learning-run-detail-row">
            <span className="learning-run-detail-label">Status</span>
            <span
              className={statusIcon(selected.status).className}
            >
              {selected.status}
            </span>
          </div>
          <div className="learning-run-detail-row">
            <span className="learning-run-detail-label">Trigger</span>
            <span>{selected.trigger_mode}</span>
          </div>
          <div className="learning-run-detail-row">
            <span className="learning-run-detail-label">Provider</span>
            <span className={providerScopeClass(selected.provider_scope)}>
              {providerScopeLabel(selected.provider_scope)}
            </span>
          </div>
          {selected.status !== "running" && (
            <>
              <div className="learning-run-detail-row">
                <span className="learning-run-detail-label">Observations</span>
                <span>{selected.observations_analyzed}</span>
              </div>
              <div className="learning-run-detail-row">
                <span className="learning-run-detail-label">Rules created</span>
                <span>{selected.rules_created}</span>
              </div>
              {selected.rules_updated > 0 && (
                <div className="learning-run-detail-row">
                  <span className="learning-run-detail-label">
                    Rules updated
                  </span>
                  <span>{selected.rules_updated}</span>
                </div>
              )}
              <div className="learning-run-detail-row">
                <span className="learning-run-detail-label">Duration</span>
                <span>{formatDuration(selected.duration_ms)}</span>
              </div>
              <div className="learning-run-detail-row">
                <span className="learning-run-detail-label">Time</span>
                <span>
                  {new Date(selected.created_at).toLocaleString()}
                </span>
              </div>
            </>
          )}
          {selected.error && (
            <div className="learning-run-detail-error">{selected.error}</div>
          )}
          {selected.phases && (() => {
						const phases: RunPhase[] = typeof selected.phases === "string"
							? JSON.parse(selected.phases)
							: selected.phases;
						return (
							<div className="learning-run-phases">
								{phases.map((phase) => {
									const dot = phaseStatusDot(phase.status);
									return (
										<div key={phase.name} className="learning-run-phase">
											<span style={{ color: dot.color }}>{dot.label}</span>
											<span className="learning-phase-name">{phase.name}</span>
											<span className="learning-phase-duration">
												{formatDuration(phase.duration_ms)}
											</span>
											{phase.findings_count > 0 && (
												<span className="learning-phase-count">
													{phase.findings_count} found
												</span>
											)}
										</div>
									);
								})}
							</div>
						);
					})()}
          {selected.status === "running" && liveLogs[selected.id]?.length > 0 && (
            <pre className="learning-run-detail-logs" ref={logRef}>
              {liveLogs[selected.id].join("\n")}
            </pre>
          )}
          {selected.status !== "running" && selected.logs && (
            <pre className="learning-run-detail-logs">{selected.logs}</pre>
          )}
        </div>
      )}
    </div>
  );
}

export default RunHistory;
