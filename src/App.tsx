import { useState, useEffect, useCallback, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { LogicalSize } from "@tauri-apps/api/dpi";
import { check } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";
import TitleBar from "./components/TitleBar";
import UsageDisplay from "./components/UsageDisplay";
import AnalyticsView from "./components/analytics/AnalyticsView";
import type { UseIntegrationsResult } from "./hooks/useIntegrations";
import { useToast } from "./hooks/useToast";
import type { UsageData, TimeMode, PendingUpdate } from "./types";

const BASE_WIDTH = 260;
const MIN_LIVE_SCALE = 0.35;
const LIVE_FIT_GUTTER_PX = 14;
const BASE_HEIGHTS: Record<TimeMode, number> = {
	marker: 200,
	dual: 250,
	background: 200,
};
const TIME_MODE_KEY = "quill-time-mode";
const SHOW_LIVE_KEY = "quill-show-live";
const SHOW_ANALYTICS_KEY = "quill-show-analytics";
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
	} catch { /* ignore */ }
	return DEFAULT_SIZES[key] ?? DEFAULT_SIZES.live;
}

function saveSize(key: LayoutKey, width: number, height: number): void {
	try {
		localStorage.setItem(SIZE_PREFIX + key, JSON.stringify({ width, height }));
	} catch { /* ignore */ }
}

function loadBool(key: string, fallback: boolean): boolean {
	try {
		const stored = localStorage.getItem(key);
		if (stored === "true") return true;
		if (stored === "false") return false;
	} catch { /* ignore */ }
	return fallback;
}

function loadSplitRatio(): number {
	try {
		const stored = localStorage.getItem(SPLIT_RATIO_KEY);
		if (stored) {
			const val = parseFloat(stored);
			if (val >= MIN_SPLIT && val <= MAX_SPLIT) return val;
		}
	} catch { /* ignore */ }
	return DEFAULT_SPLIT_RATIO;
}

function loadTimeMode(): TimeMode {
	try {
		const stored = localStorage.getItem(TIME_MODE_KEY);
		if (stored === "marker" || stored === "dual" || stored === "background") return stored;
	} catch { /* ignore */ }
	return "marker";
}

interface AppProps {
	integrations: UseIntegrationsResult;
}

function App({ integrations }: AppProps) {
	const { toast } = useToast();
	const [usageData, setUsageData] = useState<UsageData | null>(null);
	const [showMenu, setShowMenu] = useState(false);
	const [menuPos, setMenuPos] = useState({ x: 0, y: 0 });
	const [timeMode, setTimeMode] = useState<TimeMode>(loadTimeMode);
	const [showLive, setShowLive] = useState(() => loadBool(SHOW_LIVE_KEY, true));
	const [showAnalytics, setShowAnalytics] = useState(() => loadBool(SHOW_ANALYTICS_KEY, false));
	const [splitRatio, setSplitRatio] = useState(loadSplitRatio);
	const liveRef = useRef<HTMLDivElement>(null);
	const upperRef = useRef<HTMLDivElement>(null);
	const splitRatioRef = useRef(splitRatio);
	const observerRef = useRef<ResizeObserver | null>(null);
	const showLiveRef = useRef(showLive);
	const showAnalyticsRef = useRef(showAnalytics);
	const currentLayoutRef = useRef<LayoutKey | null>(
		layoutKey(loadBool(SHOW_LIVE_KEY, true), loadBool(SHOW_ANALYTICS_KEY, false)),
	);
	const {
		statuses,
		loading: providersLoading,
		error: providersError,
		hasEnabledProvider,
		refresh: refreshIntegrations,
	} = integrations;
	const hasDetectedProvider = statuses.some((status) => status.detectedCli);
	const liveProviderKey = statuses
		.filter((status) => status.enabled)
		.map((status) => status.provider)
		.join(",");

	const saveCurrentSize = useCallback(async () => {
		const key = currentLayoutRef.current;
		if (!key) return;
		try {
			const size = await getCurrentWindow().innerSize();
			saveSize(key, Math.round(size.width), Math.round(size.height));
		} catch { /* ignore */ }
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
				} catch { /* ignore */ }
			}

			setShowLive(nextLive);
			setShowAnalytics(nextAnalytics);
			showLiveRef.current = nextLive;
			showAnalyticsRef.current = nextAnalytics;
			currentLayoutRef.current = nextKey;
			try { localStorage.setItem(SHOW_LIVE_KEY, String(nextLive)); } catch { /* ignore */ }
			try { localStorage.setItem(SHOW_ANALYTICS_KEY, String(nextAnalytics)); } catch { /* ignore */ }

			if (nextKey) {
				const saved = loadSize(nextKey);
				const width = currentWidth ?? saved.width;
				try {
					await getCurrentWindow().setSize(new LogicalSize(width, saved.height));
				} catch { /* ignore */ }
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

	const handleTimeModeChange = useCallback((mode: TimeMode) => {
		setTimeMode(mode);
		try { localStorage.setItem(TIME_MODE_KEY, mode); } catch { /* ignore */ }
	}, []);

	const isSplit = showLive && showAnalytics;

	const observeLiveTargets = useCallback((observer: ResizeObserver) => {
		const liveEl = liveRef.current;
		if (!liveEl) return;

		observer.observe(liveEl);

		const usageDisplay = liveEl.querySelector(".usage-display");
		if (usageDisplay instanceof HTMLElement) {
			observer.observe(usageDisplay);
		}
	}, []);

	const handleDividerMouseDown = useCallback(
		(e: React.MouseEvent<HTMLDivElement>) => {
			e.preventDefault();
			const liveEl = liveRef.current;
			const containerEl = upperRef.current;
			if (!liveEl || !containerEl) return;
			const dividerRect = e.currentTarget.getBoundingClientRect();
			const dragOffset = e.clientY - dividerRect.top;

			const liveInner = liveEl.querySelector(".usage-display") as HTMLElement | null;
			const analyticsInner = containerEl.querySelector(".analytics-view") as HTMLElement | null;
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

			observerRef.current?.disconnect();

			document.documentElement.classList.add("dragging-divider");
			(e.currentTarget as HTMLElement).classList.add("active");

			let rafId = 0;

			const onMouseMove = (ev: MouseEvent) => {
				cancelAnimationFrame(rafId);
				const clientY = ev.clientY;
				rafId = requestAnimationFrame(() => {
					const rect = containerEl.getBoundingClientRect();
					const dividerTop = clientY - dragOffset;
					const ratio = Math.max(
						MIN_SPLIT,
						Math.min(MAX_SPLIT, (dividerTop - rect.top) / rect.height),
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

				if (observerRef.current) {
					observeLiveTargets(observerRef.current);
				}

				setSplitRatio(splitRatioRef.current);
				try { localStorage.setItem(SPLIT_RATIO_KEY, String(splitRatioRef.current)); } catch { /* ignore */ }
			};

			document.addEventListener("mousemove", onMouseMove);
			document.addEventListener("mouseup", onMouseUp);
		},
		[observeLiveTargets],
	);

	const handleDividerKeyDown = useCallback(
		(e: React.KeyboardEvent<HTMLDivElement>) => {
			const step = 0.02;
			let delta = 0;
			if (e.key === "ArrowUp") delta = -step;
			else if (e.key === "ArrowDown") delta = step;
			else return;

			e.preventDefault();
			const next = Math.max(MIN_SPLIT, Math.min(MAX_SPLIT, splitRatioRef.current + delta));
			splitRatioRef.current = next;
			setSplitRatio(next);
			if (liveRef.current) {
				liveRef.current.style.flex = `0 0 ${next * 100}%`;
			}
			try { localStorage.setItem(SPLIT_RATIO_KEY, String(next)); } catch { /* ignore */ }
		},
		[],
	);

	const refresh = useCallback(async () => {
		try {
			const data = await invoke<UsageData>("fetch_usage_data");
			setUsageData(data);
		} catch (e) {
			toast("error", `Usage data fetch failed: ${e}`);
			setUsageData({ buckets: [], provider_errors: [], provider_credits: [], error: String(e) });
		}
	}, [toast]);

	useEffect(() => {
		if (providersLoading) {
			return;
		}
		if (!hasEnabledProvider) {
			setUsageData(null);
			return;
		}
		refresh();
		const interval = setInterval(refresh, 3 * 60_000);
		return () => clearInterval(interval);
	}, [hasEnabledProvider, liveProviderKey, providersLoading, refresh]);

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
			await saveCurrentSize();
			await invoke("hide_window");
		});
		return () => { unlistenPromise.then((fn) => fn()); };
	}, [saveCurrentSize]);

	useEffect(() => {
		if (!showLive) return;

		const el = liveRef.current;
		if (!el) return;

		const fallbackHeight = BASE_HEIGHTS[timeMode] ?? 200;
		let rafId = 0;
		let mutationObserver: MutationObserver | null = null;

		const updateScale = () => {
			cancelAnimationFrame(rafId);
			rafId = requestAnimationFrame(() => {
				const w = el.clientWidth;
				const h = el.clientHeight;
				if (w <= 0 || h <= 0) return;
				const availableHeight = Math.max(h - LIVE_FIT_GUTTER_PX, 1);

				el.style.setProperty("--s", "1");
				const usageDisplay = el.querySelector(".usage-display");
				const contentHeight =
					usageDisplay instanceof HTMLElement
						? Math.max(usageDisplay.scrollHeight, fallbackHeight)
						: fallbackHeight;
				const wScale = w / BASE_WIDTH;
				const hScale = availableHeight / contentHeight;
				const maxLiveScale = isSplit ? 1 : 2.5;
				let scale =
					Math.round(
						Math.max(MIN_LIVE_SCALE, Math.min(wScale, hScale, maxLiveScale)) * 100,
					) / 100;

				el.style.setProperty("--s", String(scale));

				if (usageDisplay instanceof HTMLElement) {
					const fittedHeight = usageDisplay.scrollHeight;
					if (fittedHeight > availableHeight) {
						const correctedScale =
							Math.round(
								Math.max(
									MIN_LIVE_SCALE,
									Math.min(scale * (availableHeight / fittedHeight), maxLiveScale),
								) * 100,
							) / 100;
						scale = correctedScale;
					}
				}

				el.style.setProperty("--s", String(scale));
			});
		};

		const observer = new ResizeObserver(updateScale);
		observerRef.current = observer;
		observeLiveTargets(observer);
		const usageDisplay = el.querySelector(".usage-display");
		if (usageDisplay instanceof HTMLElement) {
			mutationObserver = new MutationObserver(updateScale);
			mutationObserver.observe(usageDisplay, {
				attributes: true,
				attributeFilter: ["class", "style"],
				childList: true,
				subtree: true,
				characterData: true,
			});
		}
		updateScale();
		return () => {
			observer.disconnect();
			mutationObserver?.disconnect();
			observerRef.current = null;
			cancelAnimationFrame(rafId);
		};
	}, [isSplit, splitRatio, timeMode, showLive, usageData, liveProviderKey, observeLiveTargets]);

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

	const handleRefresh = async () => {
		closeMenu();
		await refreshIntegrations();
		if (hasEnabledProvider && !providersLoading) {
			await refresh();
		}
	};

	const liveStyle = isSplit ? { flex: `0 0 ${splitRatio * 100}%` } : undefined;
	const emptyState = (() => {
		if (providersError) {
			return {
				title: "Provider status unavailable",
				description:
					"Quill could not load integration status. Restart the app, then enable Claude Code or Codex from the QUILL menu.",
			};
		}
		if (hasDetectedProvider) {
			return {
				title: "No provider is enabled",
				description:
					"Enable Claude Code or Codex from the QUILL menu to restore Quill features.",
			};
		}
		return {
			title: "Install Claude Code or Codex",
			description:
				"Quill needs at least one supported provider installed and enabled before its features can run.",
		};
	})();

	return (
		<div className="app" onContextMenu={handleContextMenu} onClick={closeMenu}>
			<TitleBar
				showLive={showLive}
				showAnalytics={showAnalytics}
				onToggleLive={handleToggleLive}
				onToggleAnalytics={handleToggleAnalytics}
				onClose={handleClose}
				pendingUpdate={pendingUpdate}
				updating={updating}
				onUpdate={handleUpdate}
				integrations={integrations}
			/>
			<div
				className={`panels${isSplit ? " panels--split" : ""}`}
				ref={upperRef}
			>
				{providersLoading ? (
					<div className="content">
						<div className="loading">Checking integrations...</div>
					</div>
				) : !hasEnabledProvider ? (
					<div className="content">
						<div className="integration-empty-state">
							<p className="integration-empty-state__eyebrow">Providers</p>
							<h2 className="integration-empty-state__title">
								{emptyState.title}
							</h2>
							<p className="integration-empty-state__description">
								{emptyState.description}
							</p>
							<button
								className="integration-empty-state__action"
								onClick={() => void refreshIntegrations()}
								disabled={providersLoading}
							>
								Rescan Providers
							</button>
						</div>
					</div>
				) : (
					<>
						{showLive && (
					<div className="content live-content" ref={liveRef} style={liveStyle}>
						<UsageDisplay
							data={usageData}
							timeMode={timeMode}
							enabledProviders={statuses.filter((status) => status.enabled).map((status) => status.provider)}
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
						{!showLive && !showAnalytics && (
					<div className="content">
						<div className="loading">Toggle a view from the titlebar</div>
					</div>
						)}
					</>
				)}
			</div>
			{showMenu && (
				<div
					className="context-menu"
					style={{ left: menuPos.x, top: menuPos.y }}
					onClick={(e) => e.stopPropagation()}
				>
					<button className="context-menu-item" onClick={() => void handleRefresh()}>
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
