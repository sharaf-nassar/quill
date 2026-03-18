import { useState, useEffect, useCallback, useRef, useMemo } from "react";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { check } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";
import TitleBar from "./components/TitleBar";
import UsageDisplay from "./components/UsageDisplay";
import AnalyticsView from "./components/analytics/AnalyticsView";
import TilingContainer from "./components/tiling/TilingContainer";
import {
	collectPanels,
	hasPanel,
	removePanel,
	smartInsert,
	saveLastLayout,
	loadLastLayout,
	savePositionMemory,
	loadAllPositionMemory,
	isSnapshotCompatible,
	pruneUnknownPanels,
	removeLegacyKeys,
} from "./components/tiling/layoutEngine";
import type { LayoutNode, SectionId } from "./components/tiling/types";
import { useToast } from "./hooks/useToast";
import type { UsageData, TimeMode, PendingUpdate } from "./types";

const TIME_MODE_KEY = "quill-time-mode";
const SHOW_LIVE_KEY = "quill-show-live";
const SHOW_ANALYTICS_KEY = "quill-show-analytics";
const KNOWN_PANELS = new Set<SectionId>(["live", "analytics"]);

function loadBool(key: string, fallback: boolean): boolean {
	try {
		const stored = localStorage.getItem(key);
		if (stored === "true") return true;
		if (stored === "false") return false;
	} catch { /* ignore */ }
	return fallback;
}

function loadTimeMode(): TimeMode {
	try {
		const stored = localStorage.getItem(TIME_MODE_KEY);
		if (stored === "marker" || stored === "dual" || stored === "background") return stored;
	} catch { /* ignore */ }
	return "marker";
}

function buildInitialLayout(): LayoutNode | null {
	const saved = loadLastLayout();
	if (saved) {
		const pruned = pruneUnknownPanels(saved.tree, KNOWN_PANELS);
		if (pruned) return pruned;
	}

	const panels: SectionId[] = [];
	if (loadBool(SHOW_LIVE_KEY, true)) panels.push("live");
	if (loadBool(SHOW_ANALYTICS_KEY, false)) panels.push("analytics");

	if (panels.length === 0) return null;
	if (panels.length === 1) return { type: "leaf", panelId: panels[0] };

	return panels.reduceRight<LayoutNode>((acc, panelId, i) => {
		if (i === panels.length - 1) return { type: "leaf", panelId };
		return {
			type: "split",
			direction: "vertical",
			ratio: 1 / (panels.length - i),
			children: [{ type: "leaf", panelId }, acc],
		};
	}, { type: "leaf", panelId: panels[panels.length - 1] });
}

function App() {
	const { toast } = useToast();
	const [usageData, setUsageData] = useState<UsageData | null>(null);
	const [showMenu, setShowMenu] = useState(false);
	const [menuPos, setMenuPos] = useState({ x: 0, y: 0 });
	const [timeMode, setTimeMode] = useState<TimeMode>(loadTimeMode);
	const [layoutTree, setLayoutTree] = useState<LayoutNode | null>(buildInitialLayout);
	const containerRef = useRef<HTMLDivElement>(null);

	const visiblePanels = useMemo<SectionId[]>(() => {
		if (!layoutTree) return [];
		return collectPanels(layoutTree);
	}, [layoutTree]);

	const showLive = visiblePanels.includes("live");
	const showAnalytics = visiblePanels.includes("analytics");

	useEffect(() => {
		removeLegacyKeys();
	}, []);

	useEffect(() => {
		if (layoutTree) {
			saveLastLayout(layoutTree, visiblePanels);
		}
		try {
			localStorage.setItem(SHOW_LIVE_KEY, String(showLive));
			localStorage.setItem(SHOW_ANALYTICS_KEY, String(showAnalytics));
		} catch { /* ignore */ }
	}, [layoutTree, visiblePanels, showLive, showAnalytics]);

	const updateLayout = useCallback((newTree: LayoutNode) => {
		setLayoutTree(newTree);
	}, []);

	const handleToggleSection = useCallback(
		(sectionId: SectionId) => {
			if (layoutTree && hasPanel(layoutTree, sectionId)) {
				savePositionMemory(sectionId, layoutTree, visiblePanels);
				const newTree = removePanel(layoutTree, sectionId);
				setLayoutTree(newTree);
			} else {
				const memory = loadAllPositionMemory();
				const entry = memory[sectionId];

				if (entry && isSnapshotCompatible(entry.visiblePanels, visiblePanels, sectionId)) {
					const pruned = pruneUnknownPanels(entry.snapshot, KNOWN_PANELS);
					if (pruned) {
						setLayoutTree(pruned);
						return;
					}
				}

				if (!layoutTree) {
					setLayoutTree({ type: "leaf", panelId: sectionId });
				} else {
					const el = containerRef.current;
					const w = el?.clientWidth ?? 500;
					const h = el?.clientHeight ?? 500;
					setLayoutTree(smartInsert(layoutTree, sectionId, w, h));
				}
			}
		},
		[layoutTree, visiblePanels],
	);

	const handleApplyPreset = useCallback(
		(tree: LayoutNode) => {
			setLayoutTree(tree);
		},
		[],
	);

	const handleTimeModeChange = useCallback((mode: TimeMode) => {
		setTimeMode(mode);
		try { localStorage.setItem(TIME_MODE_KEY, mode); } catch { /* ignore */ }
	}, []);

	const handleClose = useCallback(async () => {
		await invoke("hide_window");
	}, []);

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

	const [pendingUpdate, setPendingUpdate] = useState<PendingUpdate | null>(null);
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

	useEffect(() => {
		const onResize = () => {
			document.dispatchEvent(new MouseEvent("mouseup", { bubbles: true }));
		};
		window.addEventListener("resize", onResize);
		return () => window.removeEventListener("resize", onResize);
	}, []);

	useEffect(() => {
		const unlistenPromise = getCurrentWindow().onCloseRequested(async (event) => {
			event.preventDefault();
			await invoke("hide_window");
		});
		return () => { unlistenPromise.then((fn) => fn()); };
	}, []);

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
		await invoke("quit_app");
	};

	const handleRefresh = () => {
		closeMenu();
		refresh();
	};

	const livePanel = useMemo(
		() => (
			<UsageDisplay
				data={usageData}
				timeMode={timeMode}
				onTimeModeChange={handleTimeModeChange}
			/>
		),
		[usageData, timeMode, handleTimeModeChange],
	);

	const analyticsPanel = useMemo(
		() => <AnalyticsView currentBuckets={usageData?.buckets ?? []} />,
		[usageData],
	);

	const panelMap = useMemo(
		() => ({
			live: livePanel,
			analytics: analyticsPanel,
		}),
		[livePanel, analyticsPanel],
	);

	return (
		<div className="app" onContextMenu={handleContextMenu} onClick={closeMenu}>
			<TitleBar
				showLive={showLive}
				showAnalytics={showAnalytics}
				onToggleSection={handleToggleSection}
				onClose={handleClose}
				pendingUpdate={pendingUpdate}
				updating={updating}
				onUpdate={handleUpdate}
				layout={layoutTree}
				visiblePanels={visiblePanels}
				onApplyPreset={handleApplyPreset}
			/>
			<div className="tiling-panels" ref={containerRef}>
				{layoutTree ? (
					<TilingContainer
						layout={layoutTree}
						panels={panelMap}
						onLayoutChange={updateLayout}
						timeMode={timeMode}
					/>
				) : (
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
