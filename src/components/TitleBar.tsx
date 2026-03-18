import { useState, useEffect, useCallback } from "react";
import { getVersion } from "@tauri-apps/api/app";
import { WebviewWindow } from "@tauri-apps/api/webviewWindow";
import type { PendingUpdate, SectionId } from "../types";
import type { LayoutNode } from "./tiling/types";
import PresetsMenu from "./tiling/PresetsMenu";

interface TitleBarProps {
	showLive: boolean;
	showAnalytics: boolean;
	onToggleSection: (sectionId: SectionId) => void;
	onClose: () => void;
	pendingUpdate: PendingUpdate | null;
	updating: boolean;
	onUpdate: () => void;
	layout: LayoutNode | null;
	visiblePanels: SectionId[];
	onApplyPreset: (tree: LayoutNode) => void;
}

function TitleBar({
	showLive,
	showAnalytics,
	onToggleSection,
	onClose,
	pendingUpdate,
	updating,
	onUpdate,
	layout,
	visiblePanels,
	onApplyPreset,
}: TitleBarProps) {
	const [version, setVersion] = useState("");

	useEffect(() => {
		getVersion()
			.then(setVersion)
			.catch(() => {});
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
			width: 700,
			height: 600,
			minWidth: 500,
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

	return (
		<div className="titlebar" data-tauri-drag-region>
			<div className="titlebar-left">
				<div className="view-toggle">
					<button
						className={`view-tab${showLive ? " active" : ""}`}
						onClick={() => onToggleSection("live")}
					>
						Live
					</button>
					<button
						className={`view-tab${showAnalytics ? " active" : ""}`}
						onClick={() => onToggleSection("analytics")}
					>
						Analytics
					</button>
					<button
						className="view-tab view-tab--learning"
						onClick={handleOpenLearning}
						aria-label="Open learning"
						title="Learning"
					>
						&#10022;
					</button>
					<button
						className="view-tab view-tab--search"
						onClick={handleOpenSessions}
						aria-label="Search sessions"
						title="Search sessions"
					>
						&#8981;
					</button>
				</div>
				{layout && (
					<PresetsMenu
						layout={layout}
						visiblePanels={visiblePanels}
						onApplyPreset={onApplyPreset}
					/>
				)}
			</div>
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
			{version && <span className="titlebar-version">v{version}</span>}
			<button
				className="titlebar-close"
				onClick={onClose}
				aria-label="Close window"
			>
				&times;
			</button>
		</div>
	);
}

export default TitleBar;
