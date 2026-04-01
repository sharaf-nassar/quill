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
  return provider === "claude" ? "Claude Code" : "Codex";
}

function providerActionCopy(action: PendingProviderAction) {
  const label = providerLabel(action.provider);
  if (action.nextEnabled) {
    return {
      title: `Enable ${label}?`,
      description:
        `Quill will install its ${label} integration assets, including hooks, commands, MCP configuration, and managed instruction blocks.`,
      confirmLabel: `Enable ${label}`,
      destructive: false,
    };
  }

  return {
    title: `Disable ${label}?`,
    description:
      `Quill will remove every ${label} integration asset it installed, including hooks, commands, MCP entries, and managed instruction blocks. Historical Quill data stays in the app.`,
    confirmLabel: `Disable ${label}`,
    destructive: true,
  };
}

function IntegrationsWindowView() {
  const { toast } = useToast();
  const currentWindow = getCurrentWindow();
  const {
    statuses,
    loading,
    error,
    inFlightProviders,
    enableProvider,
    disableProvider,
  } = useIntegrations();
  const [pendingProviderAction, setPendingProviderAction] =
    useState<PendingProviderAction | null>(null);

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
        await enableProvider(provider);
      } else {
        await disableProvider(provider);
      }
      await currentWindow.close();
    } catch (error) {
      toast(
        "error",
        `${nextEnabled ? "Enable" : "Disable"} failed for ${label}: ${String(error)}`,
      );
    }
  }, [currentWindow, disableProvider, enableProvider, pendingProviderAction, toast]);

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
          onRequestToggle={handleRequestToggle}
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
          onCancel={() => setPendingProviderAction(null)}
          onConfirm={handleConfirmProviderAction}
        />
      )}
    </>
  );
}

export default IntegrationsWindowView;
