import { useState, useCallback, useEffect, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useToast } from "./useToast";
import type { IntegrationProvider, ProviderFilter } from "../types";

export interface MemoryFile {
  id: number;
  project_path: string;
  provider: IntegrationProvider;
  file_path: string;
  file_name: string;
  content_hash: string;
  last_scanned_at: string;
  memory_type: string | null;
  description: string | null;
  content: string;
  changed_since_last_run: boolean;
}

export interface OptimizationSuggestion {
  id: number;
  run_id: number;
  project_path: string;
  action_type: string;
  target_file: string | null;
  reasoning: string;
  proposed_content: string | null;
  merge_sources: string[] | null;
  status: string;
  error: string | null;
  resolved_at: string | null;
  created_at: string;
  original_content: string | null;
  diff_summary: string | null;
  backup_data: string | null;
  group_id: string | null;
  provider_scope: IntegrationProvider[];
}

export interface OptimizationRun {
  id: number;
  project_path: string;
  trigger: string;
  memories_scanned: number;
  suggestions_created: number;
  status: string;
  error: string | null;
  started_at: string;
  completed_at: string | null;
  provider_scope: IntegrationProvider[];
}

export interface KnownProject {
  path: string;
  name: string;
  has_memories: boolean;
  memory_count: number;
  is_custom: boolean;
  providers: IntegrationProvider[];
}

function filterToProvider(
  providerFilter: ProviderFilter,
): IntegrationProvider | null {
  return providerFilter === "all" ? null : providerFilter;
}

interface RefreshOptions {
  showLoading?: boolean;
}

interface DeleteProjectMemoriesOptions {
  refreshAfter?: boolean;
  showToast?: boolean;
}

export function useMemoryData(providerFilter: ProviderFilter = "all") {
  const { toast } = useToast();
  const [projects, setProjects] = useState<KnownProject[]>([]);
  const [selectedProject, setSelectedProject] = useState<string>("");
  const selectedProjectRef = useRef(selectedProject);
  selectedProjectRef.current = selectedProject;
  const [memoryFiles, setMemoryFiles] = useState<MemoryFile[]>([]);
  const [suggestions, setSuggestions] = useState<OptimizationSuggestion[]>([]);
  const [runs, setRuns] = useState<OptimizationRun[]>([]);
  const [activeRunIds, setActiveRunIds] = useState<Set<number>>(new Set());
  const activeRunIdsRef = useRef(activeRunIds);
  activeRunIdsRef.current = activeRunIds;
  const [startingOptimization, setStartingOptimization] = useState(false);
  const startingOptimizationRef = useRef(startingOptimization);
  startingOptimizationRef.current = startingOptimization;
  const [loading, setLoading] = useState(true);
  const [logs, setLogs] = useState<string[]>([]);
  const optimizing =
    startingOptimization ||
    activeRunIds.size > 0 ||
    runs.some((run) => run.status === "running");

  const updateActiveRunIds = useCallback(
    (updater: (prev: Set<number>) => Set<number>) => {
      setActiveRunIds((prev) => {
        const next = updater(prev);
        activeRunIdsRef.current = next;
        return next;
      });
    },
    [],
  );

  const loadProjects = useCallback(async () => {
    try {
      const p = await invoke<KnownProject[]>("get_known_projects", {
        provider: filterToProvider(providerFilter),
      });
      setProjects(p);
      if (!selectedProjectRef.current && p.length > 0) {
        setSelectedProject(p[0].path);
      }
    } catch (e) {
      toast("error", `Failed to load projects: ${e}`);
    }
  }, [providerFilter, toast]);

  const loadProjectData = useCallback(async (
    projectPath: string,
    { showLoading = true }: RefreshOptions = {},
  ) => {
    if (!projectPath) return;
    if (showLoading) {
      setLoading(true);
    }
    try {
      const provider = filterToProvider(providerFilter);
      const [files, sugs, r] = await Promise.all([
        invoke<MemoryFile[]>("get_memory_files", { projectPath, provider }),
        invoke<OptimizationSuggestion[]>("get_optimization_suggestions", {
          projectPath,
          provider,
          statusFilter: null,
          limit: 200,
          offset: 0,
        }),
        invoke<OptimizationRun[]>("get_optimization_runs", {
          projectPath,
          provider,
          limit: 10,
        }),
      ]);
      setMemoryFiles(files);
      setSuggestions(sugs);
      setRuns(r);
    } catch (e) {
      toast("error", `Failed to load project data: ${e}`);
    } finally {
      if (showLoading) {
        setLoading(false);
      }
    }
  }, [providerFilter, toast]);

  const projectsRef = useRef(projects);
  projectsRef.current = projects;

  const loadAllProjectData = useCallback(async (
    { showLoading = true }: RefreshOptions = {},
  ) => {
    if (showLoading) {
      setLoading(true);
    }
    try {
      const provider = filterToProvider(providerFilter);
      const withMemories = projectsRef.current.filter((p) => p.memory_count > 0);
      const allFiles = await Promise.all(
        withMemories.map((p) =>
          invoke<MemoryFile[]>("get_memory_files", { projectPath: p.path, provider }),
        ),
      );
      setMemoryFiles(allFiles.flat());
      setSuggestions([]);
      setRuns([]);
    } catch (e) {
      toast("error", `Failed to load all project data: ${e}`);
    } finally {
      if (showLoading) {
        setLoading(false);
      }
    }
  }, [providerFilter, toast]);

  const refresh = useCallback((options?: RefreshOptions) => {
    if (selectedProject === "__all__") {
      return loadAllProjectData(options);
    } else if (selectedProject) {
      return loadProjectData(selectedProject, options);
    }
    return Promise.resolve();
  }, [selectedProject, loadProjectData, loadAllProjectData]);

  // Load projects on mount
  useEffect(() => {
    loadProjects();
  }, [loadProjects]);

  // Load project data when selection changes
  useEffect(() => {
    if (selectedProject === "__all__") {
      loadAllProjectData();
    } else if (selectedProject) {
      loadProjectData(selectedProject);
    }
  }, [selectedProject, loadProjectData, loadAllProjectData]);

  // Listen for events
  useEffect(() => {
    const unlistenUpdated = listen<{ run_id: number; status: string }>(
      "memory-optimizer-updated",
      (event) => {
        const { run_id, status } = event.payload;
        if (status === "running") {
          updateActiveRunIds((prev) => {
            if (prev.has(run_id)) return prev;
            const next = new Set(prev);
            next.add(run_id);
            return next;
          });
          return;
        }

        const remainingActiveRuns = new Set(activeRunIdsRef.current);
        remainingActiveRuns.delete(run_id);
        updateActiveRunIds(() => remainingActiveRuns);

        if (status !== "running") {
          if (selectedProjectRef.current !== "__all__") {
            void refresh({ showLoading: false });
          }
          if (!startingOptimizationRef.current && remainingActiveRuns.size === 0) {
            setLogs([]);
          }
        }
      },
    );

    const unlistenLog = listen<{ message: string }>(
      "memory-optimizer-log",
      (event) => {
        setLogs((prev) => [...prev, event.payload.message]);
      },
    );

    const unlistenFiles = listen<{ project_path: string }>(
      "memory-files-updated",
      () => {
        void refresh({ showLoading: false });
      },
    );

    return () => {
      unlistenUpdated.then((fn) => fn());
      unlistenLog.then((fn) => fn());
      unlistenFiles.then((fn) => fn());
    };
  }, [refresh, updateActiveRunIds]);

  const triggerOptimization = useCallback(async () => {
    if (!selectedProject || optimizing) return;
    setStartingOptimization(true);
    setLogs([]);
    try {
      const runId = await invoke<number>("trigger_memory_optimization", {
        projectPath: selectedProject,
        provider: filterToProvider(providerFilter),
      });
      updateActiveRunIds((prev) => {
        if (prev.has(runId)) return prev;
        const next = new Set(prev);
        next.add(runId);
        return next;
      });
    } catch (e) {
      toast("warning", String(e));
    } finally {
      setStartingOptimization(false);
    }
  }, [providerFilter, selectedProject, optimizing, toast, updateActiveRunIds]);

  const approveSuggestion = useCallback(
    async (id: number) => {
      try {
        await invoke("approve_suggestion", { suggestionId: id });
        refresh();
      } catch (e) {
        toast("error", `Failed to approve: ${e}`);
      }
    },
    [refresh, toast],
  );

  const denySuggestion = useCallback(
    async (id: number) => {
      try {
        await invoke("deny_suggestion", { suggestionId: id });
        refresh();
      } catch (e) {
        toast("error", `Failed to deny: ${e}`);
      }
    },
    [refresh, toast],
  );

  const undenySuggestion = useCallback(
    async (id: number) => {
      try {
        await invoke("undeny_suggestion", { suggestionId: id });
        refresh();
      } catch (e) {
        toast("error", `Failed to undeny: ${e}`);
      }
    },
    [refresh, toast],
  );

  const undoSuggestion = useCallback(
    async (id: number) => {
      try {
        await invoke("undo_suggestion", { suggestionId: id });
        refresh();
      } catch (e) {
        toast("error", `Failed to undo: ${e}`);
      }
    },
    [refresh, toast],
  );

  const approveGroup = useCallback(
    async (groupId: string) => {
      try {
        await invoke("approve_suggestion_group", { groupId });
        refresh();
      } catch (e) {
        toast("error", `Failed to approve group: ${e}`);
      }
    },
    [refresh, toast],
  );

  const denyGroup = useCallback(
    async (groupId: string) => {
      try {
        await invoke("deny_suggestion_group", { groupId });
        refresh();
      } catch (e) {
        toast("error", `Failed to deny group: ${e}`);
      }
    },
    [refresh, toast],
  );

  const addCustomProject = useCallback(
    async (path: string) => {
      try {
        await invoke("add_custom_project", { path });
        await loadProjects();
      } catch (e) {
        toast("error", `Failed to add project: ${e}`);
      }
    },
    [loadProjects, toast],
  );

  const removeCustomProject = useCallback(
    async (path: string) => {
      try {
        await invoke("remove_custom_project", { path });
        await loadProjects();
      } catch (e) {
        toast("error", `Failed to remove project: ${e}`);
      }
    },
    [loadProjects, toast],
  );

  const deleteMemoryFile = useCallback(
    async (filePath: string, projectPath?: string) => {
      const proj = projectPath ?? selectedProjectRef.current;
      if (!proj) return;
      try {
        await invoke("delete_memory_file", {
          projectPath: proj,
          filePath,
        });
        await Promise.all([loadProjects(), refresh()]);
      } catch (e) {
        toast("error", `Failed to delete memory: ${e}`);
      }
    },
    [loadProjects, refresh, toast],
  );

  const deleteProjectMemories = useCallback(
    async (
      projectPath: string,
      { refreshAfter = true, showToast = true }: DeleteProjectMemoriesOptions = {},
    ) => {
      try {
        const count = await invoke<number>("delete_project_memories", {
          projectPath,
        });
        if (showToast) {
          toast("info", `Deleted ${count} memory file(s)`);
        }
        if (refreshAfter) {
          await Promise.all([loadProjects(), loadProjectData(projectPath)]);
        }
        return count;
      } catch (e) {
        if (showToast) {
          toast("error", `Failed to delete memories: ${e}`);
        }
        throw e;
      }
    },
    [loadProjects, loadProjectData, toast],
  );

  const deleteCurrentViewMemories = useCallback(async () => {
    const currentSelection = selectedProjectRef.current;
    if (!currentSelection) {
      return;
    }

    if (currentSelection !== "__all__") {
      await deleteProjectMemories(currentSelection);
      return;
    }

    const targetProjects = projectsRef.current.filter((project) => project.memory_count > 0);
    if (targetProjects.length === 0) {
      toast("info", "No memories to delete");
      return;
    }

    const results = await Promise.allSettled(
      targetProjects.map((project) =>
        deleteProjectMemories(project.path, { refreshAfter: false, showToast: false }),
      ),
    );

    const deletedCount = results.reduce(
      (sum, result) => (result.status === "fulfilled" ? sum + result.value : sum),
      0,
    );
    const failures = results.filter((result) => result.status === "rejected");

    await Promise.all([loadProjects(), loadAllProjectData()]);

    if (failures.length === 0) {
      toast("info", `Deleted ${deletedCount} memory file(s)`);
      return;
    }

    if (deletedCount > 0) {
      toast(
        "warning",
        `Deleted ${deletedCount} memory file(s); ${failures.length} project(s) failed`,
      );
      return;
    }

    const firstFailure = failures[0];
    toast(
      "error",
      `Failed to delete memories: ${
        firstFailure?.status === "rejected" ? String(firstFailure.reason) : "Unknown error"
      }`,
    );
  }, [deleteProjectMemories, loadAllProjectData, loadProjects, toast]);

  const triggerOptimizeAll = useCallback(
    async () => {
      if (optimizing) return;
      const withMemories = projects.filter((p) => p.has_memories);
      if (withMemories.length === 0) {
        toast("info", "No projects with memories to optimize");
        return;
      }
      setStartingOptimization(true);
      setLogs([]);
      const BATCH_SIZE = 3;
      try {
        for (let i = 0; i < withMemories.length; i += BATCH_SIZE) {
          const batch = withMemories.slice(i, i + BATCH_SIZE);
          const results = await Promise.allSettled(
            batch.map((p) =>
              invoke<number>("trigger_memory_optimization", {
                projectPath: p.path,
                provider: filterToProvider(providerFilter),
              }),
            ),
          );
          const startedRunIds = results.flatMap((result) =>
            result.status === "fulfilled" ? [result.value] : [],
          );
          if (startedRunIds.length > 0) {
            updateActiveRunIds((prev) => {
              const next = new Set(prev);
              for (const runId of startedRunIds) {
                next.add(runId);
              }
              return next;
            });
          }

          const firstFailure = results.find((result) => result.status === "rejected");
          if (firstFailure?.status === "rejected") {
            toast("warning", String(firstFailure.reason));
          }
        }
      } catch (e) {
        toast("warning", String(e));
      } finally {
        setStartingOptimization(false);
      }
    },
    [projects, providerFilter, optimizing, toast, updateActiveRunIds],
  );

  return {
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
    approveSuggestion,
    denySuggestion,
    undenySuggestion,
    undoSuggestion,
    approveGroup,
    denyGroup,
    addCustomProject,
    removeCustomProject,
    deleteMemoryFile,
    deleteProjectMemories,
    deleteCurrentViewMemories,
    triggerOptimizeAll,
    refresh,
  };
}
