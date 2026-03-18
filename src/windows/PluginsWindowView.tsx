import { useState, useCallback } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { invoke } from "@tauri-apps/api/core";
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
import type { PluginsTab } from "../types";
import "../styles/plugins.css";

function PluginsWindowView() {
	const [activeTab, setActiveTab] = useState<PluginsTab>("installed");
	const installed = useInstalledPlugins();
	const marketplaces = useMarketplaces();
	const updates = useAvailableUpdates();
	const operations = usePluginOperations();
	const bulkUpdate = useBulkUpdate();

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
			await invoke("add_marketplace", { repo });
			handlePluginChanged();
		},
		[handlePluginChanged],
	);

	const handleRemoveMarketplace = useCallback(
		async (name: string) => {
			await invoke("remove_marketplace", { name });
			handlePluginChanged();
		},
		[handlePluginChanged],
	);

	const handleRefreshMarketplace = useCallback(
		async (name: string) => {
			await invoke("refresh_marketplace", { name });
			handlePluginChanged();
		},
		[handlePluginChanged],
	);

	const handleRefreshAllMarketplaces = useCallback(async () => {
		await invoke("refresh_all_marketplaces");
		handlePluginChanged();
	}, [handlePluginChanged]);

	if (installed.loading && marketplaces.loading) {
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
				<div className="plugins-body">
					<div className="plugins-loading">Loading...</div>
				</div>
			</div>
		);
	}

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
			<div className="plugins-body">
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
						onAdd={handleAddMarketplace}
						onRemove={handleRemoveMarketplace}
						onRefresh={handleRefreshMarketplace}
						onRefreshAll={handleRefreshAllMarketplaces}
					/>
				)}
				{activeTab === "updates" && (
					<UpdatesTab
						updates={updates}
						operations={operations}
						bulkUpdate={bulkUpdate}
						onChanged={handlePluginChanged}
					/>
				)}
			</div>
		</div>
	);
}

export default PluginsWindowView;
