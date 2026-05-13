import { getCurrentWindow } from "@tauri-apps/api/window";

type ResizeDirection =
	| "North"
	| "NorthEast"
	| "East"
	| "SouthEast"
	| "South"
	| "SouthWest"
	| "West"
	| "NorthWest";

const HANDLES: Array<{ className: string; direction: ResizeDirection }> = [
	{ className: "window-resize-handle--north", direction: "North" },
	{ className: "window-resize-handle--north-east", direction: "NorthEast" },
	{ className: "window-resize-handle--east", direction: "East" },
	{ className: "window-resize-handle--south-east", direction: "SouthEast" },
	{ className: "window-resize-handle--south", direction: "South" },
	{ className: "window-resize-handle--south-west", direction: "SouthWest" },
	{ className: "window-resize-handle--west", direction: "West" },
	{ className: "window-resize-handle--north-west", direction: "NorthWest" },
];

function startResize(direction: ResizeDirection) {
	return (event: React.MouseEvent<HTMLDivElement>) => {
		if (event.button !== 0) return;
		event.preventDefault();
		event.stopPropagation();
		void getCurrentWindow().startResizeDragging(direction).catch(() => {});
	};
}

function WindowResizeHandles() {
	return (
		<div className="window-resize-handles" aria-hidden="true">
			{HANDLES.map(({ className, direction }) => (
				<div
					key={direction}
					className={`window-resize-handle ${className}`}
					onMouseDown={startResize(direction)}
				/>
			))}
		</div>
	);
}

export default WindowResizeHandles;
