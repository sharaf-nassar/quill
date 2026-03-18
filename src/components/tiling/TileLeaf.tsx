import { useRef, useEffect, useCallback, type ReactNode } from "react";
import type { SectionId } from "./types";

const BASE_WIDTH = 260;
const BASE_HEIGHTS: Record<string, number> = {
	marker: 200,
	dual: 250,
	background: 200,
};

const PANEL_LABELS: Record<SectionId, string> = {
	live: "Live",
	analytics: "Analytics",
};

interface TileLeafProps {
	panelId: SectionId;
	children: ReactNode;
	draggable: boolean;
	onDragStart?: (panelId: SectionId) => void;
	timeMode?: string;
}

export default function TileLeaf({
	panelId,
	children,
	draggable,
	onDragStart,
	timeMode = "marker",
}: TileLeafProps) {
	const leafRef = useRef<HTMLDivElement>(null);
	const contentRef = useRef<HTMLDivElement>(null);

	useEffect(() => {
		if (panelId !== "live") return;

		const measureEl = contentRef.current;
		const targetEl = leafRef.current;
		if (!measureEl || !targetEl) return;

		const baseH = BASE_HEIGHTS[timeMode] ?? 200;
		let rafId = 0;
		let lastScale = -1;

		const updateScale = () => {
			cancelAnimationFrame(rafId);
			rafId = requestAnimationFrame(() => {
				const w = measureEl.clientWidth;
				const h = measureEl.clientHeight;
				if (w <= 0 || h <= 0) return;
				const wScale = w / BASE_WIDTH;
				const hScale = h / baseH;
				const scale = Math.round(Math.max(0.6, Math.min(wScale, hScale, 2.5)) * 100) / 100;
				if (scale !== lastScale) {
					lastScale = scale;
					targetEl.style.setProperty("--s", String(scale));
				}
			});
		};

		const observer = new ResizeObserver(updateScale);
		observer.observe(measureEl);
		updateScale();

		return () => {
			observer.disconnect();
			cancelAnimationFrame(rafId);
		};
	}, [panelId, timeMode]);

	const handleDragStart = useCallback(
		(e: React.MouseEvent) => {
			if (!draggable || !onDragStart) return;
			e.preventDefault();

			const startX = e.clientX;
			const startY = e.clientY;

			const onMouseMove = (ev: MouseEvent) => {
				const dx = ev.clientX - startX;
				const dy = ev.clientY - startY;
				if (Math.abs(dx) > 5 || Math.abs(dy) > 5) {
					document.removeEventListener("mousemove", onMouseMove);
					document.removeEventListener("mouseup", onMouseUp);
					onDragStart(panelId);
				}
			};

			const onMouseUp = () => {
				document.removeEventListener("mousemove", onMouseMove);
				document.removeEventListener("mouseup", onMouseUp);
			};

			document.addEventListener("mousemove", onMouseMove);
			document.addEventListener("mouseup", onMouseUp);
		},
		[draggable, onDragStart, panelId],
	);

	return (
		<div ref={leafRef} className="tile-leaf" data-panel-id={panelId}>
			<div
				className={`tile-leaf-header${draggable ? " tile-leaf-header--draggable" : ""}`}
				onMouseDown={handleDragStart}
			>
				{draggable && <span className="tile-leaf-grip">⠿</span>}
				<span className="tile-leaf-label">{PANEL_LABELS[panelId]}</span>
			</div>
			<div ref={contentRef} className="tile-leaf-content">
				{children}
			</div>
		</div>
	);
}
