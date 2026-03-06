import { getCurrentWindow } from "@tauri-apps/api/window";
import { useLearningData } from "../hooks/useLearningData";
import RunHistory from "../components/learning/RunHistory";
import "../styles/learning.css";

function RunsWindowView() {
  const { runs, analyzing, liveLogs, loading } = useLearningData();

  const handleClose = async () => {
    await getCurrentWindow().close();
  };

  return (
    <div className="runs-window">
      <div className="runs-window-titlebar" data-tauri-drag-region>
        <span className="runs-window-title" data-tauri-drag-region>
          Run History
        </span>
        <button
          className="runs-window-close"
          onClick={handleClose}
          aria-label="Close"
        >
          &times;
        </button>
      </div>
      <div className="runs-window-body">
        {loading ? (
          <div className="learning-loading">Loading...</div>
        ) : (
          <RunHistory runs={runs} analyzing={analyzing} liveLogs={liveLogs} />
        )}
      </div>
    </div>
  );
}

export default RunsWindowView;
