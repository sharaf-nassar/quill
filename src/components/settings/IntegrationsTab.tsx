import { useEffect, useId, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type {
  CpaConnectError,
  CpaConnectErrorCode,
  CpaConnectResult,
  CpaConnectionStatus,
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

const DEFAULT_CPA_URL = "http://127.0.0.1:8317";
const CPA_ERROR_CODES = new Set<CpaConnectErrorCode>([
  "invalid_url",
  "hashed_key",
  "unreachable",
  "unauthorized",
  "unsupported_version",
  "unexpected_response",
  "storage",
]);

const CPA_ERROR_MESSAGES: Record<CpaConnectErrorCode, string> = {
  invalid_url:
    "Enter a loopback CPA URL using HTTP or HTTPS (127.0.0.1, localhost, or ::1).",
  hashed_key:
    "CPA's config contains a one-way bcrypt hash. Enter the original plaintext management key; the saved hash cannot connect.",
  unreachable:
    "CPA is unreachable at this URL. Start CPA and verify the port, then retry.",
  unauthorized:
    "CPA rejected the management key. Paste the plaintext management key and retry.",
  unsupported_version:
    "This CPA build does not expose required account fields. Update CPA and retry.",
  unexpected_response:
    "CPA returned an unexpected management response. Check CPA logs and retry.",
  storage: "Quill could not update the CPA connection. Retry the operation.",
};

function parseCpaErrorCandidate(value: unknown): unknown {
  if (value instanceof Error) return parseCpaErrorCandidate(value.message);
  if (typeof value !== "string") return value;
  try {
    return JSON.parse(value) as unknown;
  } catch {
    return value;
  }
}

function normalizeCpaError(error: unknown): CpaConnectError {
  const candidate = parseCpaErrorCandidate(error);
  if (candidate && typeof candidate === "object") {
    const code = Reflect.get(candidate, "code");
    const message = Reflect.get(candidate, "message");
    if (
      typeof code === "string" &&
      CPA_ERROR_CODES.has(code as CpaConnectErrorCode)
    ) {
      return {
        code: code as CpaConnectErrorCode,
        message:
          typeof message === "string" && message.trim()
            ? message
            : CPA_ERROR_MESSAGES[code as CpaConnectErrorCode],
      };
    }
  }
  return {
    code: "unexpected_response",
    message: CPA_ERROR_MESSAGES.unexpected_response,
  };
}

type CpaLifecycle =
  | "loading"
  | "disconnected"
  | "connecting"
  | "connected"
  | "disconnecting"
  | "error";

function cpaLifecycleLabel(state: CpaLifecycle) {
  switch (state) {
    case "loading":
      return "Checking…";
    case "connecting":
      return "Connecting…";
    case "connected":
      return "Connected";
    case "disconnecting":
      return "Disconnecting…";
    case "error":
      return "Needs attention";
    case "disconnected":
      return "Not connected";
  }
}

function CpaConnectionSettings() {
  const urlHintId = useId();
  const keyHintId = useId();
  const draftTouched = useRef(false);
  const [baseUrl, setBaseUrl] = useState(DEFAULT_CPA_URL);
  const [managementKey, setManagementKey] = useState("");
  const [configured, setConfigured] = useState(false);
  const [lifecycle, setLifecycle] = useState<CpaLifecycle>("loading");
  const [feedback, setFeedback] = useState<{
    tone: "info" | "error";
    message: string;
  } | null>(null);
  const [confirmDisconnect, setConfirmDisconnect] = useState(false);

  useEffect(() => {
    let active = true;
    void invoke<CpaConnectionStatus>("get_cpa_connection_status")
      .then((status) => {
        if (!active) return;
        setConfigured(status.configured);
        setLifecycle(status.configured ? "connected" : "disconnected");
        if (status.baseUrl && !draftTouched.current) setBaseUrl(status.baseUrl);
      })
      .catch((error: unknown) => {
        if (!active) return;
        const normalized = normalizeCpaError(error);
        setLifecycle("error");
        setFeedback({ tone: "error", message: normalized.message });
      });
    return () => {
      active = false;
    };
  }, []);

  const busy = lifecycle === "connecting" || lifecycle === "disconnecting";
  const canConnect =
    !busy &&
    lifecycle !== "loading" &&
    baseUrl.trim().length > 0 &&
    managementKey.trim().length > 0;

  const handleConnect = async () => {
    if (!canConnect) return;
    setLifecycle("connecting");
    setFeedback(null);
    try {
      const result = await invoke<CpaConnectResult>("set_cpa_connection", {
        baseUrl: baseUrl.trim(),
        managementKey,
      });
      setConfigured(true);
      setLifecycle("connected");
      setManagementKey("");
      if (result.connection.baseUrl) setBaseUrl(result.connection.baseUrl);
      setFeedback({
        tone: "info",
        message: [result.smoke.claude.message, result.smoke.codex.message].join(
          " ",
        ),
      });
    } catch (error) {
      const normalized = normalizeCpaError(error);
      setLifecycle("error");
      setFeedback({ tone: "error", message: normalized.message });
    }
  };

  const handleDisconnect = async () => {
    if (busy) return;
    setLifecycle("disconnecting");
    setFeedback(null);
    try {
      await invoke("clear_cpa_connection");
      setConfigured(false);
      setLifecycle("disconnected");
      setManagementKey("");
      setConfirmDisconnect(false);
      setFeedback({
        tone: "info",
        message:
          "CPA disconnected. Stored connection, CPA usage state, and cached rows were removed.",
      });
    } catch (error) {
      const normalized = normalizeCpaError(error);
      setLifecycle("error");
      setFeedback({ tone: "error", message: normalized.message });
    }
  };

  return (
    <section className="cpa-settings" aria-labelledby="cpa-settings-heading">
      <div className="cpa-settings-header">
        <h3 id="cpa-settings-heading">CLI Proxy API</h3>
        <span className="cpa-connection-status" data-state={lifecycle}>
          {cpaLifecycleLabel(lifecycle)}
        </span>
      </div>
      <p className="cpa-settings-description">
        Read pooled Claude and Codex OAuth limits from one local CPA instance.
        Quill sends the management key only to this loopback endpoint; CPA then
        calls provider quota APIs for smoke checks and polling.
      </p>

      <form
        className="cpa-settings-form"
        aria-busy={busy}
        onSubmit={(event) => {
          event.preventDefault();
          void handleConnect();
        }}
      >
        <div className="cpa-settings-fields">
          <label className="cpa-settings-field">
            <span>Base URL</span>
            <input
              type="url"
              className="settings-input"
              value={baseUrl}
              disabled={busy || lifecycle === "loading"}
              aria-describedby={urlHintId}
              onChange={(event) => {
                draftTouched.current = true;
                setBaseUrl(event.target.value);
              }}
              autoCapitalize="none"
              autoCorrect="off"
              spellCheck={false}
            />
            <small id={urlHintId}>Loopback only; default {DEFAULT_CPA_URL}.</small>
          </label>
          <label className="cpa-settings-field">
            <span>Management key</span>
            <input
              type="password"
              className="settings-input"
              value={managementKey}
              disabled={busy || lifecycle === "loading"}
              aria-describedby={keyHintId}
              placeholder={configured ? "Enter key to reconnect" : "Required"}
              onChange={(event) => setManagementKey(event.target.value)}
              autoComplete="off"
              spellCheck={false}
            />
            <small id={keyHintId}>
              Enter the original plaintext secret, not CPA's one-way $2… config
              value. Stored locally; never returned to this window.
            </small>
          </label>
        </div>

        <div className="cpa-settings-actions">
          <button
            type="submit"
            className="settings-button settings-button--compact"
            disabled={!canConnect}
          >
            {lifecycle === "connecting"
              ? "Connecting…"
              : configured
                ? "Reconnect"
                : "Connect"}
          </button>
          {configured ? (
            <button
              type="button"
              className="settings-button"
              disabled={busy}
              onClick={() => setConfirmDisconnect(true)}
            >
              Disconnect
            </button>
          ) : null}
        </div>
      </form>

      {feedback ? (
        <p
          className={`cpa-settings-feedback cpa-settings-feedback--${feedback.tone}`}
          role={feedback.tone === "error" ? "alert" : "status"}
          aria-live="polite"
        >
          {feedback.message}
        </p>
      ) : null}

      <p className="cpa-settings-overlap">
        Direct Claude and Codex integrations remain active. In LIMITS, a CPA
        pool replaces the matching direct row; the direct row returns when
        that provider has no CPA pool.
      </p>

      {confirmDisconnect ? (
        <ConfirmDialog
          open
          title="Disconnect CLI Proxy API?"
          description="Quill will delete the saved URL and management key, every CPA usage setting, and all cached CPA snapshots and hourly rows. Direct provider data stays intact."
          confirmLabel="Disconnect CPA"
          destructive
          busy={lifecycle === "disconnecting"}
          onCancel={() => setConfirmDisconnect(false)}
          onConfirm={() => void handleDisconnect()}
        />
      ) : null}
    </section>
  );
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

function IntegrationsTab({ integrations, features }: IntegrationsTabProps) {
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

      <div className="settings-section-header">Usage sources</div>
      <CpaConnectionSettings />

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
