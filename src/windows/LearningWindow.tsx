import { useState, useRef, useEffect, useCallback } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { useLearningData } from "../hooks/useLearningData";
import StatusStrip from "../components/learning/StatusStrip";
import RuleCard from "../components/learning/RuleCard";
import DomainBreakdown from "../components/learning/DomainBreakdown";
import FloatingRunsWindow from "../components/learning/FloatingRunsWindow";
import { MemoriesPanel } from "../components/learning/MemoriesPanel";
import type { LearningSettings, ProviderFilter } from "../types";
import "../styles/learning.css";

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
  const [providerFilter, setProviderFilter] = useState<ProviderFilter>("all");
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
    promoteRule,
  } = useLearningData(providerFilter);

  const [activeTab, setActiveTab] = useState<"rules" | "memories">("rules");
  const [showRuns, setShowRuns] = useState(false);
  const handleCloseRuns = useCallback(() => setShowRuns(false), []);
  const contentRef = useRef<HTMLDivElement>(null);

  const handleToggleEnabled = (on: boolean) => {
    updateSettings({ ...settings, enabled: on });
  };

  const handleClose = async () => {
    await getCurrentWindow().close();
  };

  if (loading) {
    return (
      <div className="learning-window">
        <div className="learning-window-titlebar" data-tauri-drag-region>
          <span className="learning-window-title" data-tauri-drag-region>Learning</span>
          <button className="learning-window-close" onClick={handleClose} aria-label="Close">&times;</button>
        </div>
        <div className="learning-app">
          <div className="learning-loading">Loading...</div>
        </div>
      </div>
    );
  }

  return (
    <div className="learning-window">
      <div className="learning-window-titlebar" data-tauri-drag-region>
        <span className="learning-window-title" data-tauri-drag-region>Learning</span>
        <button className="learning-window-close" onClick={handleClose} aria-label="Close">&times;</button>
      </div>
      <div className="learning-app">
      <div className="learning-toolbar">
        <span className="learning-toolbar-label">Learning</span>
        <div style={{ display: "flex", gap: 2, marginLeft: 8 }}>
          <button
            className={`learning-cog-btn${activeTab === "rules" ? " learning-runs-btn--active" : ""}`}
            style={{ fontSize: 10, fontWeight: activeTab === "rules" ? 700 : 400 }}
            onClick={() => setActiveTab("rules")}
          >
            Rules
          </button>
          <button
            className={`learning-cog-btn${activeTab === "memories" ? " learning-runs-btn--active" : ""}`}
            style={{ fontSize: 10, fontWeight: activeTab === "memories" ? 700 : 400 }}
            onClick={() => setActiveTab("memories")}
          >
            Memories
          </button>
        </div>
        <select
          className="learning-provider-filter"
          value={providerFilter}
          onChange={(event) => setProviderFilter(event.target.value as ProviderFilter)}
          aria-label="Provider filter"
        >
          <option value="all">All Providers</option>
          <option value="claude">Claude</option>
          <option value="codex">Codex</option>
        </select>
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
        {activeTab === "rules" ? (
          <>
            <StatusStrip
              providerFilter={providerFilter}
              observationCount={observationCount}
              unanalyzedCount={unanalyzedCount}
              topTools={topTools}
              sparkline={sparkline}
              lastRun={runs[0]}
              analyzing={analyzing}
              onAnalyze={triggerAnalysis}
            />
            {(() => {
              const activeRules = rules.filter((r) => r.file_path.length > 0);
              const discoveredRules = rules.filter((r) => r.file_path.length === 0);
              return (
                <>
                  <div className="learning-section">
                    <div className="learning-section-header">
                      ACTIVE RULES
                      <span className="learning-section-count">{activeRules.length}</span>
                    </div>
                    {activeRules.length === 0 ? (
                      <div className="learning-empty">
                        No active rules yet. Promote discovered rules or run an analysis.
                      </div>
                    ) : (
                      activeRules.map((rule) => (
                        <RuleCard key={rule.name} rule={rule} onDelete={deleteRule} />
                      ))
                    )}
                  </div>
                  {discoveredRules.length > 0 && (
                    <div className="learning-section">
                      <div className="learning-section-header">
                        DISCOVERED
                        <span className="learning-section-count">{discoveredRules.length}</span>
                      </div>
                      {discoveredRules.map((rule) => (
                        <RuleCard key={rule.name} rule={rule} onDelete={deleteRule} onPromote={promoteRule} />
                      ))}
                    </div>
                  )}
                  <DomainBreakdown rules={rules} />
                </>
              );
            })()}
          </>
        ) : (
          <MemoriesPanel providerFilter={providerFilter} />
        )}
      </div>
      {showRuns && (
        <FloatingRunsWindow onClose={handleCloseRuns} />
      )}
      </div>
    </div>
  );
}

export default LearningPanel;
