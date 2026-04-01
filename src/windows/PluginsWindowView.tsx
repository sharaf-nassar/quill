import { useState, useCallback, useEffect } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { invoke } from "@tauri-apps/api/core";
import { useIntegrations } from "../hooks/useIntegrations";
import {
	useInstalledPlugins,
	useMarketplaces,
	useAvailableUpdates,
	usePluginOperations,
	useBulkUpdate,
} from "../hooks/usePluginData";
import PluginsTabs from "../components/plugins/PluginsTabs";
import InstalledTab from "../components/plugins/InstalledTab";
import BrowseTab from "../components/plugins/BrowseTab";
import MarketplacesTab from "../components/plugins/MarketplacesTab";
import UpdatesTab from "../components/plugins/UpdatesTab";
import type { IntegrationProvider, PluginsTab } from "../types";
import "../styles/plugins.css";

function providerLabel(provider: IntegrationProvider): string {
	return provider === "claude" ? "Claude Code" : "Codex";
}

function PluginsWindowView() {
	const [activeTab, setActiveTab] = useState<PluginsTab>("installed");
	const integrations = useIntegrations();
	const enabledProviders = integrations.statuses
		.filter((status) => status.enabled)
		.map((status) => status.provider);
	const fallbackProvider = enabledProviders[0] ?? null;
	const [selectedProvider, setSelectedProvider] =
		useState<IntegrationProvider | null>(fallbackProvider);
	const installed = useInstalledPlugins(selectedProvider);
	const marketplaces = useMarketplaces(selectedProvider);
	const updates = useAvailableUpdates(selectedProvider);
	const operations = usePluginOperations();
	const bulkUpdate = useBulkUpdate(selectedProvider);

	useEffect(() => {
		if (selectedProvider && enabledProviders.includes(selectedProvider)) {
			return;
		}
		setSelectedProvider(fallbackProvider);
	}, [enabledProviders, fallbackProvider, selectedProvider]);

	const refreshInstalled = installed.refresh;
	const refreshMarketplaces = marketplaces.refresh;
	const refreshUpdates = updates.refresh;

	const handleClose = async () => {
		await getCurrentWindow().close();
	};

	const handlePluginChanged = useCallback(() => {
		refreshInstalled();
		refreshMarketplaces();
		refreshUpdates();
	}, [refreshInstalled, refreshMarketplaces, refreshUpdates]);

	const handleAddMarketplace = useCallback(
		async (repo: string) => {
			if (!selectedProvider) {
				return;
			}
			await invoke("add_marketplace", { provider: selectedProvider, repo });
			handlePluginChanged();
		},
		[handlePluginChanged, selectedProvider],
	);

	const handleRemoveMarketplace = useCallback(
		async (name: string) => {
			if (!selectedProvider) {
				return;
			}
			await invoke("remove_marketplace", { provider: selectedProvider, name });
			handlePluginChanged();
		},
		[handlePluginChanged, selectedProvider],
	);

	const handleRefreshMarketplace = useCallback(
		async (name: string) => {
			if (!selectedProvider) {
				return;
			}
			await invoke("refresh_marketplace", { provider: selectedProvider, name });
			handlePluginChanged();
		},
		[handlePluginChanged, selectedProvider],
	);

	const handleRefreshAllMarketplaces = useCallback(async () => {
		if (!selectedProvider) {
			return;
		}
		await invoke("refresh_all_marketplaces", { provider: selectedProvider });
		handlePluginChanged();
	}, [handlePluginChanged, selectedProvider]);

	// Auto-dismiss operation result after 5 seconds
	const lastResult = operations.lastResult;
	const clearResult = operations.clearResult;
	useEffect(() => {
		if (!lastResult) return;
		const timer = setTimeout(() => clearResult(), 5000);
		return () => clearTimeout(timer);
	}, [lastResult, clearResult]);

	return (
		<div className="plugins-window">
			<div className="plugins-window-titlebar" data-tauri-drag-region>
				<span className="plugins-window-title" data-tauri-drag-region>
					Plugin Manager
				</span>
				<button
					className="plugins-window-close"
					onClick={handleClose}
					aria-label="Close"
				>
					&times;
				</button>
			</div>
			{operations.lastResult && (
				<div
					className={`plugins-result-banner${operations.lastResult.success ? " plugins-result-banner--success" : " plugins-result-banner--error"}`}
				>
					<span className="plugins-result-banner__message">
						{operations.lastResult.message}
					</span>
					<button
						className="plugins-result-banner__dismiss"
						onClick={operations.clearResult}
					>
						&times;
					</button>
				</div>
			)}
			<div className="plugins-body">
				{!selectedProvider ? (
					<div className="plugins-loading">
						Enable Claude Code or Codex from the QUILL menu to manage plugins.
					</div>
				) : installed.loading && marketplaces.loading ? (
					<div className="plugins-loading">Loading...</div>
				) : (
					<>
						<div className="plugins-provider-switcher">
							{enabledProviders.map((provider) => (
								<button
									key={provider}
									className={`plugins-provider-switcher__button${
										selectedProvider === provider
											? " plugins-provider-switcher__button--active"
											: ""
									}`}
									onClick={() => setSelectedProvider(provider)}
								>
									{providerLabel(provider)}
								</button>
							))}
						</div>
						<PluginsTabs
							activeTab={activeTab}
							onTabChange={setActiveTab}
							updateCount={updates.result.plugin_updates.length}
						/>
						{activeTab === "installed" && (
							<InstalledTab
								plugins={installed.plugins}
								operations={operations}
								onChanged={handlePluginChanged}
							/>
						)}
						{activeTab === "browse" && (
							<BrowseTab
								marketplaces={marketplaces.marketplaces}
								operations={operations}
								onChanged={handlePluginChanged}
							/>
						)}
						{activeTab === "marketplaces" && (
							<MarketplacesTab
								marketplaces={marketplaces.marketplaces}
								allowEditing={selectedProvider === "claude"}
								onAdd={handleAddMarketplace}
								onRemove={handleRemoveMarketplace}
								onRefresh={handleRefreshMarketplace}
								onRefreshAll={handleRefreshAllMarketplaces}
							/>
						)}
						{activeTab === "updates" && (
							<UpdatesTab
								provider={selectedProvider}
								updates={updates}
								operations={operations}
								bulkUpdate={bulkUpdate}
								onChanged={handlePluginChanged}
							/>
						)}
					</>
				)}
			</div>
		</div>
	);
}

export default PluginsWindowView;
