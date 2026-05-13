import { Fragment, useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useBreakdownData } from "../../hooks/useBreakdownData";
import { useToast } from "../../hooks/useToast";
import { useSessionSubagents } from "../../hooks/useSessionSubagents";
import { useSkillProjects } from "../../hooks/useSkillProjects";
import { formatTokenCount } from "../../utils/tokens";
import type {
  BreakdownMode,
  BreakdownSelection,
  HostBreakdown,
  IntegrationProvider,
  ProjectBreakdown,
  SessionBreakdown,
  SessionRef,
  SkillBreakdown,
  SkillProjectBreakdown,
  SubagentNode,
} from "../../types";
import { sessionRefKey } from "../../types";

// Defensive guard against pathological parent/child cycles in the
// SubagentNode array. Today the backend always returns depth-1 nodes
// (parent_agent_id === null) but Wave-N may introduce chains; render no
// further than this depth so a malformed cycle cannot freeze the panel.
const SUBAGENT_MAX_DEPTH = 10;
// One indent step per depth level. 24px matches the visual mock.
const SUBAGENT_INDENT_PX = 24;

function formatRelativeTime(isoString: string, now: number = Date.now()): string {
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

const MODES: BreakdownMode[] = ["sessions", "projects", "hosts", "skills"];
const MODE_LABELS: Record<BreakdownMode, string> = {
  hosts: "Hosts",
  projects: "Projects",
  sessions: "Sessions",
  skills: "Skills",
};
const CONFIRM_TIMEOUT_MS = 3000;
type SkillProviderFilter = "all" | "claude" | "codex";
const SKILL_PROVIDER_FILTERS: Array<{ value: SkillProviderFilter; label: string }> = [
  { value: "all", label: "All" },
  { value: "codex", label: "Codex" },
  { value: "claude", label: "Claude" },
];

function providerLabel(provider: SessionRef["provider"]): string {
  return provider === "claude" ? "Claude" : "Codex";
}

function skillEmptyLabel(providerFilter: SkillProviderFilter, allTime: boolean): string {
  const provider = providerFilter === "all"
    ? "all providers"
    : providerFilter === "codex"
      ? "Codex"
      : "Claude Code";
  const scope = allTime ? "all time" : "this timeframe";
  return `No ${provider} skill usage for ${scope}`;
}

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

function ChevronIcon({ open }: { open: boolean }) {
  return (
    <svg
      width={10}
      height={10}
      viewBox="0 0 10 10"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.6"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden
      style={{
        transform: open ? "rotate(90deg)" : "rotate(0deg)",
        transition: "transform 120ms ease",
      }}
    >
      <path d="M3 1.5 L7 5 L3 8.5" />
    </svg>
  );
}

/**
 * Group sub-agent nodes by their `parent_agent_id`. Depth-1 nodes are
 * stored under the empty string. Calling site never needs to know about
 * the bucket key directly — `childrenOf` returns the array for a given
 * parent in `first_seen ASC` order (already the backend's contract).
 */
function groupByParent(nodes: SubagentNode[]): Map<string, SubagentNode[]> {
  const map = new Map<string, SubagentNode[]>();
  for (const node of nodes) {
    const parentKey = node.parent_agent_id ?? "";
    const bucket = map.get(parentKey);
    if (bucket) {
      bucket.push(node);
    } else {
      map.set(parentKey, [node]);
    }
  }
  return map;
}

function childrenOf(
  groups: Map<string, SubagentNode[]>,
  parentAgentId: string | null,
  visited: ReadonlySet<string>,
): SubagentNode[] {
  const bucket = groups.get(parentAgentId ?? "") ?? [];
  return bucket.filter((node) => !visited.has(node.agent_id));
}

// Sentinel for root-level `childrenOf` calls. Shared across renders so a
// cycle in the data never triggers an extra `new Set` allocation on the
// hot path.
const EMPTY_VISITED: ReadonlySet<string> = new Set<string>();

interface SubagentRowProps {
  node: SubagentNode;
  depth: number;
  groups: Map<string, SubagentNode[]>;
  expandedAgents: Record<string, boolean>;
  onToggle: (agentId: string) => void;
  visited: ReadonlySet<string>;
}

/**
 * One row inside a session's sub-agent tree. Recursively renders any
 * children whose `parent_agent_id === node.agent_id`. Depth is capped at
 * [[SUBAGENT_MAX_DEPTH]] to defend against pathological cycles.
 */
function SubagentRow({ node, depth, groups, expandedAgents, onToggle, visited }: SubagentRowProps) {
  // Cycle guard: track every agent_id along this branch and skip any child
  // we have already rendered. SUBAGENT_MAX_DEPTH stays as a defence in
  // depth, but `visited` is what actually prevents an A→B→A cycle from
  // emitting duplicate rows.
  const nextVisited = new Set(visited);
  nextVisited.add(node.agent_id);
  const children =
    depth < SUBAGENT_MAX_DEPTH ? childrenOf(groups, node.agent_id, nextVisited) : [];
  const hasChildren = children.length > 0;
  const isOpen = !!expandedAgents[node.agent_id];
  const shortId = node.agent_id.slice(0, 8);
  // Wave 2 always returns label=null; fall back to the full 16-char
  // agent_id so the column is never blank, matching the mockup.
  const labelText = node.label ?? node.agent_id;
  const tokensPerTurn =
    node.turn_count > 0 ? Math.round(node.total_tokens / node.turn_count) : 0;

  return (
    <>
      <div
        className={`breakdown-row breakdown-row-subagent${hasChildren ? " has-children" : ""}`}
        role="listitem"
        tabIndex={0}
        aria-expanded={hasChildren ? isOpen : undefined}
        aria-label={`Sub-agent ${shortId}: ${formatTokenCount(node.total_tokens)} tokens, ${node.turn_count} turns`}
        style={{ paddingLeft: `${10 + depth * SUBAGENT_INDENT_PX}px` }}
        onClick={() => {
          if (hasChildren) onToggle(node.agent_id);
        }}
        onKeyDown={(e) => {
          if (hasChildren && (e.key === "Enter" || e.key === " ")) {
            e.preventDefault();
            onToggle(node.agent_id);
          }
        }}
      >
        <span className="breakdown-name breakdown-name-subagent" title={node.agent_id}>
          <span className="breakdown-tree-guide" aria-hidden>
            {"└─"}
          </span>
          {hasChildren ? (
            <span className="breakdown-chevron" aria-hidden>
              <ChevronIcon open={isOpen} />
            </span>
          ) : (
            <span className="breakdown-chevron breakdown-chevron-spacer" aria-hidden />
          )}
          <span className="breakdown-subagent-id">{shortId}</span>
          <span className="breakdown-provider-tag agent" title="Sub-agent">
            AGENT
          </span>
          <span className="breakdown-subagent-label" title={labelText}>
            {labelText}
          </span>
        </span>
        <span className="breakdown-tokens">{formatTokenCount(node.total_tokens)}</span>
        <span className="breakdown-turns">
          {node.turn_count} turns
          <span className="breakdown-session-count">
            {formatTokenCount(tokensPerTurn)}/t
          </span>
        </span>
        <span className="breakdown-time">
          {formatRelativeTime(node.last_active)}
        </span>
      </div>
      {isOpen &&
        hasChildren &&
        children.map((child) => (
          <SubagentRow
            key={child.agent_id}
            node={child}
            depth={depth + 1}
            groups={groups}
            expandedAgents={expandedAgents}
            onToggle={onToggle}
            visited={nextVisited}
          />
        ))}
    </>
  );
}

interface SessionTreeBranchProps {
  row: SessionBreakdown;
  sessKey: string;
  hasSubagents: boolean;
  isExpanded: boolean;
  isSelected: boolean;
  subState: import("../../hooks/useSessionSubagents").SessionSubagentState | null;
  groups: Map<string, SubagentNode[]> | null;
  expandedAgents: Record<string, boolean>;
  onToggleSession: () => void;
  onRowClick: () => void;
  onToggleAgent: (agentId: string) => void;
}

/**
 * One session row plus its (lazily fetched) sub-agent tree. Click on the
 * chevron toggles expansion; click anywhere else preserves the existing
 * selection / drill-down behaviour. Rows without sub-agents render with a
 * blank chevron spacer so token/turn columns stay aligned.
 */
function SessionTreeBranch({
  row,
  sessKey: _sessKey,
  hasSubagents,
  isExpanded,
  isSelected,
  subState,
  groups,
  expandedAgents,
  onToggleSession,
  onRowClick,
  onToggleAgent,
}: SessionTreeBranchProps) {
  const projectLabel = projectName(row.project);
  // Sample wall-clock once per render so every "active"/relative-time read
  // below is consistent. Hoisting matches `formatRelativeTime`'s pattern
  // and avoids `Date.now()` appearing inside JSX, which would re-evaluate
  // on every child diff. The single read is intentionally impure; the
  // alternative is to lift `now` into a `useSyncExternalStore`, which is
  // overkill for a coarse "<5 min" badge.
  // eslint-disable-next-line react-hooks/purity
  const now = Date.now();
  const topLevelChildren = groups ? childrenOf(groups, null, EMPTY_VISITED) : [];

  return (
    <>
      <div
        className={`breakdown-row breakdown-row-session${isSelected ? " selected" : ""}${hasSubagents ? " has-subagents" : ""}`}
        role="listitem"
        tabIndex={0}
        aria-expanded={hasSubagents ? isExpanded : undefined}
        aria-label={`Session ${row.session_id.slice(0, 8)}${projectLabel ? ` in ${projectLabel}` : ""} on ${row.hostname}: ${formatTokenCount(row.total_tokens)} tokens, ${row.turn_count} turns${hasSubagents ? `, ${row.subagent_count ?? 0} sub-agents` : ""}`}
        onClick={onRowClick}
        onKeyDown={(e) => {
          if (e.key === "Enter" || e.key === " ") {
            e.preventDefault();
            onRowClick();
          }
        }}
      >
        <span className="breakdown-name" title={row.session_id}>
          {hasSubagents ? (
            <button
              type="button"
              className="breakdown-chevron breakdown-chevron-btn"
              aria-label={isExpanded ? "Collapse sub-agents" : "Expand sub-agents"}
              aria-expanded={isExpanded}
              onClick={(e) => {
                // Don't trigger row selection when toggling the tree.
                e.stopPropagation();
                onToggleSession();
              }}
              onKeyDown={(e) => {
                if (e.key === "Enter" || e.key === " ") {
                  e.stopPropagation();
                }
              }}
            >
              <ChevronIcon open={isExpanded} />
            </button>
          ) : (
            <span className="breakdown-chevron breakdown-chevron-spacer" aria-hidden />
          )}
          {row.session_id.slice(0, 8)}
          <span
            className={`breakdown-provider-tag ${row.provider}`}
            title={providerLabel(row.provider)}
          >
            {providerLabel(row.provider)}
          </span>
          {projectLabel && (
            <span
              className="breakdown-project-tag"
              title={row.project ?? undefined}
            >
              {projectLabel}
            </span>
          )}
          <span className="breakdown-host-tag">{row.hostname}</span>
          {hasSubagents && (
            <span
              className="breakdown-session-count"
              title={`${row.subagent_count ?? 0} sub-agents`}
            >
              +{row.subagent_count ?? 0}
            </span>
          )}
        </span>
        <span className="breakdown-tokens">{formatTokenCount(row.total_tokens)}</span>
        <span className="breakdown-turns">{row.turn_count} turns</span>
        <span className="breakdown-time">
          {now - new Date(row.last_active).getTime() < 300000
            ? "active"
            : formatRelativeTime(row.last_active, now)}
          <span className="breakdown-duration">
            {" · "}
            {formatDuration(row.first_seen, row.last_active)}
          </span>
        </span>
      </div>
      {isExpanded && subState && (
        <>
          {subState.loading && subState.nodes.length === 0 && (
            <div
              className="breakdown-row breakdown-row-subagent breakdown-row-subagent-status"
              role="listitem"
              aria-live="polite"
              style={{ paddingLeft: `${10 + SUBAGENT_INDENT_PX}px` }}
            >
              <span className="breakdown-name breakdown-subagent-status-text">
                Loading sub-agents…
              </span>
            </div>
          )}
          {subState.error && (
            <div
              className="breakdown-row breakdown-row-subagent breakdown-row-subagent-status error"
              role="listitem"
              style={{ paddingLeft: `${10 + SUBAGENT_INDENT_PX}px` }}
            >
              <span className="breakdown-name breakdown-subagent-status-text">
                Failed to load sub-agents
              </span>
            </div>
          )}
          {!subState.loading &&
            !subState.error &&
            subState.loaded &&
            topLevelChildren.length === 0 && (
              <div
                className="breakdown-row breakdown-row-subagent breakdown-row-subagent-status"
                role="listitem"
                style={{ paddingLeft: `${10 + SUBAGENT_INDENT_PX}px` }}
              >
                <span className="breakdown-name breakdown-subagent-status-text">
                  No sub-agents
                </span>
              </div>
            )}
          {groups &&
            topLevelChildren.map((node) => (
              <SubagentRow
                key={node.agent_id}
                node={node}
                depth={1}
                groups={groups}
                expandedAgents={expandedAgents}
                onToggle={onToggleAgent}
                visited={EMPTY_VISITED}
              />
            ))}
        </>
      )}
    </>
  );
}

interface SkillProjectRowProps {
  row: SkillProjectBreakdown;
}

/**
 * Read-only sub-row rendered beneath an expanded skill row. Re-uses the
 * sub-agent visual treatment (left tree guide, indent, muted background)
 * so the parent–child relationship matches the Sessions tree pattern.
 */
function SkillProjectRow({ row }: SkillProjectRowProps) {
  const label = projectName(row.project) ?? row.project;
  return (
    <div
      className="breakdown-row breakdown-row-subagent breakdown-row-skill-project"
      role="listitem"
      aria-label={`${label}${row.hostname ? ` on ${row.hostname}` : ""}: ${row.total_count} skill uses`}
      style={{ paddingLeft: `${10 + SUBAGENT_INDENT_PX}px` }}
    >
      <span
        className="breakdown-name breakdown-name-subagent"
        title={row.project}
      >
        <span className="breakdown-tree-guide" aria-hidden>
          {"└─"}
        </span>
        {label}
        {row.hostname && (
          <span className="breakdown-host-tag">{row.hostname}</span>
        )}
      </span>
      <span className="breakdown-tokens">
        {row.total_count.toLocaleString()}
      </span>
      <span className="breakdown-turns">
        {row.total_count === 1 ? "use" : "uses"}
      </span>
      <span className="breakdown-time">
        {formatRelativeTime(row.last_used)}
      </span>
    </div>
  );
}

interface BreakdownPanelProps {
  days: number;
  selection: BreakdownSelection | null;
  onSelect: (selection: BreakdownSelection | null) => void;
}

function BreakdownPanel({ days, selection, onSelect }: BreakdownPanelProps) {
  const { toast } = useToast();
  const [mode, setMode] = useState<BreakdownMode>("sessions");
  const [skillsAllTime, setSkillsAllTime] = useState(true);
  const [skillsProvider, setSkillsProvider] = useState<SkillProviderFilter>("all");
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [deleting, setDeleting] = useState(false);
  const [renaming, setRenaming] = useState(false);
  const [editingCwd, setEditingCwd] = useState<string | null>(null);
  // Set of expanded session keys (`provider:session_id`). Stored as a
  // plain object keyed on `sessionRefKey` so toggling triggers a render
  // without cloning the whole map.
  const [expandedSessions, setExpandedSessions] = useState<Record<string, boolean>>({});
  // Set of expanded sub-agent ids inside the tree (depth >= 2). Today no
  // backend node has a non-null `parent_agent_id`, but the recursive
  // renderer is ready for it.
  const [expandedAgents, setExpandedAgents] = useState<Record<string, boolean>>({});
  // Set of expanded skill names. Stored as a plain object keyed on
  // `skill_name` (the breakdown's natural primary key) so toggling a
  // single skill never clones a large map.
  const [expandedSkills, setExpandedSkills] = useState<Record<string, boolean>>({});
  const { fetchTree: fetchSubagentTree, getState: getSubagentState } = useSessionSubagents();
  const { fetchProjects: fetchSkillProjects, stateFor: skillProjectsState } =
    useSkillProjects();
  const renameInputRef = useRef<HTMLInputElement | null>(null);
  const confirmTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const skillProviderArg: IntegrationProvider | null =
    skillsProvider === "all" ? null : skillsProvider;
  const { data, loading, error, refresh } = useBreakdownData(mode, days, {
    skillAllTime: skillsAllTime,
    skillProvider: skillProviderArg,
  });
  // Mirrors the cache key shape used by `useBreakdownData` so the
  // per-skill project drilldown invalidates on the exact same filter
  // axes (days / all-time / provider) that drive the parent rows.
  const skillRequestKey = `${mode}:${days}:${skillsAllTime}:${skillProviderArg ?? "all"}`;

  const handleModeChange = (m: BreakdownMode) => {
    setMode(m);
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

  const toggleSessionExpand = useCallback(
    (provider: IntegrationProvider, sessionId: string) => {
      const key = sessionRefKey({ provider, session_id: sessionId });
      const willOpen = !expandedSessions[key];
      if (willOpen) {
        // Lazy-fetch on first expand. The hook short-circuits if already
        // loaded or in-flight, so re-expanding never refetches. Fire the
        // fetch BEFORE the state update so React Strict Mode's double-
        // invoked updater can't trigger duplicate fetches.
        void fetchSubagentTree(provider, sessionId);
      }
      setExpandedSessions((prev) => {
        if (willOpen) {
          return { ...prev, [key]: true };
        }
        const { [key]: _omit, ...rest } = prev;
        return rest;
      });
    },
    [expandedSessions, fetchSubagentTree],
  );

  const toggleAgentExpand = useCallback((agentId: string) => {
    setExpandedAgents((prev) => {
      if (prev[agentId]) {
        const { [agentId]: _omit, ...rest } = prev;
        return rest;
      }
      return { ...prev, [agentId]: true };
    });
  }, []);

  // When the filter axes change (days / all-time / provider) the cached
  // sub-row data for the previous filter no longer applies, so collapse
  // all expanded skill rows. Re-expanding triggers a fresh fetch under
  // the new request key.
  useEffect(() => {
    setExpandedSkills({});
  }, [skillRequestKey]);

  const toggleSkillExpand = useCallback(
    (skillName: string) => {
      const willOpen = !expandedSkills[skillName];
      if (willOpen) {
        // Lazy-fetch on first expand. The hook short-circuits if already
        // loaded or in-flight under the current request key. Fire BEFORE
        // the state update so Strict Mode's double-invoked updater can't
        // trigger duplicate fetches.
        void fetchSkillProjects(skillName, skillRequestKey, {
          days,
          allTime: skillsAllTime,
          provider: skillProviderArg,
          limit: 50,
        });
      }
      setExpandedSkills((prev) => {
        if (willOpen) {
          return { ...prev, [skillName]: true };
        }
        const { [skillName]: _omit, ...rest } = prev;
        return rest;
      });
    },
    [
      expandedSkills,
      fetchSkillProjects,
      skillRequestKey,
      days,
      skillsAllTime,
      skillProviderArg,
    ],
  );

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
  const activeSelection = mode === "skills" ? null : selection;

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
        {activeSelection && (
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
          {activeSelection && activeSelection.type === "project" && editingCwd === null && (
            <button
              className="breakdown-rename-btn"
              onClick={handleRenameStart}
              disabled={renaming || deleting}
              aria-label={`Rename project: ${activeSelection.key}`}
              title="Rename project path"
            >
              <PencilIcon size={11} />
            </button>
          )}
          {activeSelection && (
            <button
              className={`breakdown-delete-btn${confirmDelete ? " confirm" : ""}`}
              onClick={handleDeleteClick}
              disabled={deleting || renaming}
              aria-label={
                confirmDelete
                  ? `Confirm delete ${activeSelection.type}: ${activeSelection.key}`
                  : `Delete ${activeSelection.type}: ${activeSelection.key}`
              }
              title={
                confirmDelete
                  ? "Click again to confirm"
                  : `Delete all data for this ${activeSelection.type}`
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

      {mode === "skills" && (
        <div className="breakdown-skill-controls" aria-label="Skill breakdown controls">
          <div className="breakdown-skill-provider-tabs" role="tablist" aria-label="Skill provider filter">
            {SKILL_PROVIDER_FILTERS.map((filter) => (
              <button
                key={filter.value}
                role="tab"
                className={`breakdown-skill-provider${skillsProvider === filter.value ? " active" : ""}`}
                aria-pressed={skillsProvider === filter.value}
                aria-selected={skillsProvider === filter.value}
                onClick={() => setSkillsProvider(filter.value)}
              >
                <span className="breakdown-skill-provider-label">{filter.label}</span>
                <span className="breakdown-skill-provider-underline" aria-hidden />
              </button>
            ))}
          </div>
          <button
            className={`breakdown-skill-toggle${skillsAllTime ? " active" : ""}`}
            aria-pressed={skillsAllTime}
            onClick={() => setSkillsAllTime((value) => !value)}
            title={skillsAllTime ? "Showing all history. Click to scope to the active timeframe." : "Click to ignore the timeframe and count every recorded skill use."}
          >
            <span className="breakdown-skill-toggle-glyph" aria-hidden>&#8734;</span>
            <span className="breakdown-skill-toggle-label">All time</span>
          </button>
        </div>
      )}

      {error && <div className="analytics-error">{error}</div>}

      {loading ? (
        <div className="breakdown-empty">{"Loading\u2026"}</div>
      ) : data.length === 0 ? (
        <div className="breakdown-empty">
          {mode === "skills"
            ? skillEmptyLabel(skillsProvider, skillsAllTime)
            : `No ${mode} data yet`}
        </div>
      ) : (
        <div
          className="breakdown-list"
          role="list"
          aria-label={`${MODE_LABELS[mode]} breakdown`}
        >
          {mode === "hosts"
            ? (data as HostBreakdown[]).map((row) => (
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
                  </span>
                  <span className="breakdown-time">
                    {formatRelativeTime(row.last_active)}
                  </span>
                </div>
              ))
            : mode === "projects"
              ? (data as ProjectBreakdown[]).map((row) => {
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
              : mode === "skills"
                ? (data as SkillBreakdown[]).map((row) => {
                    const hasProjects = row.project_count > 1;
                    const isExpanded = hasProjects && !!expandedSkills[row.skill_name];
                    const projectsState = hasProjects
                      ? skillProjectsState(row.skill_name, skillRequestKey)
                      : null;
                    return (
                      <Fragment key={row.skill_name}>
                        <div
                          className={`breakdown-row breakdown-row-skill${hasProjects ? " has-children" : ""}`}
                          role="listitem"
                          aria-expanded={hasProjects ? isExpanded : undefined}
                          aria-label={`${row.skill_name}: ${row.total_count} skill uses${hasProjects ? `, ${row.project_count} projects` : ""}`}
                        >
                          <span className="breakdown-name" title={row.skill_name}>
                            {hasProjects && (
                              <button
                                type="button"
                                className="breakdown-chevron breakdown-chevron-btn"
                                aria-label={isExpanded ? "Collapse projects" : "Expand projects"}
                                aria-expanded={isExpanded}
                                onClick={(e) => {
                                  e.stopPropagation();
                                  toggleSkillExpand(row.skill_name);
                                }}
                                onKeyDown={(e) => {
                                  if (e.key === "Enter" || e.key === " ") {
                                    e.stopPropagation();
                                  }
                                }}
                              >
                                <ChevronIcon open={isExpanded} />
                              </button>
                            )}
                            {row.skill_name}
                          </span>
                          <span className="breakdown-tokens">
                            {row.total_count.toLocaleString()}
                          </span>
                          <span className="breakdown-turns">
                            {row.total_count === 1 ? "use" : "uses"}
                          </span>
                          <span className="breakdown-time">
                            {formatRelativeTime(row.last_used)}
                          </span>
                        </div>
                        {isExpanded && projectsState && projectsState.status === "loading" && (
                          <div
                            className="breakdown-row breakdown-row-subagent breakdown-row-subagent-status"
                            role="listitem"
                            aria-live="polite"
                            style={{ paddingLeft: `${10 + SUBAGENT_INDENT_PX}px` }}
                          >
                            <span className="breakdown-name breakdown-subagent-status-text">
                              Loading projects…
                            </span>
                          </div>
                        )}
                        {isExpanded && projectsState && projectsState.status === "error" && (
                          <div
                            className="breakdown-row breakdown-row-subagent breakdown-row-subagent-status error"
                            role="listitem"
                            style={{ paddingLeft: `${10 + SUBAGENT_INDENT_PX}px` }}
                          >
                            <span className="breakdown-name breakdown-subagent-status-text">
                              {projectsState.message}
                            </span>
                          </div>
                        )}
                        {isExpanded &&
                          projectsState &&
                          projectsState.status === "loaded" &&
                          projectsState.rows.length === 0 && (
                            <div
                              className="breakdown-row breakdown-row-subagent breakdown-row-subagent-status"
                              role="listitem"
                              style={{ paddingLeft: `${10 + SUBAGENT_INDENT_PX}px` }}
                            >
                              <span className="breakdown-name breakdown-subagent-status-text">
                                No project data
                              </span>
                            </div>
                          )}
                        {isExpanded &&
                          projectsState &&
                          projectsState.status === "loaded" &&
                          projectsState.rows.map((projectRow) => (
                            <SkillProjectRow
                              key={`${projectRow.project}::${projectRow.hostname ?? ""}`}
                              row={projectRow}
                            />
                          ))}
                      </Fragment>
                    );
                  })
                : (data as SessionBreakdown[]).map((row) => {
                  const sessKey = sessionRefKey({
                    provider: row.provider,
                    session_id: row.session_id,
                  });
                  const hasSubagents = !!row.has_subagents;
                  const isExpanded = hasSubagents && !!expandedSessions[sessKey];
                  const subState = hasSubagents
                    ? getSubagentState(row.provider, row.session_id)
                    : null;
                  const groups = subState ? groupByParent(subState.nodes) : null;
                  return (
                    <SessionTreeBranch
                      key={sessKey}
                      row={row}
                      sessKey={sessKey}
                      hasSubagents={hasSubagents}
                      isExpanded={isExpanded}
                      isSelected={isSelected("session", sessKey)}
                      subState={subState}
                      groups={groups}
                      expandedAgents={expandedAgents}
                      onToggleSession={() =>
                        toggleSessionExpand(row.provider, row.session_id)
                      }
                      onRowClick={() => handleSessionRowClick(row)}
                      onToggleAgent={toggleAgentExpand}
                    />
                  );
                })}
        </div>
      )}
    </div>
  );
}

export default BreakdownPanel;
