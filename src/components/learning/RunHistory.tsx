import { useState, useRef, useEffect, type ReactNode } from "react";
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

// Render an error message as a fragment, auto-linking any embedded https URLs
// so installation guidance from cc_client error variants (e.g.
// "Install from https://claude.com/claude-code/install") is one click away.
function renderErrorMessage(message: string): ReactNode {
  // split() with a capture-group regex returns alternating non-match / match
  // parts, so URLs land on odd indices.
  const parts = message.split(/(https?:\/\/[^\s)]+)/g);
  return parts.map((part, idx) =>
    idx % 2 === 1 ? (
      <a key={idx} href={part} target="_blank" rel="noreferrer noopener">
        {part}
      </a>
    ) : (
      <span key={idx}>{part}</span>
    ),
  );
}

function statusIcon(status: string): { icon: string; className: string } {
  switch (status) {
    case "running":
      return { icon: "", className: "learning-run-icon--live" };
    case "completed":
      return { icon: "\u2713", className: "learning-run-icon--ok" };
    case "interrupted":
      return { icon: "\u2014", className: "learning-run-icon--interrupted" };
    case "degraded":
      // Run finished but with one or more failed inference calls (feature 005
      // R-7 / L-3). Distinct amber warning glyph \u2014 must NOT fall through to
      // the hard-fail \u2717 which previously masked partial-success runs.
      return { icon: "\u26a0", className: "learning-run-icon--degraded" };
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
		case "degraded":
			return { color: "#F59E0B", label: "\u26a0" };
		case "failed":
			return { color: "#EF4444", label: "\u2717" };
		default:
			return { color: "#9CA3AF", label: "\u25CB" };
	}
}

// TODO(feature-006 T012): a `RunHistory` render assertion is required by the
// plan (a not-FS-confined run shows the marker + the exact
// `NO_FS_CONFINEMENT_HINT`; an FS-confined run and a legacy/no-inference run
// do NOT). It is intentionally NOT added here: the repo has NO frontend test
// infrastructure at all (no vitest/jest, no @testing-library, no jsdom/
// happy-dom, no `test` script in package.json, zero `*.test.tsx`). Scaffolding
// a framework is out of scope for this track and a per-track decision; T012
// is BLOCKED on a test-infra decision (which runner + jsdom + RTL deps). Once
// infra exists, assert against `isNotFsConfined` + `NO_FS_CONFINEMENT_HINT`
// with three fixtures: all_fs_confined:false ⇒ marker+hint shown;
// all_fs_confined:true ⇒ hidden; inference undefined ⇒ hidden.

// Honest confinement disclosure (feature 006 Follow-up A, R-A / C-A5). A run
// is "not filesystem-confined" when its inference rollup explicitly recorded
// that at least one call ran without OS filesystem confinement
// (`all_fs_confined === false`, e.g. the Linux `process-only` fallback when
// `bwrap` is absent). `undefined` (legacy/micro runs that recorded no
// `sandbox` tag, or no inference at all) is NOT a disclosure — render
// unchanged, exactly as before feature 006. FS-confined runs (`true`) are
// also unchanged.
function isNotFsConfined(run: LearningRun): boolean {
  return run.inference?.all_fs_confined === false;
}

// The actual not-filesystem-confined sandbox mechanism(s) this run used,
// derived from the per-call confinement: `process-only` (Linux without
// bwrap), `none` (no OS confinement available), or `job-object` (Windows).
// Naming the real mechanism instead of assuming the Linux fallback keeps
// the disclosure honest on every platform.
function notFsConfinedLabel(run: LearningRun): string {
  const tags = new Set<string>();
  for (const call of run.inference?.calls ?? []) {
    if (call.confinement && !call.confinement.fs_confined) {
      tags.add(call.confinement.sandbox);
    }
  }
  return tags.size > 0
    ? `${[...tags].join(", ")} (no FS isolation)`
    : "no FS isolation";
}

// Exact remediation copy required by the feature 006 contract (C-A5). Kept as
// a single source of truth so the inline marker and the detail hint agree.
const NO_FS_CONFINEMENT_HINT =
  "No filesystem confinement on this host — install bwrap for full isolation";

// Consecutive-failure detection (feature 005 R-7 / L-3). Purely derived from
// the already-fetched runs list — NO new fetch, NO backend circuit-breaker.
// "Terminal" excludes still-running runs (they have no verdict yet);
// `interrupted` is a user/process abort, not an inference failure, so it
// neither contributes to nor resets the streak. The banner fires only when
// the most recent K terminal-with-verdict runs are ALL hard `failed`.
const FAILURE_STREAK_K = 3;

function consecutiveFailureCount(runs: LearningRun[]): number {
  let streak = 0;
  for (const run of runs) {
    if (run.status === "running" || run.status === "interrupted") continue;
    if (run.status === "failed") {
      streak += 1;
    } else {
      // completed/degraded (or any other verdict) ends the streak.
      break;
    }
  }
  return streak;
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

  // Presentational consecutive-failure signal (L-3): no circuit-breaker, just
  // a hint to check the local `claude` CLI / sign-in when the last K terminal
  // runs all hard-failed. Derived from the runs already in props.
  const failureStreak = consecutiveFailureCount(runs);

  return (
    <div className="learning-section">
      <div className="learning-section-header">RECENT RUNS</div>

      {failureStreak >= FAILURE_STREAK_K && (
        <div className="learning-run-streak-banner" role="status">
          <span className="learning-run-streak-icon">⚠</span>
          <span>
            {failureStreak} consecutive failed runs — check the{" "}
            <code>claude</code> CLI / sign-in
          </span>
        </div>
      )}

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
                        : run.status === "degraded"
                          ? `degraded \u00b7 +${run.rules_created} rule${run.rules_created !== 1 ? "s" : ""}`
                          : "failed"}
                </span>
                {isNotFsConfined(run) && (
                  <span
                    className="learning-run-detail-degraded"
                    title={NO_FS_CONFINEMENT_HINT}
                  >
                    {"\u26a0 no FS confinement"}
                  </span>
                )}
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
              {/* Derived inference rollup (feature 005 R-7 / H-6 / FR-024).
                  Legacy/micro runs carry no `inference` — render em-dashes,
                  never crash. Inference time is the summed model-call
                  duration; the wall-clock Duration row above is kept. */}
              <div className="learning-run-detail-row">
                <span className="learning-run-detail-label">Model</span>
                <span>{selected.inference?.primary_model ?? "—"}</span>
              </div>
              <div className="learning-run-detail-row">
                <span className="learning-run-detail-label">Cost</span>
                <span>
                  {selected.inference
                    ? `$${selected.inference.total_cost_usd.toFixed(4)}`
                    : "—"}
                </span>
              </div>
              <div className="learning-run-detail-row">
                <span className="learning-run-detail-label">
                  Inference time
                </span>
                <span>
                  {selected.inference
                    ? formatDuration(selected.inference.total_duration_ms)
                    : "—"}
                </span>
              </div>
              {selected.inference &&
                selected.inference.failed_call_count > 0 && (
                  <div className="learning-run-detail-row">
                    <span className="learning-run-detail-label">
                      Failed calls
                    </span>
                    <span className="learning-run-detail-degraded">
                      {selected.inference.failed_call_count} /{" "}
                      {selected.inference.call_count}
                    </span>
                  </div>
                )}
              {/* Honest confinement disclosure (feature 006 Follow-up A,
                  R-A / C-A5). Surfaced only when the run explicitly recorded
                  a not-FS-confined call; FS-confined and legacy/micro runs
                  render exactly as before. Reuses the existing amber warn
                  affordance (no new design system). */}
              {isNotFsConfined(selected) && (
                <>
                  <div className="learning-run-detail-row">
                    <span className="learning-run-detail-label">
                      Confinement
                    </span>
                    <span className="learning-run-detail-degraded">
                      {notFsConfinedLabel(selected)}
                    </span>
                  </div>
                  <div
                    className="learning-run-streak-banner"
                    role="status"
                  >
                    <span className="learning-run-streak-icon">⚠</span>
                    <span>{NO_FS_CONFINEMENT_HINT}</span>
                  </div>
                </>
              )}
              <div className="learning-run-detail-row">
                <span className="learning-run-detail-label">Time</span>
                <span>
                  {new Date(selected.created_at).toLocaleString()}
                </span>
              </div>
            </>
          )}
          {selected.error && (
            <div className="learning-run-detail-error">
              {renderErrorMessage(selected.error)}
            </div>
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
