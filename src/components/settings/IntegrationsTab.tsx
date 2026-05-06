import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type {
  IndicatorPrimaryProvider,
  IntegrationProvider,
  ProviderStatus,
} from "../../types";
import type { UseIntegrationsResult } from "../../hooks/useIntegrations";
import type { UseIntegrationFeaturesResult } from "../../hooks/useIntegrationFeatures";
import { useToast } from "../../hooks/useToast";
import { providerLabel } from "../../utils/providers";
import ConfirmDialog from "../ConfirmDialog";
import SettingRow from "./SettingRow";
import Toggle, { type ToggleTone } from "./Toggle";

interface IntegrationsTabProps {
  integrations: UseIntegrationsResult;
  features: UseIntegrationFeaturesResult;
}

interface PendingProviderAction {
  provider: IntegrationProvider;
  nextEnabled: boolean;
}

function integrationToggleState(
  status: ProviderStatus,
  busy: boolean,
): { tone: ToggleTone; label: string; disabled: boolean } {
  if (busy) return { tone: "busy", label: "...", disabled: true };
  if (status.enabled) return { tone: "on", label: "ON", disabled: false };
  if (!status.detectedCli) return { tone: "na", label: "N/A", disabled: true };
  if (status.setupState === "missing") {
    return { tone: "setup", label: "SETUP", disabled: false };
  }
  return { tone: "off", label: "OFF", disabled: false };
}

function providerActionCopy(action: PendingProviderAction) {
  const label = providerLabel(action.provider);
  if (action.nextEnabled) {
    if (action.provider === "mini_max") {
      return {
        title: `Enable ${label}?`,
        description:
          "Enter your MiniMax API key to track subscription usage. Your key is stored locally and never sent anywhere except the MiniMax API.",
        confirmLabel: `Enable ${label}`,
        destructive: false,
        needsApiKey: true,
      };
    }
    return {
      title: `Enable ${label}?`,
      description: `Quill will install its ${label} integration assets, including hooks, commands, MCP configuration, and managed instruction blocks.`,
      confirmLabel: `Enable ${label}`,
      destructive: false,
      needsApiKey: false,
    };
  }
  return {
    title: `Disable ${label}?`,
    description:
      action.provider === "mini_max"
        ? "Quill will remove your stored MiniMax API key and stop tracking subscription usage. Historical data stays in the app."
        : `Quill will remove every ${label} integration asset it installed, including hooks, commands, MCP entries, and managed instruction blocks. Historical Quill data stays in the app.`,
    confirmLabel: `Disable ${label}`,
    destructive: true,
    needsApiKey: false,
  };
}

export function IntegrationsTab({ integrations, features }: IntegrationsTabProps) {
  const { toast } = useToast();
  const {
    statuses,
    loading,
    error,
    inFlightProviders,
    indicatorPrimaryProvider,
    rescanInFlight,
    enableProvider,
    disableProvider,
    saveIndicatorPrimaryProvider,
    rescan,
  } = integrations;

  const [pending, setPending] = useState<PendingProviderAction | null>(null);
  const [apiKeyInput, setApiKeyInput] = useState("");
  const [editingMinimaxKey, setEditingMinimaxKey] = useState(false);
  const [minimaxKeyDraft, setMinimaxKeyDraft] = useState("");
  const [savingMinimaxKey, setSavingMinimaxKey] = useState(false);

  const enabledProviders = statuses
    .filter((status) => status.enabled)
    .map((status) => status.provider);
  const unavailablePreferred =
    indicatorPrimaryProvider != null &&
    !enabledProviders.includes(indicatorPrimaryProvider);

  const minimax = statuses.find((s) => s.provider === "mini_max");

  const handleConfirm = async () => {
    if (!pending) return;
    const { provider, nextEnabled } = pending;
    try {
      if (nextEnabled) {
        await enableProvider(
          provider,
          provider === "mini_max" ? apiKeyInput : undefined,
        );
      } else {
        await disableProvider(provider);
      }
      setPending(null);
      setApiKeyInput("");
    } catch (err) {
      toast(
        "error",
        `${nextEnabled ? "Enable" : "Disable"} failed for ${providerLabel(provider)}: ${String(err)}`,
      );
    }
  };

  const handleSaveMinimaxKey = async () => {
    const trimmed = minimaxKeyDraft.trim();
    if (!trimmed) return;
    setSavingMinimaxKey(true);
    try {
      await invoke("set_minimax_api_key", { apiKey: trimmed });
      toast("info", "MiniMax API key updated");
      setEditingMinimaxKey(false);
      setMinimaxKeyDraft("");
    } catch (err) {
      toast("error", `Failed to update MiniMax API key: ${String(err)}`);
    } finally {
      setSavingMinimaxKey(false);
    }
  };

  const confirmCopy = pending ? providerActionCopy(pending) : null;
  const busyConfirm = pending ? inFlightProviders.has(pending.provider) : false;

  return (
    <div className="settings-panel">
      <SettingRow
        label="Status provider"
        description="Which provider's usage drives the tray icon and live badge."
        control={
          <select
            className="settings-select"
            value={indicatorPrimaryProvider ?? ""}
            disabled={loading}
            onChange={(e) =>
              void saveIndicatorPrimaryProvider(
                (e.target.value || null) as IndicatorPrimaryProvider,
              )
            }
            aria-label="Status provider"
          >
            <option value="">Auto</option>
            {unavailablePreferred ? (
              <option value={indicatorPrimaryProvider ?? ""} disabled>
                {providerLabel(indicatorPrimaryProvider!)} (n/a)
              </option>
            ) : null}
            {enabledProviders.map((p) => (
              <option key={p} value={p}>
                {providerLabel(p)}
              </option>
            ))}
          </select>
        }
      />

      <SettingRow
        label="Rescan PATH"
        description="Re-search PATH for the Claude Code and Codex CLIs without restarting Quill."
        control={
          <Toggle
            tone={rescanInFlight ? "busy" : "off"}
            label={rescanInFlight ? "..." : "RUN"}
            disabled={rescanInFlight || loading}
            onClick={() => {
              void rescan().catch((e) => toast("warning", String(e)));
            }}
          />
        }
      />

      <SettingRow
        label="Activity tracking"
        description="Records every tool call into the local Quill database (stays on your machine) to power Live Usage and the activity stream. Token reports and session sync still run when this is off — only the moment-by-moment tool feed stops. Applies to whichever providers are enabled."
        control={
          <Toggle
            tone={
              features.saving
                ? "busy"
                : features.features.activityTracking
                  ? "on"
                  : "off"
            }
            pressed={features.features.activityTracking}
            disabled={features.saving || features.loading}
            onClick={() => {
              void features
                .setActivityTracking(!features.features.activityTracking)
                .catch((e) => toast("error", String(e)));
            }}
          />
        }
      />

      <div className="settings-section-header">Providers</div>
      {loading ? (
        <div className="settings-empty">checking…</div>
      ) : error ? (
        <div className="settings-empty settings-empty--error">{error}</div>
      ) : (
        statuses.map((status) => {
          const busy = inFlightProviders.has(status.provider);
          const state = integrationToggleState(status, busy);
          const attempts = status.lastDetectionAttempts ?? [];
          const description =
            state.tone === "na" && attempts.length > 0
              ? `CLI not found. Checked: ${attempts.slice(0, 3).join(", ")}${attempts.length > 3 ? " …" : ""}`
              : state.tone === "on"
                ? "Quill assets installed and active."
                : state.tone === "setup"
                  ? "Auto-deployment pending; click to run."
                  : state.tone === "na"
                    ? "Provider CLI not detected on this machine."
                    : "Provider detected; Quill assets not installed.";
          return (
            <SettingRow
              key={status.provider}
              label={providerLabel(status.provider)}
              description={description}
              control={
                <Toggle
                  tone={state.tone}
                  label={state.label}
                  pressed={status.enabled}
                  disabled={state.disabled}
                  onClick={() =>
                    setPending({
                      provider: status.provider,
                      nextEnabled: !status.enabled,
                    })
                  }
                />
              }
            />
          );
        })
      )}

      {minimax?.enabled && (
        <SettingRow
          label="MiniMax API key"
          description="Update the stored key without disabling the integration."
          control={
            editingMinimaxKey ? (
              <div className="settings-inline-form">
                <input
                  type="password"
                  className="settings-input"
                  placeholder="sk-cp-..."
                  value={minimaxKeyDraft}
                  onChange={(e) => setMinimaxKeyDraft(e.target.value)}
                  autoFocus
                />
                <button
                  type="button"
                  className="settings-button settings-button--primary"
                  onClick={() => void handleSaveMinimaxKey()}
                  disabled={savingMinimaxKey || !minimaxKeyDraft.trim()}
                >
                  {savingMinimaxKey ? "Saving…" : "Save"}
                </button>
                <button
                  type="button"
                  className="settings-button"
                  onClick={() => {
                    setEditingMinimaxKey(false);
                    setMinimaxKeyDraft("");
                  }}
                  disabled={savingMinimaxKey}
                >
                  Cancel
                </button>
              </div>
            ) : (
              <button
                type="button"
                className="settings-button"
                onClick={() => setEditingMinimaxKey(true)}
              >
                Edit
              </button>
            )
          }
        />
      )}

      {pending && confirmCopy && (
        <ConfirmDialog
          open
          title={confirmCopy.title}
          description={confirmCopy.description}
          confirmLabel={confirmCopy.confirmLabel}
          destructive={confirmCopy.destructive}
          busy={busyConfirm}
          confirmDisabled={confirmCopy.needsApiKey && !apiKeyInput.trim()}
          onCancel={() => {
            setPending(null);
            setApiKeyInput("");
          }}
          onConfirm={() => void handleConfirm()}
        >
          {confirmCopy.needsApiKey && (
            <input
              type="password"
              className="confirm-dialog-input"
              placeholder="sk-cp-..."
              value={apiKeyInput}
              onChange={(e) => setApiKeyInput(e.target.value)}
              autoFocus
            />
          )}
        </ConfirmDialog>
      )}
    </div>
  );
}

export default IntegrationsTab;
