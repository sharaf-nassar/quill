import { useState, useMemo, useCallback } from "react";
import type { InstalledPlugin } from "../../types";
import { installedPluginInstanceKey } from "../../utils/plugins";

function projectName(path: string | null): string {
	if (!path) return "";
	const name = path.split("/").filter(Boolean).pop();
	return name === "~" ? "~" : name ?? path;
}

interface InstalledTabProps {
	plugins: InstalledPlugin[];
	operations: {
		inProgress: Set<string>;
		enablePlugin: (plugin: InstalledPlugin) => Promise<void>;
		disablePlugin: (plugin: InstalledPlugin) => Promise<void>;
		removePlugin: (plugin: InstalledPlugin) => Promise<void>;
	};
	onChanged: () => void;
}

function providerLabel(provider: InstalledPlugin["provider"]): string {
	return provider === "claude" ? "Claude" : "Codex";
}

function InstalledTab({ plugins, operations, onChanged }: InstalledTabProps) {
	const [search, setSearch] = useState("");

	const filtered = useMemo(() => {
		if (!search.trim()) return plugins;
		const q = search.toLowerCase();
		return plugins.filter(
			(p) =>
				p.name.toLowerCase().includes(q) ||
				(p.description?.toLowerCase().includes(q) ?? false),
		);
	}, [plugins, search]);

	const enabledCount = plugins.filter((p) => p.enabled).length;
	const disabledCount = plugins.length - enabledCount;

	const handleToggle = useCallback(
		async (plugin: InstalledPlugin) => {
			if (plugin.enabled) {
				await operations.disablePlugin(plugin);
			} else {
				await operations.enablePlugin(plugin);
			}
			onChanged();
		},
		[operations, onChanged],
	);

	const handleRemove = useCallback(
		async (plugin: InstalledPlugin) => {
			await operations.removePlugin(plugin);
			onChanged();
		},
		[operations, onChanged],
	);

	return (
		<div className="plugins-tab-content">
			<div className="plugins-search-bar">
				<input
					type="text"
					className="plugins-search-input"
					placeholder="Search installed plugins..."
					value={search}
					onChange={(e) => setSearch(e.target.value)}
				/>
			</div>
			<div className="plugins-list">
				{filtered.map((plugin) => {
					const instanceKey = installedPluginInstanceKey(plugin);
					const busy = operations.inProgress.has(instanceKey);
					const supportsToggle = plugin.provider === "claude";
					return (
						<div
							key={instanceKey}
							className={`plugins-row${!plugin.enabled ? " plugins-row--disabled" : ""}`}
						>
							<div className="plugins-row__info">
								<div className="plugins-row__header">
									<span className="plugins-provider-badge">
										{providerLabel(plugin.provider)}
									</span>
									<span className="plugins-row__name">{plugin.name}</span>
									{plugin.version && (
										<span className="plugins-row__version">
											{plugin.version}
										</span>
									)}
									<span
										className={`plugins-row__status${plugin.enabled ? " plugins-row__status--enabled" : " plugins-row__status--disabled"}`}
									>
										{plugin.enabled ? "enabled" : "disabled"}
									</span>
								</div>
								{plugin.description && (
									<div className="plugins-row__description">
										{plugin.description}
									</div>
								)}
								<div className="plugins-row__meta">
									<span>{plugin.marketplace}</span>
									<span className="plugins-scope-badge" title={plugin.project_path ?? undefined}>
										{plugin.scope}
										{plugin.scope === "project" && plugin.project_path && (
											<> &middot; {projectName(plugin.project_path)}</>
										)}
									</span>
								</div>
							</div>
							<div className="plugins-row__actions">
								{busy ? (
									<div className="plugins-spinner-wrap">
										<div className="plugins-spinner" />
										<span className="plugins-spinner-text">Working...</span>
									</div>
								) : (
									<>
										{supportsToggle && (
											<button
												className={`plugins-btn${plugin.enabled ? " plugins-btn--secondary" : " plugins-btn--enable"}`}
												onClick={() => handleToggle(plugin)}
											>
												{plugin.enabled ? "Disable" : "Enable"}
											</button>
										)}
										<button
											className="plugins-btn plugins-btn--danger"
											onClick={() => handleRemove(plugin)}
										>
											Remove
										</button>
									</>
								)}
							</div>
						</div>
					);
				})}
			</div>
			<div className="plugins-footer">
				{plugins.length} plugin{plugins.length !== 1 ? "s" : ""} installed
				&middot; {enabledCount} enabled &middot; {disabledCount} disabled
			</div>
		</div>
	);
}

export default InstalledTab;
