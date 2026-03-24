import { useState, useEffect, useCallback } from "react";
import { getVersion } from "@tauri-apps/api/app";
import { WebviewWindow } from "@tauri-apps/api/webviewWindow";
import { listen } from "@tauri-apps/api/event";
import type { PendingUpdate } from "../types";

interface TitleBarProps {
	showLive: boolean;
	showAnalytics: boolean;
	onToggleLive: (on: boolean) => void;
	onToggleAnalytics: (on: boolean) => void;
	onClose: () => void;
	pendingUpdate: PendingUpdate | null;
	updating: boolean;
	onUpdate: () => void;
}

function TitleBar({
	showLive,
	showAnalytics,
	onToggleLive,
	onToggleAnalytics,
	onClose,
	pendingUpdate,
	updating,
	onUpdate,
}: TitleBarProps) {
	const [version, setVersion] = useState("");
	const [pluginUpdateCount, setPluginUpdateCount] = useState(0);

	useEffect(() => {
		getVersion()
			.then(setVersion)
			.catch(() => {});
	}, []);

	useEffect(() => {
		const unlisten = listen<number>("plugin-updates-available", (event) => {
			setPluginUpdateCount(event.payload);
		});
		return () => {
			unlisten.then((fn) => fn());
		};
	}, []);

	const handleOpenSessions = useCallback(async () => {
		const existing = await WebviewWindow.getByLabel("sessions");
		if (existing) {
			await existing.show();
			await existing.setFocus();
			return;
		}
		new WebviewWindow("sessions", {
			url: "/?view=sessions",
			title: "Session Search",
			width: 1000,
			height: 650,
			minWidth: 600,
			minHeight: 400,
			decorations: false,
			transparent: true,
			resizable: true,
		});
	}, []);

	const handleOpenLearning = useCallback(async () => {
		const existing = await WebviewWindow.getByLabel("learning");
		if (existing) {
			await existing.show();
			await existing.setFocus();
			return;
		}
		new WebviewWindow("learning", {
			url: "/?view=learning",
			title: "Learning",
			width: 500,
			height: 600,
			minWidth: 400,
			minHeight: 400,
			decorations: false,
			transparent: true,
			resizable: true,
		});
	}, []);

	const handleOpenRestart = useCallback(async () => {
		const existing = await WebviewWindow.getByLabel("restart");
		if (existing) {
			await existing.show();
			await existing.setFocus();
			return;
		}
		new WebviewWindow("restart", {
			url: "/?view=restart",
			title: "Restart Claude Code",
			width: 420,
			height: 400,
			minWidth: 320,
			minHeight: 250,
			decorations: false,
			transparent: true,
			resizable: true,
		});
	}, []);

	const handleOpenPlugins = useCallback(async () => {
		const existing = await WebviewWindow.getByLabel("plugins");
		if (existing) {
			await existing.show();
			await existing.setFocus();
			return;
		}
		new WebviewWindow("plugins", {
			url: "/?view=plugins",
			title: "Plugin Manager",
			width: 700,
			height: 550,
			minWidth: 500,
			minHeight: 400,
			decorations: false,
			transparent: true,
			resizable: true,
		});
	}, []);

	return (
		<div className="titlebar" data-tauri-drag-region>
			<div className="titlebar-left">
				<div className="view-toggle">
					<button
						className={`view-tab${showLive ? " active" : ""}`}
						onClick={() => onToggleLive(!showLive)}
					>
						Live
					</button>
					<button
						className={`view-tab${showAnalytics ? " active" : ""}`}
						onClick={() => onToggleAnalytics(!showAnalytics)}
					>
						Analytics
					</button>
					<button
						className="view-tab view-tab--learning"
						onClick={handleOpenLearning}
						aria-label="Open learning"
						title="Learning"
					>
						&#x1F9E0;
					</button>
					<button
						className="view-tab view-tab--search"
						onClick={handleOpenSessions}
						aria-label="Search sessions"
						title="Search sessions"
					>
						&#8981;
					</button>
					<button
						className="view-tab view-tab--plugins"
						onClick={handleOpenPlugins}
						aria-label="Plugin Manager"
						title="Plugin Manager"
					>
						&#9881;
						{pluginUpdateCount > 0 && (
							<span className="plugins-update-badge">{pluginUpdateCount}</span>
						)}
					</button>
					<button
						className="view-tab view-tab--restart"
						onClick={handleOpenRestart}
						aria-label="Restart Claude Code"
						title="Restart Claude Code"
					>
						&#8635;
					</button>
				</div>
			</div>
			<div className="titlebar-center" data-tauri-drag-region>
				{pendingUpdate ? (
					<button
						className="titlebar-update-btn"
						onClick={onUpdate}
						disabled={updating}
						aria-label={`Update to version ${pendingUpdate.version}`}
					>
						{updating ? "Updating\u2026" : `Update ${pendingUpdate.version}`}
					</button>
				) : (
					<span className="titlebar-text" data-tauri-drag-region>
						QUILL
					</span>
				)}
			</div>
			<div className="titlebar-right">
				{version && <span className="titlebar-version">v{version}</span>}
				<button
					className="titlebar-close"
					onClick={onClose}
					aria-label="Close window"
				>
					&times;
				</button>
			</div>
		</div>
	);
}

export default TitleBar;
