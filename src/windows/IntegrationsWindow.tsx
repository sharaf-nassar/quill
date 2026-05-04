import { useCallback, useEffect, useMemo, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import ConfirmDialog from "../components/ConfirmDialog";
import ProviderMenu from "../components/integrations/ProviderMenu";
import { useIntegrations } from "../hooks/useIntegrations";
import { useToast } from "../hooks/useToast";
import type { IntegrationProvider } from "../types";

interface PendingProviderAction {
  provider: IntegrationProvider;
  nextEnabled: boolean;
}

function providerLabel(provider: IntegrationProvider): string {
  if (provider === "claude") return "Claude Code";
  if (provider === "codex") return "Codex";
  return "MiniMax";
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
      description:
        `Quill will install its ${label} integration assets, including hooks, commands, MCP configuration, and managed instruction blocks.`,
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

function IntegrationsWindowView() {
  const { toast } = useToast();
  const currentWindow = getCurrentWindow();
  const {
    statuses,
    indicatorPrimaryProvider,
    loading,
    error,
    inFlightProviders,
    contextPreservation,
    contextPreservationInFlight,
    brevityInFlightProviders,
    rescanInFlight,
    saveIndicatorPrimaryProvider,
    setContextPreservationEnabled,
    enableProvider,
    disableProvider,
    setBrevityEnabled,
    rescan,
  } = useIntegrations();
  const [pendingProviderAction, setPendingProviderAction] =
    useState<PendingProviderAction | null>(null);
  const [apiKeyInput, setApiKeyInput] = useState("");

  useEffect(() => {
    const unlistenPromise = currentWindow.onFocusChanged(({ payload: focused }) => {
      if (!focused) {
        void currentWindow.close();
      }
    });

    return () => {
      unlistenPromise.then((unlisten) => unlisten());
    };
  }, [currentWindow]);

  useEffect(() => {
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape" && !pendingProviderAction) {
        void currentWindow.close();
      }
    };

    document.addEventListener("keydown", handleKeyDown);
    return () => document.removeEventListener("keydown", handleKeyDown);
  }, [currentWindow, pendingProviderAction]);

  const handleRequestToggle = useCallback(
    (provider: IntegrationProvider, nextEnabled: boolean) => {
      setPendingProviderAction({ provider, nextEnabled });
      setApiKeyInput("");
    },
    [],
  );

  const handleConfirmProviderAction = useCallback(async () => {
    if (!pendingProviderAction) {
      return;
    }

    const { provider, nextEnabled } = pendingProviderAction;
    const label = providerLabel(provider);

    try {
      if (nextEnabled) {
        await enableProvider(provider, provider === "mini_max" ? apiKeyInput : undefined);
      } else {
        await disableProvider(provider);
      }
      setApiKeyInput("");
      await currentWindow.close();
    } catch (error) {
      toast(
        "error",
        `${nextEnabled ? "Enable" : "Disable"} failed for ${label}: ${String(error)}`,
      );
    }
  }, [apiKeyInput, currentWindow, disableProvider, enableProvider, pendingProviderAction, toast]);

  const handleContextPreservationToggle = useCallback(
    async (enabled: boolean) => {
      try {
        await setContextPreservationEnabled(enabled);
      } catch (error) {
        toast(
          "error",
          `${enabled ? "Enable" : "Disable"} failed for context preservation: ${String(error)}`,
        );
      }
    },
    [setContextPreservationEnabled, toast],
  );

  const confirmCopy = useMemo(
    () =>
      pendingProviderAction ? providerActionCopy(pendingProviderAction) : null,
    [pendingProviderAction],
  );

  const busyConfirmProvider = pendingProviderAction
    ? inFlightProviders.has(pendingProviderAction.provider)
    : false;

  return (
    <>
      <div className="integrations-window">
        <ProviderMenu
          className="provider-menu--window"
          statuses={statuses}
          loading={loading}
          error={error}
          inFlightProviders={inFlightProviders}
          contextPreservation={contextPreservation}
          contextPreservationInFlight={contextPreservationInFlight}
          brevityInFlightProviders={brevityInFlightProviders}
          indicatorPrimaryProvider={indicatorPrimaryProvider}
          onRequestToggle={handleRequestToggle}
          onContextPreservationToggle={(enabled) => {
            void handleContextPreservationToggle(enabled);
          }}
          onBrevityToggle={(provider, enabled) => {
            void setBrevityEnabled(provider, enabled).catch((e) => {
              toast("warning", String(e));
            });
          }}
          onIndicatorPrimaryProviderChange={(provider) => {
            void saveIndicatorPrimaryProvider(provider);
          }}
          onRescan={() => {
            void rescan().catch((e) => {
              toast("warning", String(e));
            });
          }}
          rescanning={rescanInFlight}
        />
      </div>
      {pendingProviderAction && confirmCopy && (
        <ConfirmDialog
          open
          title={confirmCopy.title}
          description={confirmCopy.description}
          confirmLabel={confirmCopy.confirmLabel}
          destructive={confirmCopy.destructive}
          busy={busyConfirmProvider}
          confirmDisabled={confirmCopy.needsApiKey && !apiKeyInput.trim()}
          onCancel={() => {
            setPendingProviderAction(null);
            setApiKeyInput("");
          }}
          onConfirm={handleConfirmProviderAction}
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
    </>
  );
}

export default IntegrationsWindowView;
