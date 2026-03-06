import { useState, useRef, useEffect, useCallback } from "react";
import { getCurrentWindow, LogicalSize } from "@tauri-apps/api/window";
import { useLearningData } from "../hooks/useLearningData";
import StatusStrip from "../components/learning/StatusStrip";
import RuleCard from "../components/learning/RuleCard";
import DomainBreakdown from "../components/learning/DomainBreakdown";
import FloatingRunsWindow from "../components/learning/FloatingRunsWindow";
import type { LearningSettings } from "../types";

const TRIGGER_OPTIONS = [
  { value: "on-demand", label: "On-demand" },
  { value: "session-end", label: "Session end" },
  { value: "periodic", label: "Periodic" },
  { value: "session-end+periodic", label: "Both" },
] as const;

interface LearningSettingsInlineProps {
  settings: LearningSettings;
  onUpdateSettings: (settings: LearningSettings) => void;
}

function LearningSettingsInline({
  settings,
  onUpdateSettings,
}: LearningSettingsInlineProps) {
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) return;
    const handler = (e: MouseEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) {
        setOpen(false);
      }
    };
    document.addEventListener("mousedown", handler);
    return () => document.removeEventListener("mousedown", handler);
  }, [open]);

  const showInterval =
    settings.trigger_mode === "periodic" ||
    settings.trigger_mode === "session-end+periodic";

  return (
    <div className="learning-cog-wrap" ref={ref}>
      <button
        className="learning-cog-btn"
        onClick={() => setOpen((v) => !v)}
        aria-label="Learning settings"
      >
        &#9881;
      </button>
      {open && (
        <div className="learning-cog-menu">
          <div className="learning-cog-menu-header">TRIGGER MODE</div>
          {TRIGGER_OPTIONS.map((opt) => (
            <button
              key={opt.value}
              className={`learning-cog-menu-item${settings.trigger_mode === opt.value ? " active" : ""}`}
              onClick={() =>
                onUpdateSettings({ ...settings, trigger_mode: opt.value })
              }
            >
              {opt.label}
            </button>
          ))}
          {showInterval && (
            <div className="learning-cog-interval">
              <span>Every</span>
              <input
                type="number"
                className="learning-interval-input"
                value={settings.periodic_minutes}
                onChange={(e) => {
                  const val = parseInt(e.target.value, 10);
                  if (!isNaN(val) && val >= 10) {
                    onUpdateSettings({ ...settings, periodic_minutes: val });
                  }
                }}
                min={10}
                max={1440}
              />
              <span>min</span>
            </div>
          )}
          <div className="learning-cog-menu-header">MIN CONFIDENCE</div>
          <div className="learning-cog-interval">
            <input
              type="number"
              className="learning-interval-input"
              value={settings.min_confidence}
              onChange={(e) => {
                const val = parseFloat(e.target.value);
                if (!isNaN(val) && val >= 0 && val <= 1) {
                  onUpdateSettings({ ...settings, min_confidence: val });
                }
              }}
              min={0}
              max={1}
              step={0.05}
            />
          </div>
        </div>
      )}
    </div>
  );
}

function LearningPanel() {
  const {
    settings,
    rules,
    runs,
    observationCount,
    unanalyzedCount,
    topTools,
    sparkline,
    analyzing,
    loading,
    updateSettings,
    triggerAnalysis,
    deleteRule,
  } = useLearningData();

  const [showRuns, setShowRuns] = useState(false);
  const handleCloseRuns = useCallback(() => setShowRuns(false), []);
  const contentRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const el = contentRef.current;
    if (!el) return;

    const grow = async () => {
      const overflow = el.scrollHeight - el.clientHeight;
      if (overflow <= 0) return;

      const win = getCurrentWindow();
      const size = await win.innerSize();
      const maxH = window.screen.availHeight * 0.85;
      const newH = Math.min(size.height + overflow, maxH);
      if (newH > size.height) {
        await win.setSize(new LogicalSize(size.width, Math.round(newH)));
      }
    };

    const observer = new ResizeObserver(() => {
      grow();
    });
    observer.observe(el);
    grow();

    return () => observer.disconnect();
  });

  const handleToggleEnabled = (on: boolean) => {
    updateSettings({ ...settings, enabled: on });
  };

  if (loading) {
    return (
      <div className="learning-app">
        <div className="learning-toolbar">
          <span className="learning-toolbar-label">Learning</span>
        </div>
        <div className="learning-loading">Loading...</div>
      </div>
    );
  }

  return (
    <div className="learning-app">
      <div className="learning-toolbar">
        <span className="learning-toolbar-label">Learning</span>
        <div className="learning-toolbar-right">
          {settings.trigger_mode !== "on-demand" && (
            <button
              className={`learning-toggle${settings.enabled ? " learning-toggle--on" : ""}`}
              onClick={() => handleToggleEnabled(!settings.enabled)}
              aria-label={
                settings.enabled ? "Disable learning" : "Enable learning"
              }
            >
              <span className="learning-toggle-dot" />
              {settings.enabled ? "ON" : "OFF"}
            </button>
          )}
          <button
            className={`learning-runs-btn${showRuns ? " learning-runs-btn--active" : ""}`}
            onClick={() => setShowRuns((v) => !v)}
            aria-label="Toggle run history"
            title="Run history"
          >
            <svg width="12" height="12" viewBox="0 0 16 16" fill="none">
              <circle cx="8" cy="8" r="6.5" stroke="currentColor" strokeWidth="1.5" />
              <path d="M8 4.5V8.5L10.5 10" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" />
            </svg>
            {runs.length > 0 && (
              <span className="learning-runs-badge">{runs.length}</span>
            )}
          </button>
          <LearningSettingsInline
            settings={settings}
            onUpdateSettings={updateSettings}
          />
        </div>
      </div>
      <div className="learning-content" ref={contentRef}>
        <StatusStrip
          observationCount={observationCount}
          unanalyzedCount={unanalyzedCount}
          topTools={topTools}
          sparkline={sparkline}
          lastRun={runs[0]}
          analyzing={analyzing}
          onAnalyze={triggerAnalysis}
        />
        <div className="learning-section">
          <div className="learning-section-header">
            RULES
            <span className="learning-section-count">{rules.length}</span>
          </div>
          {rules.length === 0 ? (
            <div className="learning-empty">
              No rules learned yet. Run an analysis to get started.
            </div>
          ) : (
            <>
              {rules.map((rule) => (
                <RuleCard key={rule.name} rule={rule} onDelete={deleteRule} />
              ))}
              <DomainBreakdown rules={rules} />
            </>
          )}
        </div>
      </div>
      {showRuns && (
        <FloatingRunsWindow onClose={handleCloseRuns} />
      )}
    </div>
  );
}

export default LearningPanel;
