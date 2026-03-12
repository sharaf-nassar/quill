import { useState, useEffect, useCallback, useRef, useMemo } from "react";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { LogicalSize } from "@tauri-apps/api/dpi";
import { check } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";
import TitleBar from "./components/TitleBar";
import UsageDisplay from "./components/UsageDisplay";
import AnalyticsView from "./components/analytics/AnalyticsView";
import LearningPanel from "./windows/LearningWindow";
import { useToast } from "./hooks/useToast";
import type { UsageData, TimeMode, PendingUpdate } from "./types";
import "./styles/learning.css";

const BASE_WIDTH = 260;
const BASE_HEIGHTS: Record<TimeMode, number> = {
  marker: 200,
  dual: 250,
  background: 200,
};
const TIME_MODE_KEY = "quill-time-mode";
const SHOW_LIVE_KEY = "quill-show-live";
const SHOW_ANALYTICS_KEY = "quill-show-analytics";
const SHOW_LEARNING_KEY = "quill-show-learning";
const SIZE_PREFIX = "quill-size-";
const SPLIT_RATIO_KEY = "quill-split-ratio";
const DEFAULT_SPLIT_RATIO = 0.4;
const MIN_SPLIT = 0.15;
const MAX_SPLIT = 0.85;

type LayoutKey = "live" | "analytics" | "both";

const DEFAULT_SIZES: Record<LayoutKey, { width: number; height: number }> = {
  live: { width: 280, height: 340 },
  analytics: { width: 520, height: 560 },
  both: { width: 520, height: 700 },
};

function layoutKey(live: boolean, analytics: boolean): LayoutKey | null {
  if (live && analytics) return "both";
  if (live) return "live";
  if (analytics) return "analytics";
  return null;
}

function loadSize(key: LayoutKey): { width: number; height: number } {
  try {
    const stored = localStorage.getItem(SIZE_PREFIX + key);
    if (stored) {
      const parsed = JSON.parse(stored) as { width: number; height: number };
      if (parsed.width > 0 && parsed.height > 0) return parsed;
    }
  } catch {
    /* ignore */
  }
  return DEFAULT_SIZES[key] ?? DEFAULT_SIZES.live;
}

function saveSize(key: LayoutKey, width: number, height: number): void {
  try {
    localStorage.setItem(SIZE_PREFIX + key, JSON.stringify({ width, height }));
  } catch {
    /* ignore */
  }
}

function loadBool(key: string, fallback: boolean): boolean {
  try {
    const stored = localStorage.getItem(key);
    if (stored === "true") return true;
    if (stored === "false") return false;
  } catch {
    /* ignore */
  }
  return fallback;
}

function loadSplitRatio(): number {
  try {
    const stored = localStorage.getItem(SPLIT_RATIO_KEY);
    if (stored) {
      const val = parseFloat(stored);
      if (val >= MIN_SPLIT && val <= MAX_SPLIT) return val;
    }
  } catch {
    /* ignore */
  }
  return DEFAULT_SPLIT_RATIO;
}

function loadTimeMode(): TimeMode {
  try {
    const stored = localStorage.getItem(TIME_MODE_KEY);
    if (stored === "marker" || stored === "dual" || stored === "background") {
      return stored;
    }
  } catch {
    /* ignore */
  }
  return "marker";
}

function App() {
  const { toast } = useToast();
  const [usageData, setUsageData] = useState<UsageData | null>(null);
  const [showMenu, setShowMenu] = useState(false);
  const [menuPos, setMenuPos] = useState({ x: 0, y: 0 });
  const [timeMode, setTimeMode] = useState<TimeMode>(loadTimeMode);
  const [showLive, setShowLive] = useState(() => loadBool(SHOW_LIVE_KEY, true));
  const [showAnalytics, setShowAnalytics] = useState(() =>
    loadBool(SHOW_ANALYTICS_KEY, false),
  );
  const [showLearning, setShowLearning] = useState(() =>
    loadBool(SHOW_LEARNING_KEY, false),
  );
  const [splitRatio, setSplitRatio] = useState(loadSplitRatio);
  const liveRef = useRef<HTMLDivElement>(null);
  const learningRef = useRef<HTMLDivElement>(null);
  const upperRef = useRef<HTMLDivElement>(null);
  const panelsRef = useRef<HTMLDivElement>(null);
  const splitRatioRef = useRef(splitRatio);
  const observerRef = useRef<ResizeObserver | null>(null);
  const showLiveRef = useRef(showLive);
  const showAnalyticsRef = useRef(showAnalytics);
  const currentLayoutRef = useRef<LayoutKey | null>(
    layoutKey(
      loadBool(SHOW_LIVE_KEY, true),
      loadBool(SHOW_ANALYTICS_KEY, false),
    ),
  );

  const saveCurrentSize = useCallback(async () => {
    const key = currentLayoutRef.current;
    if (!key) return;
    try {
      const size = await getCurrentWindow().innerSize();
      saveSize(key, Math.round(size.width), Math.round(size.height));
    } catch {
      /* ignore */
    }
  }, []);

  const handleClose = useCallback(async () => {
    await saveCurrentSize();
    await invoke("hide_window");
  }, [saveCurrentSize]);

  const switchLayout = useCallback(
    async (nextLive: boolean, nextAnalytics: boolean) => {
      const prevKey = currentLayoutRef.current;
      const nextKey = layoutKey(nextLive, nextAnalytics);

      let currentWidth: number | undefined;
      if (prevKey) {
        try {
          const size = await getCurrentWindow().innerSize();
          currentWidth = Math.round(size.width);
          saveSize(prevKey, currentWidth, Math.round(size.height));
        } catch {
          /* ignore */
        }
      }

      setShowLive(nextLive);
      setShowAnalytics(nextAnalytics);
      showLiveRef.current = nextLive;
      showAnalyticsRef.current = nextAnalytics;
      currentLayoutRef.current = nextKey;
      try {
        localStorage.setItem(SHOW_LIVE_KEY, String(nextLive));
      } catch {
        /* ignore */
      }
      try {
        localStorage.setItem(SHOW_ANALYTICS_KEY, String(nextAnalytics));
      } catch {
        /* ignore */
      }

      if (nextKey) {
        const saved = loadSize(nextKey);
        const width = currentWidth ?? saved.width;
        try {
          await getCurrentWindow().setSize(
            new LogicalSize(width, saved.height),
          );
        } catch {
          /* ignore */
        }
      }
    },
    [],
  );

  const handleToggleLive = useCallback(
    (on: boolean) => {
      switchLayout(on, showAnalyticsRef.current);
    },
    [switchLayout],
  );

  const handleToggleAnalytics = useCallback(
    (on: boolean) => {
      switchLayout(showLiveRef.current, on);
    },
    [switchLayout],
  );

  const handleTimeModeChange = (mode: TimeMode) => {
    setTimeMode(mode);
    try {
      localStorage.setItem(TIME_MODE_KEY, mode);
    } catch {
      /* ignore */
    }
  };

  const handleToggleLearning = useCallback(() => {
    const next = !showLearning;
    setShowLearning(next);
    try {
      localStorage.setItem(SHOW_LEARNING_KEY, String(next));
    } catch {
      /* ignore */
    }
  }, [showLearning]);

  const isSplit = showLive && showAnalytics;

  const handleDividerMouseDown = useCallback(
    (e: React.MouseEvent<HTMLDivElement>) => {
      e.preventDefault();
      const liveEl = liveRef.current;
      const containerEl = upperRef.current;
      if (!liveEl || !containerEl) return;

      // Freeze inner content at current pixel sizes so children skip layout
      const liveInner = liveEl.querySelector(
        ".usage-display",
      ) as HTMLElement | null;
      const analyticsInner = containerEl.querySelector(
        ".analytics-view",
      ) as HTMLElement | null;
      if (liveInner) {
        liveInner.style.height = `${liveInner.offsetHeight}px`;
        liveInner.style.overflow = "hidden";
        liveInner.style.flex = "none";
      }
      if (analyticsInner) {
        analyticsInner.style.height = `${analyticsInner.offsetHeight}px`;
        analyticsInner.style.overflow = "hidden";
        analyticsInner.style.flex = "none";
      }

      // Pause the live panel's ResizeObserver (stops --s cascade)
      observerRef.current?.disconnect();

      // Add drag classes directly on DOM — no React re-renders
      document.documentElement.classList.add("dragging-divider");
      (e.currentTarget as HTMLElement).classList.add("active");

      let rafId = 0;

      const onMouseMove = (ev: MouseEvent) => {
        cancelAnimationFrame(rafId);
        const clientY = ev.clientY;
        rafId = requestAnimationFrame(() => {
          const rect = containerEl.getBoundingClientRect();
          const ratio = Math.max(
            MIN_SPLIT,
            Math.min(MAX_SPLIT, (clientY - rect.top) / rect.height),
          );
          splitRatioRef.current = ratio;
          liveEl.style.flex = `0 0 ${ratio * 100}%`;
        });
      };

      const onMouseUp = () => {
        cancelAnimationFrame(rafId);
        document.documentElement.classList.remove("dragging-divider");
        document.removeEventListener("mousemove", onMouseMove);
        document.removeEventListener("mouseup", onMouseUp);

        // Unfreeze inner content — let flex/auto sizing resume
        if (liveInner) {
          liveInner.style.height = "";
          liveInner.style.overflow = "";
          liveInner.style.flex = "";
        }
        if (analyticsInner) {
          analyticsInner.style.height = "";
          analyticsInner.style.overflow = "";
          analyticsInner.style.flex = "";
        }

        // Reconnect observer — fires once with final size for --s update
        if (observerRef.current && liveRef.current) {
          observerRef.current.observe(liveRef.current);
        }

        // Sync final ratio into React state once
        setSplitRatio(splitRatioRef.current);
        try {
          localStorage.setItem(SPLIT_RATIO_KEY, String(splitRatioRef.current));
        } catch {
          /* ignore */
        }
      };

      document.addEventListener("mousemove", onMouseMove);
      document.addEventListener("mouseup", onMouseUp);
    },
    [],
  );

  const handleDividerKeyDown = useCallback(
    (e: React.KeyboardEvent<HTMLDivElement>) => {
      const step = 0.02;
      let delta = 0;
      if (e.key === "ArrowUp") delta = -step;
      else if (e.key === "ArrowDown") delta = step;
      else return;

      e.preventDefault();
      const next = Math.max(
        MIN_SPLIT,
        Math.min(MAX_SPLIT, splitRatioRef.current + delta),
      );
      splitRatioRef.current = next;
      setSplitRatio(next);
      if (liveRef.current) {
        liveRef.current.style.flex = `0 0 ${next * 100}%`;
      }
      try {
        localStorage.setItem(SPLIT_RATIO_KEY, String(next));
      } catch {
        /* ignore */
      }
    },
    [],
  );

  const handleLearningDividerMouseDown = useCallback(
    (e: React.MouseEvent<HTMLDivElement>) => {
      e.preventDefault();
      const panelsEl = panelsRef.current;
      const learningEl = learningRef.current;
      const upperEl = upperRef.current;
      if (!panelsEl || !learningEl || !upperEl) return;

      document.documentElement.classList.add("dragging-divider");
      (e.currentTarget as HTMLElement).classList.add("active");

      let rafId = 0;
      const dividerHeight = 9; // matches .panel-divider height

      const onMouseMove = (ev: MouseEvent) => {
        cancelAnimationFrame(rafId);
        const clientY = ev.clientY;
        rafId = requestAnimationFrame(() => {
          const rect = panelsEl.getBoundingClientRect();
          const available = rect.height - dividerHeight;
          const upperPx = Math.max(80, Math.min(clientY - rect.top, available - 80));
          const lowerPx = available - upperPx;
          upperEl.style.flex = `0 0 ${upperPx}px`;
          learningEl.style.flex = `0 0 ${lowerPx}px`;
        });
      };

      const onMouseUp = () => {
        cancelAnimationFrame(rafId);
        document.documentElement.classList.remove("dragging-divider");
        document.removeEventListener("mousemove", onMouseMove);
        document.removeEventListener("mouseup", onMouseUp);
      };

      document.addEventListener("mousemove", onMouseMove);
      document.addEventListener("mouseup", onMouseUp);
    },
    [],
  );

  const refresh = useCallback(async () => {
    try {
      const data = await invoke<UsageData>("fetch_usage_data");
      setUsageData(data);
    } catch (e) {
      toast("error", `Usage data fetch failed: ${e}`);
      setUsageData({ buckets: [], error: String(e) });
    }
  }, [toast]);

  useEffect(() => {
    refresh();
    const interval = setInterval(refresh, 3 * 60_000);
    return () => clearInterval(interval);
  }, [refresh]);

  // Check for app updates on startup and every 4 hours
  const [pendingUpdate, setPendingUpdate] = useState<PendingUpdate | null>(
    null,
  );
  const [updating, setUpdating] = useState(false);

  const checkForUpdate = useCallback(() => {
    check()
      .then((update) => {
        if (update) {
          console.log(`Update available: ${update.version}`);
          setPendingUpdate(update);
        }
      })
      .catch((e) => console.log("Update check skipped:", e));
  }, []);

  useEffect(() => {
    if (import.meta.env.DEV) return;
    checkForUpdate();
    const interval = setInterval(checkForUpdate, 4 * 60 * 60_000);
    return () => clearInterval(interval);
  }, [checkForUpdate]);

  const handleUpdate = useCallback(async () => {
    if (!pendingUpdate || updating) return;
    setUpdating(true);
    try {
      await pendingUpdate.downloadAndInstall();
      await relaunch();
    } catch (e) {
      toast("error", `Update failed: ${e}`);
      setUpdating(false);
    }
  }, [pendingUpdate, updating, toast]);

  // Release stuck scrollbar drag state during OS window resize.
  // WebKit can leave the scrollbar thumb in a dragging state when the
  // window is resized via the OS chrome (bottom-right corner), because
  // the mouseup from the resize never reaches the webview.
  useEffect(() => {
    const onResize = () => {
      document.dispatchEvent(new MouseEvent("mouseup", { bubbles: true }));
    };
    window.addEventListener("resize", onResize);
    return () => window.removeEventListener("resize", onResize);
  }, []);

  // Intercept OS-level close (Alt+F4, etc.) to hide instead of quit
  useEffect(() => {
    const unlistenPromise = getCurrentWindow().onCloseRequested(
      async (event) => {
        event.preventDefault();
        await saveCurrentSize();
        await invoke("hide_window");
      },
    );
    return () => {
      unlistenPromise.then((fn) => fn());
    };
  }, [saveCurrentSize]);

  useEffect(() => {
    if (!showLive) return;

    const el = liveRef.current;
    if (!el) return;

    const baseH = BASE_HEIGHTS[timeMode] ?? 200;
    let rafId = 0;
    let lastScale = -1;

    const updateScale = () => {
      cancelAnimationFrame(rafId);
      rafId = requestAnimationFrame(() => {
        const w = el.clientWidth;
        const h = el.clientHeight;
        if (w <= 0 || h <= 0) return;
        const wScale = w / BASE_WIDTH;
        const hScale = h / baseH;
        const scale =
          Math.round(Math.max(0.6, Math.min(wScale, hScale, 2.5)) * 100) / 100;
        if (scale !== lastScale) {
          lastScale = scale;
          el.style.setProperty("--s", String(scale));
        }
      });
    };

    const observer = new ResizeObserver(updateScale);
    observerRef.current = observer;
    observer.observe(el);
    updateScale();
    return () => {
      observer.disconnect();
      observerRef.current = null;
      cancelAnimationFrame(rafId);
    };
  }, [timeMode, showLive]);

  const handleContextMenu = (e: React.MouseEvent) => {
    e.preventDefault();
    const menuWidth = 100;
    const menuHeight = 70;
    const x = Math.min(e.clientX, window.innerWidth - menuWidth);
    const y = Math.min(e.clientY, window.innerHeight - menuHeight);
    setMenuPos({ x, y });
    setShowMenu(true);
  };

  const closeMenu = () => setShowMenu(false);

  const handleQuit = async () => {
    closeMenu();
    await saveCurrentSize();
    await invoke("quit_app");
  };

  const handleRefresh = () => {
    closeMenu();
    refresh();
  };

  const liveStyle = useMemo(
    () => (isSplit ? { flex: `0 0 ${splitRatio * 100}%` } : undefined),
    [isSplit, splitRatio],
  );

  return (
    <div className="app" onContextMenu={handleContextMenu} onClick={closeMenu}>
      <TitleBar
        showLive={showLive}
        showAnalytics={showAnalytics}
        showLearning={showLearning}
        onToggleLive={handleToggleLive}
        onToggleAnalytics={handleToggleAnalytics}
        onToggleLearning={handleToggleLearning}
        onClose={handleClose}
        pendingUpdate={pendingUpdate}
        updating={updating}
        onUpdate={handleUpdate}
      />
      <div
        className={`panels${isSplit ? " panels--split" : ""}${showLearning ? " panels--learning" : ""}${showLive && showAnalytics && showLearning ? " panels--triple" : ""}`}
        ref={panelsRef}
      >
        {(showLive || showAnalytics) && (
          <div className="upper-panels" ref={upperRef}>
            {showLive && (
              <div className="content live-content" ref={liveRef} style={liveStyle}>
                <UsageDisplay
                  data={usageData}
                  timeMode={timeMode}
                  onTimeModeChange={handleTimeModeChange}
                />
              </div>
            )}
            {isSplit && (
              <div
                className="panel-divider"
                role="separator"
                aria-orientation="horizontal"
                aria-label="Resize panels"
                tabIndex={0}
                onMouseDown={handleDividerMouseDown}
                onKeyDown={handleDividerKeyDown}
              />
            )}
            {showAnalytics && (
              <div className="content analytics-content">
                <AnalyticsView currentBuckets={usageData?.buckets ?? []} />
              </div>
            )}
          </div>
        )}
        {showLearning && (showLive || showAnalytics) && (
          <div
            className="panel-divider"
            role="separator"
            aria-orientation="horizontal"
            aria-label="Resize learning panel"
            tabIndex={0}
            onMouseDown={handleLearningDividerMouseDown}
          />
        )}
        {showLearning && (
          <div className="content learning-content-wrap" ref={learningRef}>
            <LearningPanel />
          </div>
        )}
        {!showLive && !showAnalytics && !showLearning && (
          <div className="content">
            <div className="loading">Toggle a view from the titlebar</div>
          </div>
        )}
      </div>
      {showMenu && (
        <div
          className="context-menu"
          style={{ left: menuPos.x, top: menuPos.y }}
          onClick={(e) => e.stopPropagation()}
        >
          <button className="context-menu-item" onClick={handleRefresh}>
            Refresh
          </button>
          <button className="context-menu-item" onClick={handleQuit}>
            Quit
          </button>
        </div>
      )}
    </div>
  );
}

export default App;
