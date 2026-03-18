import { useState, useMemo, useCallback } from "react";
import type { InstalledPlugin } from "../../types";

interface InstalledTabProps {
	plugins: InstalledPlugin[];
	operations: {
		inProgress: Set<string>;
		enablePlugin: (name: string) => Promise<void>;
		disablePlugin: (name: string) => Promise<void>;
		removePlugin: (name: string, marketplace: string) => Promise<void>;
	};
	onChanged: () => void;
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
				await operations.disablePlugin(plugin.name);
			} else {
				await operations.enablePlugin(plugin.name);
			}
			onChanged();
		},
		[operations, onChanged],
	);

	const handleRemove = useCallback(
		async (plugin: InstalledPlugin) => {
			await operations.removePlugin(plugin.name, plugin.marketplace);
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
					const busy = operations.inProgress.has(plugin.name);
					return (
						<div
							key={`${plugin.name}@${plugin.marketplace}`}
							className={`plugins-row${!plugin.enabled ? " plugins-row--disabled" : ""}`}
						>
							<div className="plugins-row__info">
								<div className="plugins-row__header">
									<span className="plugins-row__name">{plugin.name}</span>
									<span className="plugins-row__version">{plugin.version}</span>
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
									{plugin.marketplace} &middot; {plugin.scope} scope
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
										<button
											className={`plugins-btn${plugin.enabled ? " plugins-btn--secondary" : " plugins-btn--enable"}`}
											onClick={() => handleToggle(plugin)}
										>
											{plugin.enabled ? "Disable" : "Enable"}
										</button>
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
