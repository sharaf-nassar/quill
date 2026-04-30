import { useState, useEffect, type CSSProperties } from "react";
import { useMemoryData, type MemoryFile } from "../../hooks/useMemoryData";
import { SuggestionCard } from "./SuggestionCard";
import type { ProviderFilter } from "../../types";
import {
  memoryTypeLabel,
  providerBadgeClass,
  providerLabel,
  providerScopeClass,
  providerScopeLabel,
} from "../../utils/providers";

const STYLES = {
  selectorRow: {
    display: "flex",
    gap: 4,
    alignItems: "center",
    marginTop: 6,
  } as CSSProperties,
  select: {
    flex: 1,
    background: "#1e1e24",
    border: "1px solid rgba(255,255,255,0.1)",
    borderRadius: 4,
    color: "#d4d4d4",
    fontSize: 11,
    padding: "3px 6px",
  } as CSSProperties,
  managePanel: {
    marginTop: 4,
    padding: 6,
    background: "#1e1e24",
    border: "1px solid rgba(255,255,255,0.1)",
    borderRadius: 6,
  } as CSSProperties,
  showEmptyLabel: {
    display: "flex",
    alignItems: "center",
    gap: 6,
    fontSize: 10,
    color: "rgba(255,255,255,0.5)",
    cursor: "pointer",
    marginBottom: 6,
  } as CSSProperties,
  deleteConfirmRow: {
    display: "flex",
    gap: 4,
    alignItems: "center",
    fontSize: 10,
  } as CSSProperties,
  deleteConfirmLabel: { color: "#EF4444" } as CSSProperties,
  deleteConfirmBtn: {
    borderColor: "#EF4444",
    color: "#EF4444",
    fontSize: 9,
    padding: "2px 8px",
  } as CSSProperties,
  cancelBtn: { fontSize: 10, color: "#888" } as CSSProperties,
  deleteAllBtn: {
    borderColor: "rgba(239,68,68,0.4)",
    color: "#EF4444",
    fontSize: 9,
    padding: "2px 8px",
    width: "100%",
  } as CSSProperties,
  customProjectsHeader: {
    fontSize: 9,
    color: "rgba(255,255,255,0.4)",
    marginBottom: 4,
  } as CSSProperties,
  customProjectRow: {
    display: "flex",
    alignItems: "center",
    gap: 4,
    fontSize: 10,
    padding: "2px 0",
  } as CSSProperties,
  customProjectName: { flex: 1, color: "#d4d4d4" } as CSSProperties,
  addProjectRow: { display: "flex", gap: 4, marginTop: 4 } as CSSProperties,
  addProjectInput: {
    flex: 1,
    background: "#121216",
    border: "1px solid rgba(255,255,255,0.15)",
    borderRadius: 3,
    color: "#d4d4d4",
    fontSize: 10,
    padding: "2px 4px",
  } as CSSProperties,
  addBtn: { fontSize: 9, padding: "2px 6px" } as CSSProperties,
  manageCogBtn: { fontSize: 11 } as CSSProperties,
  optimizeAllBtn: {
    fontSize: 9,
    padding: "2px 8px",
    whiteSpace: "nowrap",
  } as CSSProperties,
  tabBtnBase: { fontSize: 10 } as CSSProperties,
  fileConfirmBar: {
    display: "flex",
    gap: 4,
    alignItems: "center",
    fontSize: 10,
    padding: "4px 8px",
    marginBottom: 3,
    background: "rgba(239,68,68,0.08)",
    border: "1px solid rgba(239,68,68,0.2)",
    borderRadius: 6,
  } as CSSProperties,
  fileConfirmLabel: { color: "#EF4444", flex: 1 } as CSSProperties,
  optimizeRow: {
    display: "flex",
    gap: 4,
    alignItems: "center",
    marginTop: 6,
  } as CSSProperties,
  logsArea: { marginTop: 4 } as CSSProperties,
  sectionHeaderClickable: { cursor: "pointer" } as CSSProperties,
  deleteAllDeleteMb: { marginBottom: 6 } as CSSProperties,
} as const;

const TYPE_COLORS: Record<string, string> = {
  user: "#3B82F6",
  feedback: "#EF4444",
  project: "#22C55E",
  reference: "#EAB308",
  "claude-md": "#A855F7",
  "agents-md": "#F97316",
};

const CHANGED_STYLE: CSSProperties = {
  fontSize: 9,
  color: "#EAB308",
  fontWeight: 600,
};

function typeBadgeStyle(type: string): CSSProperties {
  const color = TYPE_COLORS[type] || "#888";
  return {
    fontSize: 9,
    padding: "1px 5px",
    borderRadius: 3,
    background: `${color}20`,
    color,
    textTransform: "uppercase",
    fontWeight: 600,
  };
}

interface MemoryFileCardProps {
  file: MemoryFile;
  expanded: boolean;
  onToggle: (fp: string) => void;
  onDelete: (path: string, name: string) => void;
}

function MemoryFileCard({ file: mf, expanded, onToggle, onDelete }: MemoryFileCardProps) {
  return (
    <div className="learning-rule-card">
      <div
        className="learning-rule-header"
        onClick={() => onToggle(mf.file_path)}
      >
        <span className="learning-rule-expand">
          {expanded ? "▾" : "▸"}
        </span>
        <span className="learning-rule-name">{mf.file_name}</span>
        <span className={providerBadgeClass(mf.provider)}>
          {providerLabel(mf.provider)}
        </span>
        {mf.memory_type && (
          <span style={typeBadgeStyle(mf.memory_type)}>
            {memoryTypeLabel(mf.memory_type)}
          </span>
        )}
        {mf.changed_since_last_run && (
          <span style={CHANGED_STYLE}>changed</span>
        )}
        <button
          className="learning-rule-delete"
          title="Delete memory file"
          onClick={(e) => {
            e.stopPropagation();
            onDelete(mf.file_path, mf.file_name);
          }}
        >
          x
        </button>
      </div>
      {mf.description && (
        <span className="learning-rule-domain">{mf.description}</span>
      )}
      {expanded && (
        <pre className="learning-rule-content">{mf.content}</pre>
      )}
    </div>
  );
}

interface InstructionFileCardProps {
  file: MemoryFile;
  expanded: boolean;
  onToggle: (fp: string) => void;
}

function InstructionFileCard({
  file: mf,
  expanded,
  onToggle,
}: InstructionFileCardProps) {
  return (
    <div className="learning-rule-card">
      <div
        className="learning-rule-header"
        onClick={() => onToggle(mf.file_path)}
      >
        <span className="learning-rule-expand">
          {expanded ? "▾" : "▸"}
        </span>
        <span className="learning-rule-name">{mf.file_name}</span>
        <span className={providerBadgeClass(mf.provider)}>
          {providerLabel(mf.provider)}
        </span>
        <span style={typeBadgeStyle(mf.memory_type ?? "reference")}>
          {memoryTypeLabel(mf.memory_type)}
        </span>
        {mf.changed_since_last_run && (
          <span style={CHANGED_STYLE}>changed</span>
        )}
      </div>
      {mf.description && (
        <span className="learning-rule-domain">{mf.description}</span>
      )}
      {expanded && (
        <pre className="learning-rule-content">{mf.content}</pre>
      )}
    </div>
  );
}

interface MemoriesPanelProps {
  providerFilter: ProviderFilter;
}

export function MemoriesPanel({ providerFilter }: MemoriesPanelProps) {
  const {
    projects,
    selectedProject,
    setSelectedProject,
    memoryFiles,
    suggestions,
    runs,
    optimizing,
    loading,
    logs,
    triggerOptimization,
    triggerOptimizeAll,
    approveSuggestion,
    denySuggestion,
    undenySuggestion,
    undoSuggestion,
    approveGroup,
    denyGroup,
    addCustomProject,
    removeCustomProject,
    deleteMemoryFile,
    deleteCurrentViewMemories,
  } = useMemoryData(providerFilter);

  const [showManage, setShowManage] = useState(false);
  const [newPath, setNewPath] = useState("");
  const [showDenied, setShowDenied] = useState(false);
  const [showApproved, setShowApproved] = useState(false);
  const [showHistory, setShowHistory] = useState(false);
  const [expandedFiles, setExpandedFiles] = useState<Set<string>>(new Set());
  const [confirmDelete, setConfirmDelete] = useState<
    { type: "file"; path: string; name: string; projectPath?: string } | { type: "project"; path: string } | null
  >(null);
  const [showEmpty, setShowEmpty] = useState(false);
  const [compressProse, setCompressProse] = useState(false);

  // Filter projects: only show those with memories unless toggled
  const visibleProjects = showEmpty
    ? projects
    : projects.filter((p) => p.memory_count > 0 || p.is_custom);

  const isAllView = selectedProject === "__all__";
  const totalMemoryCount = projects.reduce((sum, p) => sum + p.memory_count, 0);

  // If selected project is no longer in the visible list, auto-select first visible
  useEffect(() => {
    if (
      selectedProject !== "__all__" &&
      visibleProjects.length > 0 &&
      !visibleProjects.some((p) => p.path === selectedProject)
    ) {
      setSelectedProject(visibleProjects[0].path);
    }
  }, [visibleProjects, selectedProject, setSelectedProject]);

  const actualMemoryFiles = memoryFiles.filter(
    (file) => !["claude-md", "agents-md"].includes(file.memory_type ?? ""),
  );
  const instructionFiles = memoryFiles.filter((file) =>
    ["claude-md", "agents-md"].includes(file.memory_type ?? ""),
  );

  const pendingSuggestions = suggestions.filter((s) => s.status === "pending");
  const deniedSuggestions = suggestions.filter((s) => s.status === "denied");
  const approvedSuggestions = suggestions.filter((s) => s.status === "approved");
  const actionableSuggestions = [...pendingSuggestions, ...suggestions.filter((s) => s.status === "undone")];

  const projectsWithMemories = projects.filter((p) => p.memory_count > 0);
  const currentViewMemoryCount = actualMemoryFiles.length;
  const currentViewProjectCount = isAllView
    ? projectsWithMemories.length
    : selectedProject
      ? 1
      : 0;
  const deleteCurrentViewLabel = isAllView
    ? "Delete all memories for current view"
    : "Delete all memories for this project";
  const deleteCurrentViewConfirmLabel = isAllView
    ? `Delete all ${currentViewMemoryCount} memories across ${currentViewProjectCount} projects?`
    : `Delete all ${currentViewMemoryCount} memories?`;

  const toggleFile = (fp: string) => {
    setExpandedFiles((prev) => {
      const next = new Set(prev);
      if (next.has(fp)) next.delete(fp);
      else next.add(fp);
      return next;
    });
  };

  return (
    <div>
      {/* Project selector row */}
      <div style={STYLES.selectorRow}>
        <select
          value={selectedProject}
          onChange={(e) => setSelectedProject(e.target.value)}
          style={STYLES.select}
        >
          <option value="__all__">All Projects ({totalMemoryCount})</option>
          {visibleProjects.map((p) => (
            <option key={p.path} value={p.path}>
              {p.name} ({p.memory_count})
            </option>
          ))}
        </select>
        <button
          className="learning-cog-btn"
          onClick={() => setShowManage(!showManage)}
          title="Manage projects"
          style={STYLES.manageCogBtn}
        >
          {showManage ? "−" : "+"}
        </button>
        <button
          className="learning-analyze-btn"
          style={STYLES.optimizeAllBtn}
          disabled={optimizing || projectsWithMemories.length === 0}
          onClick={triggerOptimizeAll}
        >
          {optimizing ? "..." : "Optimize All"}
        </button>
      </div>

      {/* Manage panel */}
      {showManage && (
        <div style={STYLES.managePanel}>
          {/* Show empty toggle */}
          <label style={STYLES.showEmptyLabel}>
            <input
              type="checkbox"
              checked={showEmpty}
              onChange={(e) => setShowEmpty(e.target.checked)}
            />
            Show projects with no memories
          </label>

          {/* Delete all memories for current view */}
          {selectedProject && currentViewMemoryCount > 0 && (
            <div style={STYLES.deleteAllDeleteMb}>
              {confirmDelete?.type === "project" && confirmDelete.path === selectedProject ? (
                <div style={STYLES.deleteConfirmRow}>
                  <span style={STYLES.deleteConfirmLabel}>
                    {deleteCurrentViewConfirmLabel}
                  </span>
                  <button
                    className="learning-analyze-btn"
                    style={STYLES.deleteConfirmBtn}
                    onClick={async () => {
                      await deleteCurrentViewMemories();
                      setConfirmDelete(null);
                    }}
                  >
                    Confirm
                  </button>
                  <button
                    className="learning-rule-delete"
                    style={STYLES.cancelBtn}
                    onClick={() => setConfirmDelete(null)}
                  >
                    Cancel
                  </button>
                </div>
              ) : (
                <button
                  className="learning-analyze-btn"
                  style={STYLES.deleteAllBtn}
                  onClick={() => setConfirmDelete({ type: "project", path: selectedProject })}
                >
                  {deleteCurrentViewLabel}
                </button>
              )}
            </div>
          )}

          <div style={STYLES.customProjectsHeader}>
            CUSTOM PROJECTS
          </div>
          {projects
            .filter((p) => p.is_custom)
            .map((p) => (
              <div key={p.path} style={STYLES.customProjectRow}>
                <span style={STYLES.customProjectName}>{p.path}</span>
                <button
                  className="learning-rule-delete"
                  onClick={() => removeCustomProject(p.path)}
                >
                  x
                </button>
              </div>
            ))}
          <div style={STYLES.addProjectRow}>
            <input
              value={newPath}
              onChange={(e) => setNewPath(e.target.value)}
              placeholder="/path/to/project"
              style={STYLES.addProjectInput}
              onKeyDown={(e) => {
                if (e.key === "Enter" && newPath.trim()) {
                  addCustomProject(newPath.trim());
                  setNewPath("");
                }
              }}
            />
            <button
              className="learning-analyze-btn"
              style={STYLES.addBtn}
              onClick={() => {
                if (newPath.trim()) {
                  addCustomProject(newPath.trim());
                  setNewPath("");
                }
              }}
            >
              Add
            </button>
          </div>
        </div>
      )}

      {loading ? (
        <div className="learning-empty">Loading...</div>
      ) : isAllView ? (
        /* ─── All Projects grouped view ─── */
        <>
          {actualMemoryFiles.length === 0 ? (
            <div className="learning-empty">
              No memory files across any project.
            </div>
          ) : (
            (() => {
              // Group by project_path, preserve project order
              const byProject = new Map<string, typeof actualMemoryFiles>();
              for (const mf of actualMemoryFiles) {
                const list = byProject.get(mf.project_path) || [];
                list.push(mf);
                byProject.set(mf.project_path, list);
              }
              // Resolve display name from projects list
              const nameFor = (path: string) =>
                projects.find((p) => p.path === path)?.name || path.split("/").pop() || path;

              return [...byProject.entries()].map(([projectPath, files]) => (
                <div key={projectPath} className="learning-section">
                  {(() => {
                    const projectProviders =
                      projects.find((project) => project.path === projectPath)?.providers ?? [];
                    return (
                  <div
                    className="learning-section-header"
                    style={{ cursor: "pointer" }}
                    onClick={() => setSelectedProject(projectPath)}
                    title={`Switch to ${nameFor(projectPath)}`}
                  >
                    {nameFor(projectPath)}
                    {projectProviders.map((provider) => (
                      <span key={`${projectPath}-${provider}`} className={providerBadgeClass(provider)}>
                        {providerLabel(provider)}
                      </span>
                    ))}
                    <span className="learning-section-count">
                      {files.length}
                    </span>
                  </div>
                    );
                  })()}
                  {files.map((mf) => (
                    <MemoryFileCard
                      key={mf.file_path}
                      file={mf}
                      expanded={expandedFiles.has(mf.file_path)}
                      onToggle={toggleFile}
                      onDelete={(path, name) =>
                        setConfirmDelete({ type: "file", path, name, projectPath: mf.project_path })
                      }
                    />
                  ))}
                </div>
              ));
            })()
          )}
          {confirmDelete?.type === "file" && (
            <div style={STYLES.fileConfirmBar}>
              <span style={STYLES.fileConfirmLabel}>
                Delete {confirmDelete.name}?
              </span>
              <button
                className="learning-analyze-btn"
                style={STYLES.deleteConfirmBtn}
                onClick={() => {
                  deleteMemoryFile(confirmDelete.path, confirmDelete.projectPath);
                  setConfirmDelete(null);
                }}
              >
                Delete
              </button>
              <button
                className="learning-rule-delete"
                style={STYLES.cancelBtn}
                onClick={() => setConfirmDelete(null)}
              >
                Cancel
              </button>
            </div>
          )}
        </>
      ) : (
        <>
          {/* Memory files */}
          <div className="learning-section">
            <div className="learning-section-header">
              MEMORY FILES
              <span className="learning-section-count">
                {actualMemoryFiles.length}
              </span>
            </div>
            {confirmDelete?.type === "file" && (
              <div style={STYLES.fileConfirmBar}>
                <span style={STYLES.fileConfirmLabel}>
                  Delete {confirmDelete.name}?
                </span>
                <button
                  className="learning-analyze-btn"
                  style={STYLES.deleteConfirmBtn}
                  onClick={() => {
                    deleteMemoryFile(confirmDelete.path, confirmDelete.projectPath);
                    setConfirmDelete(null);
                  }}
                >
                  Delete
                </button>
                <button
                  className="learning-rule-delete"
                  style={STYLES.cancelBtn}
                  onClick={() => setConfirmDelete(null)}
                >
                  Cancel
                </button>
              </div>
            )}
            {actualMemoryFiles.length === 0 ? (
              <div className="learning-empty">
                No memory files for this project.
              </div>
            ) : (
              actualMemoryFiles.map((mf) => (
                <MemoryFileCard
                  key={mf.file_path}
                  file={mf}
                  expanded={expandedFiles.has(mf.file_path)}
                  onToggle={toggleFile}
                  onDelete={(path, name) =>
                    setConfirmDelete({ type: "file", path, name })
                  }
                />
              ))
            )}
          </div>

          {/* Instruction files */}
          {instructionFiles.length > 0 && (
            <div className="learning-section">
              <div className="learning-section-header">
                INSTRUCTION FILES
                <span className="learning-section-count">
                  {instructionFiles.length}
                </span>
              </div>
              {instructionFiles.map((mf) => (
                <InstructionFileCard
                  key={mf.file_path}
                  file={mf}
                  expanded={expandedFiles.has(mf.file_path)}
                  onToggle={toggleFile}
                />
              ))}
            </div>
          )}

          {/* Optimize buttons + logs */}
          <div style={STYLES.optimizeRow}>
            <button
              className="learning-analyze-btn"
              disabled={optimizing || !selectedProject}
              onClick={() => triggerOptimization({ compressProse })}
              title={
                compressProse
                  ? "Caveman-compress prose in scanned memory files, then run the optimizer"
                  : "Run the memory optimizer"
              }
            >
              {optimizing
                ? "Optimizing..."
                : compressProse
                  ? "Compress + Optimize"
                  : "Optimize"}
            </button>
            <label
              className={`learning-compress-toggle${
                compressProse ? " learning-compress-toggle--active" : ""
              }${optimizing ? " learning-compress-toggle--disabled" : ""}`}
              title="Run caveman-compress on scanned memory files before the optimizer pass. Backs up each file as <name>.original.md."
            >
              <input
                type="checkbox"
                checked={compressProse}
                disabled={optimizing}
                onChange={(e) => setCompressProse(e.target.checked)}
              />
              Compress prose
            </label>
            {runs.length > 0 && (
              <button
                className={`learning-runs-btn${showHistory ? " learning-runs-btn--active" : ""}`}
                onClick={() => setShowHistory(!showHistory)}
              >
                History
                <span className="learning-runs-badge">{runs.length}</span>
              </button>
            )}
          </div>

          {/* Live logs during optimization */}
          {optimizing && logs.length > 0 && (
            <div className="learning-run-detail-logs" style={STYLES.logsArea}>
              {logs.join("\n")}
            </div>
          )}

          {/* Suggestions */}
          {actionableSuggestions.length > 0 && (
            <div className="learning-section">
              <div className="learning-section-header">
                SUGGESTIONS
                <span className="learning-section-count">
                  {actionableSuggestions.length}
                </span>
              </div>
              {(() => {
                // Partition into grouped and ungrouped
                const grouped = new Map<string, typeof actionableSuggestions>();
                const ungrouped: typeof actionableSuggestions = [];
                for (const s of actionableSuggestions) {
                  if (s.group_id) {
                    const list = grouped.get(s.group_id) || [];
                    list.push(s);
                    grouped.set(s.group_id, list);
                  } else {
                    ungrouped.push(s);
                  }
                }
                return (
                  <>
                    {/* Grouped suggestions */}
                    {[...grouped.entries()].map(([gid, items]) => (
                      <div
                        key={gid}
                        style={{
                          border: "1px solid rgba(139,92,246,0.3)",
                          borderRadius: 6,
                          padding: 6,
                          marginBottom: 4,
                          background: "rgba(139,92,246,0.04)",
                        }}
                      >
                        <div
                          style={{
                            display: "flex",
                            alignItems: "center",
                            gap: 6,
                            marginBottom: 4,
                          }}
                        >
                          <span
                            style={{
                              fontSize: 9,
                              fontWeight: 700,
                              color: "#8B5CF6",
                              textTransform: "uppercase",
                              letterSpacing: 0.5,
                            }}
                          >
                            Linked ({items.length})
                          </span>
                          <span
                            style={{
                              fontSize: 9,
                              color: "rgba(255,255,255,0.4)",
                              flex: 1,
                            }}
                          >
                            These share files — approve or deny together
                          </span>
                          <button
                            className="learning-analyze-btn"
                            style={{
                              borderColor: "#8B5CF6",
                              color: "#8B5CF6",
                              fontSize: 9,
                              padding: "2px 8px",
                            }}
                            onClick={() => approveGroup(gid)}
                          >
                            Approve All
                          </button>
                          <button
                            className="learning-rule-delete"
                            style={{ fontSize: 11, color: "#EF4444" }}
                            onClick={() => denyGroup(gid)}
                          >
                            Deny All
                          </button>
                        </div>
                        {items.map((s) => (
                          <SuggestionCard
                            key={s.id}
                            suggestion={s}
                            onApprove={approveSuggestion}
                            onDeny={denySuggestion}
                            onUndo={undoSuggestion}
                            inGroup
                          />
                        ))}
                      </div>
                    ))}
                    {/* Ungrouped suggestions */}
                    {ungrouped.map((s) => (
                      <SuggestionCard
                        key={s.id}
                        suggestion={s}
                        onApprove={approveSuggestion}
                        onDeny={denySuggestion}
                        onUndo={undoSuggestion}
                      />
                    ))}
                  </>
                );
              })()}
            </div>
          )}

          {/* Denied suggestions (togglable) */}
          {deniedSuggestions.length > 0 && (
            <div className="learning-section">
              <div
                className="learning-section-header"
                style={STYLES.sectionHeaderClickable}
                onClick={() => setShowDenied(!showDenied)}
              >
                {showDenied ? "▾" : "▸"} DENIED
                <span className="learning-section-count">
                  {deniedSuggestions.length}
                </span>
              </div>
              {showDenied &&
                deniedSuggestions.map((s) => (
                  <SuggestionCard
                    key={s.id}
                    suggestion={s}
                    onApprove={approveSuggestion}
                    onDeny={denySuggestion}
                    onUndeny={undenySuggestion}
                    onUndo={undoSuggestion}
                  />
                ))}
            </div>
          )}

          {/* Approved suggestions (togglable) */}
          {approvedSuggestions.length > 0 && (
            <div className="learning-section">
              <div
                className="learning-section-header"
                style={STYLES.sectionHeaderClickable}
                onClick={() => setShowApproved(!showApproved)}
              >
                {showApproved ? "▾" : "▸"} APPROVED
                <span className="learning-section-count">
                  {approvedSuggestions.length}
                </span>
              </div>
              {showApproved &&
                approvedSuggestions.map((s) => (
                  <SuggestionCard
                    key={s.id}
                    suggestion={s}
                    onApprove={approveSuggestion}
                    onDeny={denySuggestion}
                    onUndeny={undenySuggestion}
                    onUndo={undoSuggestion}
                  />
                ))}
            </div>
          )}

          {/* Run history */}
          {showHistory && (
            <div className="learning-section">
              <div className="learning-section-header">RUN HISTORY</div>
              <div className="learning-runs-list">
                {runs.map((r) => (
                  <div key={r.id} className="learning-run-row">
                    <span
                      className={`learning-run-icon learning-run-icon--${
                        r.status === "completed"
                          ? "ok"
                          : r.status === "failed"
                            ? "fail"
                            : "live-text"
                      }`}
                    >
                      {r.status === "completed"
                        ? "✓"
                        : r.status === "failed"
                          ? "✗"
                          : "●"}
                    </span>
                    <span className="learning-run-trigger">{r.trigger}</span>
                    <span className={providerScopeClass(r.provider_scope)}>
                      {providerScopeLabel(r.provider_scope)}
                    </span>
                    <span className="learning-run-result">
                      {r.memories_scanned} scanned, {r.suggestions_created}{" "}
                      suggestions
                    </span>
                    <span className="learning-run-time">
                      {new Date(r.started_at).toLocaleDateString()}
                    </span>
                  </div>
                ))}
              </div>
            </div>
          )}
        </>
      )}
    </div>
  );
}
