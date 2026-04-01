import { useState, useEffect, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type {
	BulkUpdateProgress,
	InstalledPlugin,
	IntegrationProvider,
	Marketplace,
	MarketplacePlugin,
	PluginUpdate,
	UpdateCheckResult,
} from "../types";
import {
	installedPluginInstanceKey,
	pluginUpdateInstanceKey,
} from "../utils/plugins";

function usePluginRefreshEffect(refresh: () => Promise<void>) {
	useEffect(() => {
		refresh();
	}, [refresh]);

	useEffect(() => {
		const unlisten = listen("plugin-changed", () => {
			refresh();
		});
		return () => {
			unlisten.then((fn) => fn());
		};
	}, [refresh]);
}

export function useInstalledPlugins(provider: IntegrationProvider | null) {
	const [plugins, setPlugins] = useState<InstalledPlugin[]>([]);
	const [loading, setLoading] = useState(true);
	const [error, setError] = useState<string | null>(null);

	const refresh = useCallback(async () => {
		if (!provider) {
			setPlugins([]);
			setLoading(false);
			return;
		}

		setLoading(true);
		try {
			const data = await invoke<InstalledPlugin[]>("get_installed_plugins", {
				provider,
			});
			setPlugins(data);
			setError(null);
		} catch (e) {
			setError(String(e));
		} finally {
			setLoading(false);
		}
	}, [provider]);

	usePluginRefreshEffect(refresh);

	return { plugins, loading, error, refresh };
}

export function useMarketplaces(provider: IntegrationProvider | null) {
	const [marketplaces, setMarketplaces] = useState<Marketplace[]>([]);
	const [loading, setLoading] = useState(true);
	const [error, setError] = useState<string | null>(null);

	const refresh = useCallback(async () => {
		if (!provider) {
			setMarketplaces([]);
			setLoading(false);
			return;
		}

		setLoading(true);
		try {
			const data = await invoke<Marketplace[]>("get_marketplaces", { provider });
			setMarketplaces(data);
			setError(null);
		} catch (e) {
			setError(String(e));
		} finally {
			setLoading(false);
		}
	}, [provider]);

	usePluginRefreshEffect(refresh);

	return { marketplaces, loading, error, refresh };
}

export function useAvailableUpdates(provider: IntegrationProvider | null) {
	const [result, setResult] = useState<UpdateCheckResult>({
		plugin_updates: [],
		last_checked: null,
		next_check: null,
	});
	const [loading, setLoading] = useState(true);
	const [error, setError] = useState<string | null>(null);

	const refresh = useCallback(async () => {
		if (!provider) {
			setResult({
				plugin_updates: [],
				last_checked: null,
				next_check: null,
			});
			setLoading(false);
			return;
		}

		setLoading(true);
		try {
			const data = await invoke<UpdateCheckResult>("get_available_updates", {
				provider,
			});
			setResult(data);
			setError(null);
		} catch (e) {
			setError(String(e));
		} finally {
			setLoading(false);
		}
	}, [provider]);

	const checkNow = useCallback(async () => {
		if (!provider) {
			return;
		}

		setLoading(true);
		try {
			const data = await invoke<UpdateCheckResult>("check_updates_now", {
				provider,
			});
			setResult(data);
			setError(null);
		} catch (e) {
			setError(String(e));
		} finally {
			setLoading(false);
		}
	}, [provider]);

	usePluginRefreshEffect(refresh);

	useEffect(() => {
		const unlisten = listen<number>("plugin-updates-available", () => {
			if (provider === "claude") {
				refresh();
			}
		});
		return () => {
			unlisten.then((fn) => fn());
		};
	}, [provider, refresh]);

	return { result, loading, error, refresh, checkNow };
}

export interface OperationResult {
	pluginName: string;
	action: string;
	success: boolean;
	message: string;
}

export function usePluginOperations() {
	const [inProgress, setInProgress] = useState<Set<string>>(new Set());
	const [lastResult, setLastResult] = useState<OperationResult | null>(null);

	const withOperation = useCallback(
		async (
			key: string,
			pluginName: string,
			action: string,
			operation: () => Promise<unknown>,
		) => {
			setInProgress((prev) => new Set(prev).add(key));
			setLastResult(null);
			try {
				const result = await operation();
				setLastResult({
					pluginName,
					action,
					success: true,
					message:
						typeof result === "string" && result.trim()
							? result.trim()
							: `${action} completed`,
				});
			} catch (e) {
				setLastResult({
					pluginName,
					action,
					success: false,
					message: String(e),
				});
			} finally {
				setInProgress((prev) => {
					const next = new Set(prev);
					next.delete(key);
					return next;
				});
			}
		},
		[],
	);

	const installPlugin = useCallback(
		async (marketplace: Marketplace, plugin: MarketplacePlugin) => {
			await withOperation(
				`${plugin.provider}:${plugin.plugin_id}`,
				plugin.name,
				"Install",
				async () => {
					return await invoke("install_plugin", {
						provider: plugin.provider,
						name: plugin.name,
						marketplace: marketplace.name,
						marketplacePath: plugin.marketplace_path,
					});
				},
			);
		},
		[withOperation],
	);

	const removePlugin = useCallback(
		async (plugin: InstalledPlugin) => {
			await withOperation(
				installedPluginInstanceKey(plugin),
				plugin.name,
				"Remove",
				async () => {
					return await invoke("remove_plugin", {
						provider: plugin.provider,
						name: plugin.name,
						marketplace: plugin.marketplace,
						pluginId: plugin.plugin_id,
					});
				},
			);
		},
		[withOperation],
	);

	const enablePlugin = useCallback(
		async (plugin: InstalledPlugin) => {
			await withOperation(
				installedPluginInstanceKey(plugin),
				plugin.name,
				"Enable",
				async () => {
					return await invoke("enable_plugin", {
						provider: plugin.provider,
						name: plugin.name,
					});
				},
			);
		},
		[withOperation],
	);

	const disablePlugin = useCallback(
		async (plugin: InstalledPlugin) => {
			await withOperation(
				installedPluginInstanceKey(plugin),
				plugin.name,
				"Disable",
				async () => {
					return await invoke("disable_plugin", {
						provider: plugin.provider,
						name: plugin.name,
					});
				},
			);
		},
		[withOperation],
	);

	const updatePlugin = useCallback(
		async (update: PluginUpdate) => {
			await withOperation(
				pluginUpdateInstanceKey(update),
				update.name,
				"Update",
				async () => {
					return await invoke("update_plugin", {
						provider: update.provider,
						name: update.name,
						marketplace: update.marketplace,
						scope: update.scope,
						projectPath: update.project_path,
					});
				},
			);
		},
		[withOperation],
	);

	const clearResult = useCallback(() => {
		setLastResult(null);
	}, []);

	return {
		inProgress,
		lastResult,
		clearResult,
		installPlugin,
		removePlugin,
		enablePlugin,
		disablePlugin,
		updatePlugin,
	};
}

export function useBulkUpdate(provider: IntegrationProvider | null) {
	const [progress, setProgress] = useState<BulkUpdateProgress | null>(null);
	const [running, setRunning] = useState(false);

	useEffect(() => {
		const unlisten = listen<BulkUpdateProgress>("plugin-bulk-progress", (event) => {
			setProgress(event.payload);
		});
		return () => {
			unlisten.then((fn) => fn());
		};
	}, []);

	const updateAll = useCallback(async () => {
		if (!provider) {
			return;
		}

		setRunning(true);
		try {
			const result = await invoke<BulkUpdateProgress>("update_all_plugins", {
				provider,
			});
			setProgress(result);
		} finally {
			setRunning(false);
		}
	}, [provider]);

	const reset = useCallback(() => {
		setProgress(null);
	}, []);

	return { progress, running, updateAll, reset };
}
