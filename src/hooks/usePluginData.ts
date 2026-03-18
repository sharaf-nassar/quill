import { useState, useEffect, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type {
	InstalledPlugin,
	Marketplace,
	UpdateCheckResult,
	BulkUpdateProgress,
} from "../types";

export function useInstalledPlugins() {
	const [plugins, setPlugins] = useState<InstalledPlugin[]>([]);
	const [loading, setLoading] = useState(true);
	const [error, setError] = useState<string | null>(null);

	const refresh = useCallback(async () => {
		setLoading(true);
		try {
			const data = await invoke<InstalledPlugin[]>("get_installed_plugins");
			setPlugins(data);
			setError(null);
		} catch (e) {
			setError(String(e));
		} finally {
			setLoading(false);
		}
	}, []);

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

	return { plugins, loading, error, refresh };
}

export function useMarketplaces() {
	const [marketplaces, setMarketplaces] = useState<Marketplace[]>([]);
	const [loading, setLoading] = useState(true);
	const [error, setError] = useState<string | null>(null);

	const refresh = useCallback(async () => {
		setLoading(true);
		try {
			const data = await invoke<Marketplace[]>("get_marketplaces");
			setMarketplaces(data);
			setError(null);
		} catch (e) {
			setError(String(e));
		} finally {
			setLoading(false);
		}
	}, []);

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

	return { marketplaces, loading, error, refresh };
}

export function useAvailableUpdates() {
	const [result, setResult] = useState<UpdateCheckResult>({
		plugin_updates: [],
		last_checked: null,
		next_check: null,
	});
	const [loading, setLoading] = useState(true);
	const [error, setError] = useState<string | null>(null);

	const refresh = useCallback(async () => {
		setLoading(true);
		try {
			const data = await invoke<UpdateCheckResult>("get_available_updates");
			setResult(data);
			setError(null);
		} catch (e) {
			setError(String(e));
		} finally {
			setLoading(false);
		}
	}, []);

	const checkNow = useCallback(async () => {
		setLoading(true);
		try {
			const data = await invoke<UpdateCheckResult>("check_updates_now");
			setResult(data);
			setError(null);
		} catch (e) {
			setError(String(e));
		} finally {
			setLoading(false);
		}
	}, []);

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

	useEffect(() => {
		const unlisten = listen<number>("plugin-updates-available", () => {
			refresh();
		});
		return () => {
			unlisten.then((fn) => fn());
		};
	}, [refresh]);

	return { result, loading, error, refresh, checkNow };
}

export function usePluginOperations() {
	const [inProgress, setInProgress] = useState<Set<string>>(new Set());

	const withOperation = useCallback(
		async (pluginName: string, operation: () => Promise<unknown>) => {
			setInProgress((prev) => new Set(prev).add(pluginName));
			try {
				await operation();
			} finally {
				setInProgress((prev) => {
					const next = new Set(prev);
					next.delete(pluginName);
					return next;
				});
			}
		},
		[],
	);

	const installPlugin = useCallback(
		async (name: string, marketplace: string) => {
			await withOperation(name, async () => {
				await invoke("install_plugin", { name, marketplace });
			});
		},
		[withOperation],
	);

	const removePlugin = useCallback(
		async (name: string, marketplace: string) => {
			await withOperation(name, async () => {
				await invoke("remove_plugin", { name, marketplace });
			});
		},
		[withOperation],
	);

	const enablePlugin = useCallback(
		async (name: string) => {
			await withOperation(name, async () => {
				await invoke("enable_plugin", { name });
			});
		},
		[withOperation],
	);

	const disablePlugin = useCallback(
		async (name: string) => {
			await withOperation(name, async () => {
				await invoke("disable_plugin", { name });
			});
		},
		[withOperation],
	);

	const updatePlugin = useCallback(
		async (name: string, marketplace: string) => {
			await withOperation(name, async () => {
				await invoke("update_plugin", { name, marketplace });
			});
		},
		[withOperation],
	);

	return {
		inProgress,
		installPlugin,
		removePlugin,
		enablePlugin,
		disablePlugin,
		updatePlugin,
	};
}

export function useBulkUpdate() {
	const [progress, setProgress] = useState<BulkUpdateProgress | null>(null);
	const [running, setRunning] = useState(false);

	useEffect(() => {
		const unlisten = listen<BulkUpdateProgress>(
			"plugin-bulk-progress",
			(event) => {
				setProgress(event.payload);
			},
		);
		return () => {
			unlisten.then((fn) => fn());
		};
	}, []);

	const updateAll = useCallback(async () => {
		setRunning(true);
		try {
			const result = await invoke<BulkUpdateProgress>("update_all_plugins");
			setProgress(result);
		} finally {
			setRunning(false);
		}
	}, []);

	const reset = useCallback(() => {
		setProgress(null);
	}, []);

	return { progress, running, updateAll, reset };
}
