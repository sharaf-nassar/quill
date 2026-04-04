import { useState, useMemo, useRef, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useBreakdownData } from "../../hooks/useBreakdownData";
import { useToast } from "../../hooks/useToast";
import { formatTokenCount } from "../../utils/tokens";
import { providerLabel } from "../../utils/providers";
import type {
  BreakdownMode,
  BreakdownSelection,
  HostBreakdown,
  ProjectBreakdown,
  SessionBreakdown,
  SessionRef,
} from "../../types";
import { sessionRefKey } from "../../types";

function formatRelativeTime(isoString: string): string {
  const now = Date.now();
  const then = new Date(isoString).getTime();
  const diffMs = now - then;
  const diffMin = Math.floor(diffMs / 60000);
  if (diffMin < 1) return "just now";
  if (diffMin < 60) return `${diffMin}m ago`;
  const diffHr = Math.floor(diffMin / 60);
  if (diffHr < 24) return `${diffHr}h ago`;
  const diffDays = Math.floor(diffHr / 24);
  return `${diffDays}d ago`;
}

function formatDuration(firstSeen: string, lastActive: string): string {
  const diffMs = new Date(lastActive).getTime() - new Date(firstSeen).getTime();
  const diffMin = Math.floor(diffMs / 60000);
  if (diffMin < 1) return "< 1m";
  if (diffMin < 60) return `${diffMin}m`;
  const hours = Math.floor(diffMin / 60);
  const mins = diffMin % 60;
  if (hours < 24) return mins > 0 ? `${hours}h ${mins}m` : `${hours}h`;
  return `${Math.floor(hours / 24)}d`;
}

function projectName(path: string | null | undefined): string | null {
  if (!path) return null;
  const segments = path.split("/").filter(Boolean);
  return segments.length > 0 ? segments[segments.length - 1] : null;
}

const MODES: BreakdownMode[] = ["hosts", "projects", "sessions"];
const MODE_LABELS: Record<BreakdownMode, string> = {
  hosts: "Hosts",
  projects: "Projects",
  sessions: "Sessions",
};
const PAGE_SIZE = 5;
const CONFIRM_TIMEOUT_MS = 3000;

interface TrashIconProps {
  size?: number;
}

function PencilIcon({ size = 12 }: TrashIconProps) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 16 16"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.5"
      strokeLinecap="round"
      strokeLinejoin="round"
    >
      <path d="M11.5 1.5l3 3L5 14H2v-3z" />
      <path d="M9.5 3.5l3 3" />
    </svg>
  );
}

function TrashIcon({ size = 12 }: TrashIconProps) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 16 16"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.5"
      strokeLinecap="round"
      strokeLinejoin="round"
    >
      <path d="M2 4h12" />
      <path d="M5 4V2.5a.5.5 0 0 1 .5-.5h5a.5.5 0 0 1 .5.5V4" />
      <path d="M3.5 4l.75 9.5a1 1 0 0 0 1 .9h5.5a1 1 0 0 0 1-.9L12.5 4" />
      <path d="M6.5 7v4" />
      <path d="M9.5 7v4" />
    </svg>
  );
}

interface BreakdownPanelProps {
  days: number;
  selection: BreakdownSelection | null;
  onSelect: (selection: BreakdownSelection | null) => void;
}

function BreakdownPanel({ days, selection, onSelect }: BreakdownPanelProps) {
  const { toast } = useToast();
  const [mode, setMode] = useState<BreakdownMode>("hosts");
  const [page, setPage] = useState(0);
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [deleting, setDeleting] = useState(false);
  const [renaming, setRenaming] = useState(false);
  const [editingCwd, setEditingCwd] = useState<string | null>(null);
  const renameInputRef = useRef<HTMLInputElement | null>(null);
  const confirmTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const { data, loading, error, refresh } = useBreakdownData(mode, days);

  const totalPages = Math.max(1, Math.ceil(data.length / PAGE_SIZE));
  const currentPage = Math.min(page, totalPages - 1);
  const pageData = useMemo(
    () => data.slice(currentPage * PAGE_SIZE, (currentPage + 1) * PAGE_SIZE),
    [data, currentPage],
  );

  const handleModeChange = (m: BreakdownMode) => {
    setMode(m);
    setPage(0);
    resetConfirm();
  };

  const handleRowClick = (
    type: BreakdownSelection["type"],
    key: string,
    row: { first_seen?: string; last_active: string },
  ) => {
    resetConfirm();
    if (selection?.type === type && selection?.key === key) {
      onSelect(null);
    } else {
      onSelect({
        type,
        key,
        firstSeen: row.first_seen || row.last_active,
        lastActive: row.last_active,
      });
    }
  };

  const handleSessionRowClick = (row: SessionBreakdown) => {
    const ref: SessionRef = {
      provider: row.provider,
      session_id: row.session_id,
    };
    const key = sessionRefKey(ref);

    resetConfirm();
    if (selection?.type === "session" && selection.key === key) {
      onSelect(null);
      return;
    }

    onSelect({
      type: "session",
      key,
      firstSeen: row.first_seen,
      lastActive: row.last_active,
      provider: row.provider,
      sessionId: row.session_id,
    });
  };

  const isSelected = (type: BreakdownSelection["type"], key: string) =>
    selection?.type === type && selection?.key === key;

  const resetConfirm = useCallback(() => {
    setConfirmDelete(false);
    if (confirmTimer.current) {
      clearTimeout(confirmTimer.current);
      confirmTimer.current = null;
    }
  }, []);

  const handleDeleteClick = useCallback(async () => {
    if (!selection) return;

    if (!confirmDelete) {
      setConfirmDelete(true);
      confirmTimer.current = setTimeout(() => {
        setConfirmDelete(false);
      }, CONFIRM_TIMEOUT_MS);
      return;
    }

    // Confirmed — perform delete
    resetConfirm();
    setDeleting(true);

    let command: string;
    let args: Record<string, string>;

    switch (selection.type) {
      case "host":
        command = "delete_host_data";
        args = { hostname: selection.key };
        break;
      case "project":
        command = "delete_project_data";
        args = { cwd: selection.key };
        break;
      case "session":
        if (!selection.provider || !selection.sessionId) {
          setDeleting(false);
          toast("error", "Missing provider data for this session");
          return;
        }
        command = "delete_session_data";
        args = {
          provider: selection.provider,
          sessionId: selection.sessionId,
        };
        break;
    }

    try {
      await invoke(command, args);
      onSelect(null);
      refresh();
    } catch (err) {
      toast("error", `Failed to delete ${selection.type} data: ${err}`);
    } finally {
      setDeleting(false);
    }
  }, [selection, confirmDelete, resetConfirm, onSelect, refresh, toast]);

  const handleRenameStart = useCallback(() => {
    if (!selection || selection.type !== "project") return;
    resetConfirm();
    setEditingCwd(selection.key);
    // Auto-focus after render
    setTimeout(() => renameInputRef.current?.focus(), 0);
  }, [selection, resetConfirm]);

  const handleRenameCancel = useCallback(() => {
    setEditingCwd(null);
    resetConfirm();
  }, [resetConfirm]);

  const handleRenameConfirm = useCallback(async () => {
    if (!selection || !editingCwd) return;
    const newCwd = editingCwd.trim();
    if (!newCwd || newCwd === selection.key) {
      setEditingCwd(null);
      return;
    }
    setRenaming(true);
    try {
      await invoke("rename_project", { oldCwd: selection.key, newCwd });
      setEditingCwd(null);
      onSelect(null);
      refresh();
      toast("info", `Renamed to ${newCwd.split("/").filter(Boolean).pop()}`);
    } catch (err) {
      toast("error", `Rename failed: ${err}`);
    } finally {
      setRenaming(false);
    }
  }, [selection, editingCwd, onSelect, refresh, toast]);

  const willMerge = editingCwd !== null && data.some(
    (row) => "project" in row && (row as ProjectBreakdown).project === editingCwd.trim()
      && editingCwd.trim() !== selection?.key,
  );

  return (
    <div className="breakdown-panel">
      <div className="breakdown-header">
        <div className="range-tabs breakdown-toggle">
          {MODES.map((m) => (
            <button
              key={m}
              className={`range-tab${mode === m ? " active" : ""}`}
              aria-pressed={mode === m}
              onClick={() => handleModeChange(m)}
            >
              {MODE_LABELS[m]}
            </button>
          ))}
        </div>
        {selection && (
          <button
            className="breakdown-clear-btn"
            onClick={() => {
              resetConfirm();
              onSelect(null);
            }}
            aria-label="Clear selection"
          >
            &#10005;
          </button>
        )}
        <div className="breakdown-header-right">
          {data.length > PAGE_SIZE && (
            <div className="breakdown-pagination">
              <button
                className="breakdown-page-btn"
                disabled={currentPage === 0}
                onClick={() => setPage((p) => p - 1)}
                aria-label="Previous page"
              >
                &#9664;
              </button>
              <span className="breakdown-page-info">
                {currentPage + 1}/{totalPages}
              </span>
              <button
                className="breakdown-page-btn"
                disabled={currentPage >= totalPages - 1}
                onClick={() => setPage((p) => p + 1)}
                aria-label="Next page"
              >
                &#9654;
              </button>
            </div>
          )}
          {selection && selection.type === "project" && editingCwd === null && (
            <button
              className="breakdown-rename-btn"
              onClick={handleRenameStart}
              disabled={renaming || deleting}
              aria-label={`Rename project: ${selection.key}`}
              title="Rename project path"
            >
              <PencilIcon size={11} />
            </button>
          )}
          {selection && (
            <button
              className={`breakdown-delete-btn${confirmDelete ? " confirm" : ""}`}
              onClick={handleDeleteClick}
              disabled={deleting || renaming}
              aria-label={
                confirmDelete
                  ? `Confirm delete ${selection.type}: ${selection.key}`
                  : `Delete ${selection.type}: ${selection.key}`
              }
              title={
                confirmDelete
                  ? "Click again to confirm"
                  : `Delete all data for this ${selection.type}`
              }
            >
              {deleting ? (
                <span className="breakdown-delete-spinner" />
              ) : confirmDelete ? (
                "Delete?"
              ) : (
                <TrashIcon size={11} />
              )}
            </button>
          )}
        </div>
      </div>

      {error && <div className="analytics-error">{error}</div>}

      {loading ? (
        <div className="breakdown-empty">{"Loading\u2026"}</div>
      ) : data.length === 0 ? (
        <div className="breakdown-empty">No {mode} data yet</div>
      ) : (
        <div
          className="breakdown-list"
          role="list"
          aria-label={`${MODE_LABELS[mode]} breakdown`}
        >
          {mode === "hosts"
            ? (pageData as HostBreakdown[]).map((row) => (
                <div
                  key={row.hostname}
                  className={`breakdown-row${isSelected("host", row.hostname) ? " selected" : ""}`}
                  role="listitem"
                  tabIndex={0}
                  aria-label={`${row.hostname}: ${formatTokenCount(row.total_tokens)} tokens, ${row.turn_count} turns`}
                  onClick={() => handleRowClick("host", row.hostname, row)}
                  onKeyDown={(e) => {
                    if (e.key === "Enter" || e.key === " ") {
                      e.preventDefault();
                      handleRowClick("host", row.hostname, row);
                    }
                  }}
                >
                  <span className="breakdown-name" title={row.hostname}>
                    {row.hostname}
                  </span>
                  <span className="breakdown-tokens">
                    {formatTokenCount(row.total_tokens)}
                  </span>
                  <span className="breakdown-turns">
                    {row.turn_count} turns
                    {row.turn_count > 0 && (
                      <span className="breakdown-tpt"> · {formatTokenCount(Math.round(row.total_tokens / row.turn_count))}/t</span>
                    )}
                  </span>
                  <span className="breakdown-time">
                    {formatRelativeTime(row.last_active)}
                  </span>
                </div>
              ))
            : mode === "projects"
              ? (pageData as ProjectBreakdown[]).map((row) => {
                  const isEditing = editingCwd !== null && isSelected("project", row.project);
                  return (
                  <div
                    key={`${row.project}::${row.hostname}`}
                    className={`breakdown-row breakdown-row-project${isSelected("project", row.project) ? " selected" : ""}`}
                    role="listitem"
                    tabIndex={isEditing ? -1 : 0}
                    aria-label={`${projectName(row.project)} on ${row.hostname}: ${formatTokenCount(row.total_tokens)} tokens, ${row.turn_count} turns, ${row.session_count} sessions`}
                    onClick={() => { if (!isEditing) handleRowClick("project", row.project, row); }}
                    onKeyDown={(e) => {
                      if (!isEditing && (e.key === "Enter" || e.key === " ")) {
                        e.preventDefault();
                        handleRowClick("project", row.project, row);
                      }
                    }}
                  >
                    {isEditing ? (
                      <span className="breakdown-name breakdown-rename-input-wrap">
                        <input
                          ref={renameInputRef}
                          className="breakdown-rename-input"
                          type="text"
                          value={editingCwd}
                          onChange={(e) => setEditingCwd(e.target.value)}
                          onKeyDown={(e) => {
                            if (e.key === "Enter") { e.preventDefault(); handleRenameConfirm(); }
                            if (e.key === "Escape") { e.preventDefault(); handleRenameCancel(); }
                          }}
                          disabled={renaming}
                          aria-label="New project path"
                        />
                        <button
                          className="breakdown-rename-confirm"
                          onClick={handleRenameConfirm}
                          disabled={renaming}
                          aria-label="Confirm rename"
                          title="Confirm"
                        >
                          &#10003;
                        </button>
                        <button
                          className="breakdown-rename-cancel"
                          onClick={handleRenameCancel}
                          disabled={renaming}
                          aria-label="Cancel rename"
                          title="Cancel"
                        >
                          &#10005;
                        </button>
                        {willMerge && (
                          <span className="breakdown-rename-hint">Will merge into existing project</span>
                        )}
                      </span>
                    ) : (
                      <>
                        <span className="breakdown-name" title={row.project}>
                          {projectName(row.project)}
                          <span className="breakdown-host-tag">{row.hostname}</span>
                        </span>
                        <span className="breakdown-tokens">
                          {formatTokenCount(row.total_tokens)}
                        </span>
                        <span className="breakdown-turns">
                          {row.turn_count} turns
                          {row.turn_count > 0 && (
                            <span className="breakdown-tpt"> · {formatTokenCount(Math.round(row.total_tokens / row.turn_count))}/t</span>
                          )}
                          <span className="breakdown-session-count">
                            {row.session_count} sess
                          </span>
                        </span>
                        <span className="breakdown-time">
                          {formatRelativeTime(row.last_active)}
                        </span>
                      </>
                    )}
                  </div>
                  );
                })
              : (pageData as SessionBreakdown[]).map((row) => (
                  <div
                    key={sessionRefKey({
                      provider: row.provider,
                      session_id: row.session_id,
                    })}
                    className={`breakdown-row breakdown-row-session${isSelected(
                      "session",
                      sessionRefKey({
                        provider: row.provider,
                        session_id: row.session_id,
                      }),
                    ) ? " selected" : ""}`}
                    role="listitem"
                    tabIndex={0}
                    aria-label={`Session ${row.session_id.slice(0, 8)}${projectName(row.project) ? ` in ${projectName(row.project)}` : ""} on ${row.hostname}: ${formatTokenCount(row.total_tokens)} tokens, ${row.turn_count} turns`}
                    onClick={() => handleSessionRowClick(row)}
                    onKeyDown={(e) => {
                      if (e.key === "Enter" || e.key === " ") {
                        e.preventDefault();
                        handleSessionRowClick(row);
                      }
                    }}
                  >
                    <span className="breakdown-name" title={row.session_id}>
                      {row.session_id.slice(0, 8)}
                      <span
                        className={`breakdown-provider-tag ${row.provider}`}
                        title={providerLabel(row.provider)}
                      >
                        {providerLabel(row.provider)}
                      </span>
                      {projectName(row.project) && (
                        <span
                          className="breakdown-project-tag"
                          title={row.project ?? undefined}
                        >
                          {projectName(row.project)}
                        </span>
                      )}
                      <span className="breakdown-host-tag">{row.hostname}</span>
                    </span>
                    <span className="breakdown-tokens">
                      {formatTokenCount(row.total_tokens)}
                    </span>
                    <span className="breakdown-turns">
                      {row.turn_count} turns
                      {row.turn_count > 0 && (
                        <span className="breakdown-tpt"> · {formatTokenCount(Math.round(row.total_tokens / row.turn_count))}/t</span>
                      )}
                    </span>
                    <span className="breakdown-time">
                      {Date.now() - new Date(row.last_active).getTime() < 300000
                        ? "active"
                        : formatDuration(row.first_seen, row.last_active)}
                    </span>
                  </div>
                ))}
        </div>
      )}
    </div>
  );
}

export default BreakdownPanel;
