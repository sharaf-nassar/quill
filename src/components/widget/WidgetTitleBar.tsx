// WidgetTitleBar — the 360px widget's chrome contract.
//
// Everything the old `TitleBar` carried for the app lifecycle survives here in
// widget form: the update affordance (centered, and only when the 4-hour
// updater check found something), the close control (which hides to tray, it
// never quits), and the settings entry into the Manage workspace. New to the
// widget: the always-on-top toggle, which reads and writes the single
// persisted `always_on_top` setting so the tray checkitem and Settings stay in
// sync through `runtime-settings-updated`.
//
// See specs/018-widget-ui-redesign/plan.md#Affected Components.

import { useCallback } from "react";
import { openManageWindow } from "../../lib/manageWindow";
import { useRuntimeSettings } from "../../hooks/useRuntimeSettings";
import { useToast } from "../../hooks/useToast";
import type { PendingUpdate } from "../../types";

export interface WidgetTitleBarProps {
  /** Non-null once the updater check finds a release; shows the update button. */
  pendingUpdate: PendingUpdate | null;
  /** True while `install_app_update` is running. */
  updating: boolean;
  onUpdate: () => void;
  /** Close-to-tray. The widget never exits from the titlebar. */
  onClose: () => void;
}

const SVG_PROPS = {
  viewBox: "0 0 10 10",
  width: 11,
  height: 11,
  fill: "none",
  stroke: "currentColor",
  strokeWidth: 1.1,
  strokeLinecap: "round" as const,
  strokeLinejoin: "round" as const,
  "aria-hidden": true,
  focusable: false,
};

const PinIcon = () => (
  <svg {...SVG_PROPS}>
    <path d="M3.2 1h3.6M5 1v5.4M2.6 6.4h4.8M5 6.4V9" />
  </svg>
);

const SettingsIcon = () => (
  <svg {...SVG_PROPS}>
    <path d="M1 2.6h8M1 5h8M1 7.4h8" />
    <circle cx="6.4" cy="2.6" r="1.1" fill="var(--surface)" />
    <circle cx="3.4" cy="5" r="1.1" fill="var(--surface)" />
    <circle cx="7" cy="7.4" r="1.1" fill="var(--surface)" />
  </svg>
);

const CloseIcon = () => (
  <svg {...SVG_PROPS}>
    <path d="M2.6 2.6l4.8 4.8M7.4 2.6L2.6 7.4" />
  </svg>
);

function WidgetTitleBar({
  pendingUpdate,
  updating,
  onUpdate,
  onClose,
}: WidgetTitleBarProps) {
  const { toast } = useToast();
  const { settings, loading, saving, save } = useRuntimeSettings();

  const handleToggleOnTop = useCallback(async () => {
    const next = { ...settings, alwaysOnTop: !settings.alwaysOnTop };
    try {
      await save(next);
    } catch (e) {
      // The setting is the single source of truth for tray and Settings, so a
      // failed write must surface rather than leave the button lying.
      toast("error", `Always-on-top change failed: ${e instanceof Error ? e.message : String(e)}`);
    }
  }, [settings, save, toast]);

  return (
    <div className="wg-tb" data-tauri-drag-region>
      <div className="wg-tb-brand" data-tauri-drag-region>
        <span className="wg-glyph" aria-hidden="true" />
        <span className="wg-wordmark" data-tauri-drag-region>
          Quill
        </span>
      </div>

      <div className="wg-tb-center">
        {pendingUpdate && (
          <button
            type="button"
            className="wg-update"
            onClick={onUpdate}
            disabled={updating}
            title={`Install Quill ${pendingUpdate.version} and restart`}
          >
            {updating ? "Updating…" : `Update ${pendingUpdate.version}`}
          </button>
        )}
      </div>

      <div className="wg-tb-right">
        <button
          type="button"
          className="wg-key"
          aria-pressed={settings.alwaysOnTop}
          aria-label="Always on top"
          title="Always on top"
          disabled={loading || saving}
          onClick={() => void handleToggleOnTop()}
        >
          <PinIcon />
        </button>

        <button
          type="button"
          className="wg-key"
          aria-label="Open settings"
          title="Settings"
          onClick={() => void openManageWindow("settings")}
        >
          <SettingsIcon />
        </button>

        <button
          type="button"
          className="wg-key"
          aria-label="Hide widget to tray"
          title="Hide to tray"
          onClick={onClose}
        >
          <CloseIcon />
        </button>
      </div>
    </div>
  );
}

export default WidgetTitleBar;
