import { useCallback } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import RestartPanel from "../components/restart/RestartPanel";
import "../styles/restart.css";

function RestartWindowView() {
	const handleClose = useCallback(async () => {
		await getCurrentWindow().close();
	}, []);

	return (
		<div className="restart-window">
			<div className="restart-window-titlebar" data-tauri-drag-region>
				<span className="restart-window-title" data-tauri-drag-region>
					Restart Agent Sessions
				</span>
				<button
					className="restart-window-close"
					onClick={handleClose}
					aria-label="Close"
				>
					&times;
				</button>
			</div>
			<RestartPanel />
		</div>
	);
}

export default RestartWindowView;
