import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useCallback, useEffect, useState } from "react";
import {
  RETENTION_WINDOW_PRESETS,
  useRetentionPolicy,
  type RetentionWindowPreset,
} from "../../hooks/useRetentionPolicy";
import type { UseRuntimeSettingsResult } from "../../hooks/useRuntimeSettings";
import { useRollupBackfill } from "../../hooks/useRollupBackfill";
import { useToast } from "../../hooks/useToast";
import type {
  RetentionAuditRecord,
  RetentionMaintenanceProgress,
  RetentionMaintenanceResult,
  RetentionPreview,
} from "../../types";
import { formatRetentionCutoff } from "../../utils/retention";
import SettingRow from "./SettingRow";
import Toggle from "./Toggle";
import { clampInt } from "./utils";

interface PerformanceTabProps {
  runtime: UseRuntimeSettingsResult;
}

type CompactDatabaseProgress = {
  phase: string;
  pct?: number;
};

type CompactDatabaseResult = {
  status: "completed" | "skipped";
  reason?: string;
  bytes_before: number;
  bytes_after: number;
};

/**
 * The retention control's stage (feature 014).
 *
 * Preview and run are two commands with a user decision between them, so the
 * surface is a state machine rather than a pair of booleans: "previewing" and
 * "running" are both busy but say different things, and "confirm" and
 * "declined" are both a returned preview but only one of them may lead to a
 * delete.
 */
type RetentionStage =
  | { kind: "idle" }
  | { kind: "previewing" }
  /** A `ready` preview awaiting consent. */
  | { kind: "confirm"; preview: RetentionPreview }
  /** A `skipped` preview: nothing to consent to, and why. */
  | { kind: "declined"; preview: RetentionPreview }
  | { kind: "running"; archiving: boolean }
  | { kind: "done"; result: RetentionMaintenanceResult };

const IDLE: RetentionStage = { kind: "idle" };

/**
 * The S2 sentence, in so many words, wherever the distinction between rows and
 * bytes can mislead. A `DELETE` returns pages to SQLite's free list; only a
 * `VACUUM` returns bytes to the filesystem, so a user watching disk usage after
 * a prune that could not compact would otherwise conclude nothing happened.
 */
const RECLAIM_SENTENCE =
  "Deleting rows alone frees no filesystem bytes; compaction is required to return the space to your disk.";

/**
 * The one skip whose remedy is an action rather than copy: the backend refuses
 * a confirmation that no longer binds the user's consent, and the fix is a new
 * preview. Matched exactly because the backend deliberately sends a machine
 * token here instead of a sentence.
 */
const STALE_PREVIEW_REASON = "stale_preview";

/**
 * The lease refusal. Both maintenance commands take the same process-wide
 * ingest quiesce, so this is the one skip whose remedy is "wait and retry"
 * rather than "change something" — it gets a retry button and a heading that
 * does not claim the database had nothing to prune.
 */
const BUSY_REASON = "another maintenance operation is running";

function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  const units = ["KB", "MB", "GB", "TB"];
  const index = Math.min(Math.floor(Math.log(bytes) / Math.log(1024)), units.length);
  const value = bytes / 1024 ** index;
  return `${value.toFixed(value >= 10 || index === 0 ? 0 : 1)} ${units[index - 1]}`;
}

function formatCount(rows: number): string {
  return rows.toLocaleString();
}

/**
 * Terminate a backend-authored reason so it can be concatenated with the copy
 * around it. Some reasons arrive as full sentences and some as fragments, and
 * neither side of that boundary is worth making the other guess about.
 */
function asSentence(reason: string): string {
  return /[.!?]$/.test(reason.trim()) ? reason.trim() : `${reason.trim()}.`;
}

/** Per-target row counts, singularised. */
function formatRowPair(
  toolActions: number,
  sessionEvents: number,
  modelUsageObservations = 0,
): string {
  const tools = `${formatCount(toolActions)} tool action${toolActions === 1 ? "" : "s"}`;
  const events = `${formatCount(sessionEvents)} session event${sessionEvents === 1 ? "" : "s"}`;
  if (modelUsageObservations === 0) return `${tools} and ${events}`;
  const observations = `${formatCount(modelUsageObservations)} model observation${modelUsageObservations === 1 ? "" : "s"}`;
  return `${tools}, ${events}, and ${observations}`;
}

const MS_PER_DAY = 86_400_000;

/**
 * Whole days between a conforming timestamp and now, or null if the stored
 * value cannot be parsed. Floored and clamped at zero: a record whose clock
 * sits slightly ahead of this one reads as "today" rather than as a negative
 * age nobody can act on.
 */
function daysSince(timestamp: string, now: number | null): number | null {
  const parsed = Date.parse(timestamp);
  if (now === null || !Number.isFinite(parsed)) {
    return null;
  }
  return Math.max(0, Math.floor((now - parsed) / MS_PER_DAY));
}

/**
 * The drift line, and the only mitigation the no-scheduler decision gets.
 *
 * A retention window is a plan, not a timer: a 90-day window bounds the
 * database on the days it is actually run and on no others. Stating the run's
 * age beside the configured window — "last pruned 112 days ago; window 90
 * days" — makes that drift legible without a scheduler existing anywhere.
 */
function retentionAgeLine(
  record: RetentionAuditRecord,
  windowDays: number | null,
  now: number | null,
): string {
  // A skipped run happened but removed nothing, so it must not claim a prune.
  const verb = record.status === "skipped" ? "Last attempted" : "Last pruned";
  const age = daysSince(record.ran_at, now);
  const when =
    age === null
      ? `on ${formatRetentionCutoff(record.ran_at)}`
      : age === 0
        ? "today"
        : `${formatCount(age)} day${age === 1 ? "" : "s"} ago`;
  const configured =
    windowDays === null ? "retention is now off" : `window ${windowDays} days`;
  const drifted =
    age !== null && windowDays !== null && age > windowDays
      ? " Rows older than the window have accumulated since: pruning runs only when you ask."
      : "";
  return `${verb} ${when}; ${configured}.${drifted}`;
}

/** "Completed" / "Stopped part-way" / "Nothing removed". */
function retentionStatusLabel(status: RetentionAuditRecord["status"]): string {
  if (status === "completed") return "Completed";
  return status === "partial" ? "Stopped part-way" : "Nothing removed";
}

function PerformanceTab({ runtime }: PerformanceTabProps) {
  const { toast } = useToast();
  const { settings, saving } = runtime;
  const retention = useRetentionPolicy();
  const modelRollup = useRollupBackfill("model");
  const [compactionProgress, setCompactionProgress] =
    useState<CompactDatabaseProgress | null>(null);
  const [compactionResult, setCompactionResult] =
    useState<CompactDatabaseResult | null>(null);
  const [retentionStage, setRetentionStage] = useState<RetentionStage>(IDLE);
  const [retentionProgress, setRetentionProgress] =
    useState<RetentionMaintenanceProgress | null>(null);
  // Day-resolution clock for the audit record's age line. `Date.now()` may not
  // be read during render, so it is captured here and re-captured whenever a
  // new record arrives — a run that just finished must not read as days old.
  const [auditClock, setAuditClock] = useState<number | null>(null);
  const lastRun = retention.policy.last_run;
  useEffect(() => {
    setAuditClock(Date.now());
  }, [lastRun]);

  const finishCompaction = useCallback((result: CompactDatabaseResult) => {
    setCompactionProgress(null);
    setCompactionResult(result);
  }, []);

  // Idempotent: the composite command both emits `-finished` and returns the
  // same result, so whichever arrives first settles the stage and the other is
  // a no-op rather than a second transition.
  const finishRetention = useCallback((result: RetentionMaintenanceResult) => {
    setRetentionProgress(null);
    setRetentionStage({ kind: "done", result });
  }, []);

  useEffect(() => {
    const unlistenProgress = listen<CompactDatabaseProgress>(
      "compact-database-progress",
      ({ payload }) => {
        setCompactionResult(null);
        setCompactionProgress(payload);
      },
    );
    const unlistenFinished = listen<CompactDatabaseResult>(
      "compact-database-finished",
      ({ payload }) => finishCompaction(payload),
    );

    return () => {
      void Promise.all([unlistenProgress, unlistenFinished]).then((unlisteners) => {
        unlisteners.forEach((unlisten) => unlisten());
      });
    };
  }, [finishCompaction]);

  // Preview and run share one progress event, so one listener pair covers both.
  useEffect(() => {
    const unlistenProgress = listen<RetentionMaintenanceProgress>(
      "retention-maintenance-progress",
      ({ payload }) => setRetentionProgress(payload),
    );
    const unlistenFinished = listen<RetentionMaintenanceResult>(
      "retention-maintenance-finished",
      ({ payload }) => finishRetention(payload),
    );

    return () => {
      void Promise.all([unlistenProgress, unlistenFinished]).then((unlisteners) => {
        unlisteners.forEach((unlisten) => unlisten());
      });
    };
  }, [finishRetention]);

  const update = (patch: Partial<typeof settings>) => {
    void runtime.save({ ...settings, ...patch }).catch((err) => toast("error", String(err)));
  };

  const compacting = compactionProgress != null;
  const pruning =
    retentionStage.kind === "previewing" || retentionStage.kind === "running";
  /**
   * Compaction and retention both take the same process-wide ingest quiesce
   * lease, and the loser of that race gets a structured busy skip rather than a
   * queue. Disabling both controls while either runs turns that backend refusal
   * into something the user never has to discover.
   */
  const maintenanceBusy = compacting || pruning;
  const modelRollupBusy =
    modelRollup.state.kind === "starting" || modelRollup.state.kind === "running";

  const compactDatabase = useCallback(async () => {
    setCompactionResult(null);
    setCompactionProgress({ phase: "Starting" });
    try {
      finishCompaction(await invoke<CompactDatabaseResult>("compact_database"));
    } catch (err) {
      setCompactionProgress(null);
      toast("error", String(err));
    }
  }, [finishCompaction, toast]);

  const previewRetention = useCallback(async () => {
    setRetentionStage({ kind: "previewing" });
    setRetentionProgress({ phase: "Counting rows", pct: 0 });
    try {
      const preview = await invoke<RetentionPreview>("preview_retention");
      setRetentionProgress(null);
      setRetentionStage(
        preview.status === "ready"
          ? { kind: "confirm", preview }
          : { kind: "declined", preview },
      );
    } catch (err) {
      setRetentionProgress(null);
      setRetentionStage(IDLE);
      toast("error", String(err));
    }
  }, [toast]);

  const confirmRetention = useCallback(async (archiveBeforePrune: boolean) => {
    setRetentionStage({ kind: "running", archiving: archiveBeforePrune });
    setRetentionProgress({ phase: "Counting rows", pct: 0 });
    try {
      // Re-preview to mint a fresh cutoff token. The backend refuses a
      // confirmation that trails a freshly derived cutoff by more than one
      // counting phase, so the token is taken at the moment of the run rather
      // than held open while the confirm panel sat on screen — and the numbers
      // the run acts on are the numbers that were just counted.
      const fresh = await invoke<RetentionPreview>("preview_retention");
      if (fresh.status !== "ready" || fresh.cutoff === null || fresh.window_days === null) {
        setRetentionProgress(null);
        setRetentionStage({ kind: "declined", preview: fresh });
        return;
      }
      finishRetention(
        await invoke<RetentionMaintenanceResult>("run_retention_maintenance", {
          confirmedCutoff: fresh.cutoff,
          confirmedWindowDays: fresh.window_days,
          archiveBeforePrune,
        }),
      );
    } catch (err) {
      setRetentionProgress(null);
      setRetentionStage(IDLE);
      toast("error", String(err));
    }
  }, [finishRetention, toast]);

  const changeRetentionWindow = (raw: string) => {
    // Consent is per-value: a stale confirm panel describes a window the user
    // is no longer asking for, so changing the preset discards it.
    setRetentionStage(IDLE);
    setRetentionProgress(null);
    const next = raw === "never" ? null : (Number(raw) as RetentionWindowPreset);
    void retention.setWindowDays(next).catch((err) => toast("error", String(err)));
  };

  const compactionDescription = compactionProgress
    ? `Compacting: ${compactionProgress.phase}${
        compactionProgress.pct == null ? "" : ` · ${compactionProgress.pct}%`
      }`
    : compactionResult?.status === "skipped"
      ? `Skipped: ${compactionResult.reason ?? "the database is not ready to compact."}`
      : compactionResult?.status === "completed"
        ? `Complete: ${formatBytes(compactionResult.bytes_before)} → ${formatBytes(
            compactionResult.bytes_after,
          )}`
        : "Reclaim unused SQLite pages. Quill pauses ingest while maintenance runs.";

  const retentionWindow = retention.policy.window_days;
  const modelRollupDescription = (() => {
    const state = modelRollup.state;
    if (state.kind === "starting") return "Starting the model index rebuild…";
    if (state.kind === "running") {
      const { progress } = state;
      const phase =
        progress.phase === "preflight"
          ? "Checking disk space"
          : progress.phase === "checkpointing"
            ? "Saving progress"
            : "Building index";
      return `${phase} · ${formatCount(progress.rowsDone)}/${formatCount(
        progress.rowsTotal,
      )} model observations. Models keeps using raw evidence until this completes.`;
    }
    if (state.kind === "refused") return `Not started: ${state.reason}`;
    if (state.kind === "error") {
      const saved =
        state.progress === null
          ? "No new progress was committed."
          : `${formatCount(state.progress.rowsDone)}/${formatCount(
              state.progress.rowsTotal,
            )} observations are committed.`;
      return `Stopped: ${state.detail} ${saved} Rebuild to resume.`;
    }
    if (state.kind === "completed") {
      return state.progress === null
        ? "Complete. Model analytics now use the rebuilt index. Raw-pruned history was preserved."
        : `Complete: ${formatCount(state.progress.rowsDone)}/${formatCount(
            state.progress.rowsTotal,
          )} model observations indexed. Raw-pruned history was preserved.`;
    }
    return "Rebuild closed-hour model analytics from retained observations. Raw-pruned history remains authoritative and is never replaced.";
  })();
  const retentionDescription = retention.loading
    ? "Reading the retention policy…"
    : retentionProgress
      ? `${retentionProgress.phase} · ${retentionProgress.pct}%`
      : retentionWindow === null
        ? "Off — Quill keeps every transcript row. Pruning deletes tool activity and session events older than the window you choose, and only ever when you ask."
        : `Deletes tool activity and session events older than ${retentionWindow} days. Never scheduled: it runs only when you preview and confirm.${
            retention.policy.watermark
              ? ` Already pruned before ${formatRetentionCutoff(retention.policy.watermark)}.`
              : ""
          }`;

  return (
    <div className="settings-panel">
      <div className="settings-section-header">Live usage polling</div>
      <SettingRow
        label="Background refresh"
        description="When ON, Quill refreshes Live Usage in the background even when the main window is hidden, so the tray indicator stays current."
        control={
          <Toggle
            tone={settings.liveUsageEnabled ? "on" : "off"}
            pressed={settings.liveUsageEnabled}
            disabled={saving}
            onClick={() => update({ liveUsageEnabled: !settings.liveUsageEnabled })}
          />
        }
      />
      <SettingRow
        label="Refresh interval (seconds)"
        description="Range 60–600. Lower values feel more responsive but consume more CPU and provider quota."
        control={
          <input
            type="number"
            className="settings-input settings-input--narrow"
            min={60}
            max={600}
            step={30}
            value={settings.liveUsageIntervalSeconds}
            onChange={(e) =>
              update({
                liveUsageIntervalSeconds: clampInt(
                  parseInt(e.target.value, 10),
                  60,
                  600,
                ),
              })
            }
            disabled={!settings.liveUsageEnabled || saving}
          />
        }
      />

      <div className="settings-section-header">Database maintenance</div>
      <SettingRow
        label="Compact database"
        description={compactionDescription}
        control={
          <button
            type="button"
            className="settings-button settings-button--compact"
            disabled={maintenanceBusy}
            onClick={() => void compactDatabase()}
          >
            {compacting ? "Compacting…" : "Compact"}
          </button>
        }
      />
      <SettingRow
        label="Rebuild model index"
        description={
          <span className="settings-rollup-status" role="status" aria-live="polite">
            {modelRollupDescription}
          </span>
        }
        control={
          <button
            type="button"
            className="settings-button settings-button--compact"
            disabled={maintenanceBusy || modelRollupBusy}
            onClick={() => void modelRollup.rebuild().catch(() => undefined)}
          >
            {modelRollupBusy ? "Building…" : "Rebuild"}
          </button>
        }
      />
      <SettingRow
        label="Prune old transcript history"
        description={retentionDescription}
        control={
          <>
            <select
              className="settings-select"
              aria-label="Retention window"
              value={retentionWindow === null ? "never" : String(retentionWindow)}
              disabled={maintenanceBusy || retention.loading || retention.saving}
              onChange={(e) => changeRetentionWindow(e.target.value)}
            >
              <option value="never">Never</option>
              {RETENTION_WINDOW_PRESETS.map((days) => (
                <option key={days} value={days}>
                  Older than {days} days
                </option>
              ))}
            </select>
            <button
              type="button"
              className="settings-button"
              disabled={maintenanceBusy || retention.loading || retentionWindow === null}
              onClick={() => void previewRetention()}
            >
              {retentionStage.kind === "previewing" ? "Counting…" : "Preview…"}
            </button>
          </>
        }
      />
      <RetentionPanel
        stage={retentionStage}
        busy={maintenanceBusy}
        onConfirm={(archiveBeforePrune) => void confirmRetention(archiveBeforePrune)}
        onPreview={() => void previewRetention()}
        onDismiss={() => setRetentionStage(IDLE)}
      />
      {/* The terminal panel narrates the run that just finished, so the durable
          record stays out of its way until it is dismissed rather than saying
          the same thing twice. */}
      {!retention.loading && retentionStage.kind !== "done" && (
        <RetentionAudit
          record={lastRun}
          windowDays={retentionWindow}
          now={auditClock}
        />
      )}
      {retention.error !== null && (
        <div className="settings-empty settings-empty--error">
          Retention policy unavailable: {retention.error}
        </div>
      )}
    </div>
  );
}

interface RetentionPanelProps {
  stage: RetentionStage;
  busy: boolean;
  onConfirm: (archiveBeforePrune: boolean) => void;
  onPreview: () => void;
  onDismiss: () => void;
}

/**
 * The consent step and the terminal states, below the row that starts them.
 *
 * Chrome-grey throughout. `DESIGN.md` reserves green / amber / red for the
 * severity meter, and a prune the user asked for is not a threshold breach —
 * so the weight of a destructive action is carried by the copy and by an
 * explicit second click, never by a red button.
 */
function RetentionPanel({
  stage,
  busy,
  onConfirm,
  onPreview,
  onDismiss,
}: RetentionPanelProps) {
  if (stage.kind === "idle") {
    return null;
  }

  if (stage.kind === "previewing" || stage.kind === "running") {
    return (
      <div className="retention-panel" role="status" aria-live="polite">
        <div className="retention-panel-heading">
          {stage.kind === "previewing" ? "Counting" : "Pruning"}
        </div>
        <p className="retention-panel-line">
          {stage.kind === "previewing"
            ? "Counting the rows a prune would remove. Ingest is paused while this runs."
            : stage.archiving
              ? "Archiving the previewed rows, then removing and compacting them. Ingest stays paused so the sidecar and delete set cannot drift."
              : "Removing rows and compacting. Ingest is paused: hook and widget reports get a retriable 503 and land once it finishes."}
        </p>
      </div>
    );
  }

  if (stage.kind === "declined") {
    const { preview } = stage;
    const contended = preview.reason === BUSY_REASON;
    return (
      <div className="retention-panel" role="note">
        <div className="retention-panel-heading">
          {contended ? "Maintenance already running" : "Nothing to prune"}
        </div>
        <p className="retention-panel-line">
          {contended
            ? "Another maintenance operation holds the database. Nothing was counted and nothing was deleted; try again once it finishes."
            : asSentence(preview.reason ?? "Retention found nothing it could remove")}
        </p>
        <div className="retention-panel-actions">
          {contended && (
            <button
              type="button"
              className="settings-button"
              disabled={busy}
              onClick={onPreview}
            >
              Count again
            </button>
          )}
          <button type="button" className="settings-button" onClick={onDismiss}>
            Dismiss
          </button>
        </div>
      </div>
    );
  }

  if (stage.kind === "confirm") {
    const { preview } = stage;
    const nonconforming =
      preview.tool_actions_nonconforming + preview.session_events_nonconforming;
    return (
      <div className="retention-panel" role="note">
        <div className="retention-panel-heading">
          Confirm pruning
          {preview.cutoff !== null && (
            <span className="retention-panel-cutoff">
              before {formatRetentionCutoff(preview.cutoff)}
            </span>
          )}
        </div>
        <p className="retention-panel-line">
          {formatRowPair(
            preview.tool_actions_rows,
            preview.session_events_rows,
            preview.model_usage_observations_rows,
          )} will
          be deleted permanently. Archive & prune writes a local JSONL copy before
          deletion begins.
        </p>
        {preview.everything_older && (
          <p className="retention-panel-line">
            This covers <strong>every</strong> transcript row Quill owns, not just the
            oldest part of it.
          </p>
        )}
        {nonconforming > 0 && (
          <p className="retention-panel-note">
            {formatRowPair(
              preview.tool_actions_nonconforming,
              preview.session_events_nonconforming,
            )}{" "}
            carry timestamps Quill cannot compare, and are kept. Archive &amp; prune
            includes them in the sidecar too.
          </p>
        )}
        {preview.affected_surfaces.length > 0 && (
          <>
            <p className="retention-panel-line">What stops working, pre-cutoff only:</p>
            <ul className="retention-panel-list">
              {preview.affected_surfaces.map((surface) => (
                <li key={surface}>{surface}</li>
              ))}
            </ul>
          </>
        )}
        <p className="retention-panel-note">
          {RECLAIM_SENTENCE} This run compacts immediately after the deletes, so the
          file on disk shrinks in the same pass — unless there is not enough free
          space, which is reported rather than hidden.
        </p>
        <div className="retention-panel-actions">
          <button
            type="button"
            className="settings-button settings-button--confirm"
            disabled={busy}
            onClick={() => onConfirm(true)}
          >
            Archive &amp; prune
          </button>
          <button
            type="button"
            className="settings-button"
            disabled={busy}
            onClick={() => onConfirm(false)}
          >
            Prune without archive
          </button>
          <button
            type="button"
            className="settings-button"
            disabled={busy}
            onClick={onDismiss}
          >
            Cancel
          </button>
        </div>
      </div>
    );
  }

  const { result } = stage;

  if (result.status === "skipped") {
    const stale = result.reason === STALE_PREVIEW_REASON;
    const contended = result.reason === BUSY_REASON;
    return (
      <div className="retention-panel" role="note">
        <div className="retention-panel-heading">
          {stale
            ? "Preview went stale"
            : contended
              ? "Maintenance already running"
              : "Nothing was removed"}
        </div>
        <p className="retention-panel-line">
          {stale
            ? "The confirmation no longer matched a freshly counted cutoff, so nothing was deleted. Count again and confirm."
            : contended
              ? "Another maintenance operation holds the database. Nothing was deleted; try again once it finishes."
              : asSentence(result.reason ?? "Retention found nothing it could remove")}
        </p>
        {result.archive_path !== null && (
          <p className="retention-panel-note">
            Archived{" "}
            {formatRowPair(
              result.tool_actions_archived,
              result.session_events_archived,
              result.model_usage_observations_archived,
            )}{" "}
            before the prune to{" "}
            <code className="retention-panel-path">{result.archive_path}</code>.
          </p>
        )}
        <div className="retention-panel-actions">
          {(stale || contended) && (
            <button
              type="button"
              className="settings-button"
              disabled={busy}
              onClick={onPreview}
            >
              Count again
            </button>
          )}
          <button type="button" className="settings-button" onClick={onDismiss}>
            Dismiss
          </button>
        </div>
      </div>
    );
  }

  const partial = result.status === "partial";
  const removed = formatRowPair(
    result.tool_actions_deleted,
    result.session_events_deleted,
    result.model_usage_observations_deleted,
  );
  return (
    <div className="retention-panel" role="note">
      <div className="retention-panel-heading">
        {partial ? "Partly pruned" : "Pruned"}
        {result.cutoff !== null && (
          <span className="retention-panel-cutoff">
            before {formatRetentionCutoff(result.cutoff)}
          </span>
        )}
      </div>
      <p className="retention-panel-line">
        {partial
          ? `Removed ${removed}, then stopped. What was removed is gone permanently; run it again to continue.`
          : `Removed ${removed}.`}
      </p>
      {result.archive_path !== null && (
        <p className="retention-panel-note">
          Archived{" "}
          {formatRowPair(
            result.tool_actions_archived,
            result.session_events_archived,
            result.model_usage_observations_archived,
          )}{" "}
          before deletion to{" "}
          <code className="retention-panel-path">{result.archive_path}</code>.
        </p>
      )}
      {partial && (
        <p className="retention-panel-note">
          Stopped because {asSentence(result.error_reason ?? "the run could not continue")}
        </p>
      )}
      {result.compaction_status === "completed" ? (
        <p className="retention-panel-line">
          Compacted: {formatBytes(result.bytes_before)} →{" "}
          {formatBytes(result.bytes_after)} on disk.
        </p>
      ) : (
        <p className="retention-panel-note">
          Compaction was skipped
          {result.compaction_reason === null
            ? "."
            : `: ${asSentence(result.compaction_reason)}`}{" "}
          {RECLAIM_SENTENCE} The file is still {formatBytes(result.bytes_after)}; run
          Compact when there is room.
        </p>
      )}
      <div className="retention-panel-actions">
        <button type="button" className="settings-button" onClick={onDismiss}>
          Dismiss
        </button>
      </div>
    </div>
  );
}

interface RetentionAuditProps {
  /** `retention.last_run`; null on a database that has never run retention. */
  record: RetentionAuditRecord | null;
  /** The window configured *now*, which a past run need not have used. */
  windowDays: number | null;
  /**
   * The clock the age line compares against, captured in an effect because
   * reading it during render is impure. Null until that effect runs, which the
   * line degrades to an absolute run date for.
   */
  now: number | null;
}

/**
 * The durable answer to "what did I delete, and when" (feature 014).
 *
 * The toast and the terminal panel are both transient; this reads
 * `retention.last_run` back and keeps every field of it on screen — cutoff, run
 * date, status with its skip or error reason, rows removed per table, rows kept
 * because their timestamps could not be compared, and the file size on either
 * side of the run.
 *
 * Chrome-grey and hairline-ruled like {@link RetentionPanel}: a record of a
 * deletion the user asked for is not a threshold breach, so it does not spend
 * the reserved severity meter — not even on the `"partial"` status.
 */
function RetentionAudit({ record, windowDays, now }: RetentionAuditProps) {
  if (record === null) {
    // Nothing to account for yet. Worth saying only once a window is set: with
    // retention off, "never pruned" is the setting, not news.
    if (windowDays === null) {
      return null;
    }
    return (
      <div className="retention-audit" role="note">
        <div className="retention-audit-heading">Last prune</div>
        <p className="retention-audit-line">
          Never — the {windowDays}-day window applies only when you preview and
          confirm a run.
        </p>
      </div>
    );
  }

  const nonconforming =
    record.skipped_nonconforming.tool_actions +
    record.skipped_nonconforming.session_events;
  const reclaimed = record.bytes_before - record.bytes_after;

  return (
    <div className="retention-audit" role="note">
      <div className="retention-audit-heading">
        Last prune
        <span className="retention-audit-status">
          {retentionStatusLabel(record.status)}
        </span>
        {record.cutoff !== null && (
          <span className="retention-audit-cutoff">
            cutoff {formatRetentionCutoff(record.cutoff)}
            {record.window_days === null ? "" : ` · ${record.window_days}-day window`}
          </span>
        )}
      </div>
      <p className="retention-audit-line">
        {retentionAgeLine(record, windowDays, now)}
      </p>
      <dl className="retention-audit-figures">
        <div className="retention-audit-figure">
          <dt>Tool actions removed</dt>
          <dd>{formatCount(record.deleted.tool_actions)}</dd>
        </div>
        <div className="retention-audit-figure">
          <dt>Session events removed</dt>
          <dd>{formatCount(record.deleted.session_events)}</dd>
        </div>
        <div className="retention-audit-figure">
          <dt>Model observations removed</dt>
          <dd>{formatCount(record.deleted.model_usage_observations)}</dd>
        </div>
        <div className="retention-audit-figure">
          <dt>On disk</dt>
          <dd>
            {formatBytes(record.bytes_before)} → {formatBytes(record.bytes_after)}
          </dd>
        </div>
      </dl>
      {record.status === "partial" && (
        <p className="retention-audit-note">
          Stopped because{" "}
          {asSentence(record.error_reason ?? "the run could not continue")} What it
          had already removed is gone permanently.
        </p>
      )}
      {record.status === "skipped" && (
        <p className="retention-audit-note">
          {asSentence(record.reason ?? "Retention found nothing it could remove")}
        </p>
      )}
      {nonconforming > 0 && (
        <p className="retention-audit-note">
          Kept:{" "}
          {formatRowPair(
            record.skipped_nonconforming.tool_actions,
            record.skipped_nonconforming.session_events,
          )}{" "}
          carrying timestamps Quill cannot compare.
        </p>
      )}
      {reclaimed <= 0 &&
        record.deleted.tool_actions +
          record.deleted.session_events +
          record.deleted.model_usage_observations >
          0 && (
        <p className="retention-audit-note">{RECLAIM_SENTENCE}</p>
      )}
    </div>
  );
}

export default PerformanceTab;
