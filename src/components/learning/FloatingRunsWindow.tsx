import { useEffect } from "react";
import {
	getCurrentWindow,
	currentMonitor,
	PhysicalPosition,
} from "@tauri-apps/api/window";
import { WebviewWindow } from "@tauri-apps/api/webviewWindow";
import type { UnlistenFn } from "@tauri-apps/api/event";

const RUNS_WIDTH = 320;
const RUNS_HEIGHT = 400;
const GAP = 8;

interface FloatingRunsWindowProps {
	onClose: () => void;
}

function clamp(value: number, min: number, max: number): number {
	return Math.min(Math.max(value, min), max);
}

async function calcPosition(): Promise<PhysicalPosition> {
	const parent = getCurrentWindow();
	const physPos = await parent.outerPosition();
	const physSize = await parent.outerSize();
	const monitor = await currentMonitor();

	const scale = monitor?.scaleFactor ?? (await parent.scaleFactor());
	const gap = GAP * scale;
	const runsWidth = RUNS_WIDTH * scale;
	const runsHeight = RUNS_HEIGHT * scale;
	const parentRight = physPos.x + physSize.width;
	const parentBottom = physPos.y + physSize.height;

	// Align bottom of runs window with bottom of parent
	let x = parentRight + gap;
	let y = parentBottom - runsHeight;

	if (monitor) {
		const workLeft = monitor.workArea.position.x;
		const workTop = monitor.workArea.position.y;
		const workRight = workLeft + monitor.workArea.size.width;
		const workBottom = workTop + monitor.workArea.size.height;
		const spaceRight = workRight - parentRight;
		const spaceLeft = physPos.x - workLeft;

		// Prefer right side; fall back to left if not enough room
		if (spaceRight >= runsWidth + gap) {
			x = parentRight + gap;
		} else if (spaceLeft >= runsWidth + gap) {
			x = physPos.x - runsWidth - gap;
		} else {
			x = clamp(parentRight + gap, workLeft, Math.max(workLeft, workRight - runsWidth));
		}

		y = clamp(y, workTop, Math.max(workTop, workBottom - runsHeight));
	}

	return new PhysicalPosition(Math.round(x), Math.round(y));
}

function FloatingRunsWindow({ onClose }: FloatingRunsWindowProps) {
	useEffect(() => {
		let cancelled = false;
		let ownedWindow: WebviewWindow | null = null;
		const unlisteners: Array<Promise<UnlistenFn>> = [];

		const syncCloseState = (win: WebviewWindow) => {
			unlisteners.push(
				win.onCloseRequested(() => {
					if (!cancelled) {
						onClose();
					}
				}),
			);
		};

		const showWindow = async (win: WebviewWindow) => {
			try {
				await win.setPosition(await calcPosition());
			} catch (error) {
				console.error("Failed to position run history window", error);
			}

			if (cancelled) return;
			await win.show();
			await win.setFocus();
		};

		(async () => {
			const existing = await WebviewWindow.getByLabel("runs");
			if (cancelled) return;

			if (existing) {
				ownedWindow = existing;
				syncCloseState(existing);
				await showWindow(existing);
				return;
			}

			const win = new WebviewWindow("runs", {
				url: "/?view=runs",
				title: "Run History",
				width: RUNS_WIDTH,
				height: RUNS_HEIGHT,
				minWidth: 240,
				minHeight: 200,
				decorations: false,
				transparent: true,
				resizable: true,
				alwaysOnTop: true,
				visible: false,
			});
			ownedWindow = win;

			void win.once("tauri://created", () => {
				if (cancelled) return;
				syncCloseState(win);
				void showWindow(win).catch((error) => {
					console.error("Failed to show run history window", error);
					if (!cancelled) {
						onClose();
					}
				});
			});

			void win.once("tauri://error", () => {
				if (!cancelled) {
					onClose();
				}
			});
		})();

		return () => {
			cancelled = true;
			for (const unlisten of unlisteners) {
				void unlisten.then((fn) => fn());
			}
			void ownedWindow?.destroy();
		};
	}, [onClose]);

	return null;
}

export default FloatingRunsWindow;
