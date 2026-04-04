import { useState, useMemo, useCallback } from "react";
import type { Marketplace, MarketplacePlugin } from "../../types";
import { providerLabel } from "../../utils/providers";

interface BrowseTabProps {
	marketplaces: Marketplace[];
	operations: {
		inProgress: Set<string>;
		installPlugin: (
			marketplace: Marketplace,
			plugin: MarketplacePlugin,
		) => Promise<void>;
	};
	onChanged: () => void;
}

function BrowseTab({ marketplaces, operations, onChanged }: BrowseTabProps) {
	const [search, setSearch] = useState("");
	const [category, setCategory] = useState("all");
	const [collapsed, setCollapsed] = useState<Set<string>>(new Set());

	const categories = useMemo(() => {
		const cats = new Set<string>();
		for (const m of marketplaces) {
			for (const p of m.plugins) {
				if (p.category) cats.add(p.category);
			}
		}
		return ["all", ...Array.from(cats).sort()];
	}, [marketplaces]);

	const filteredMarketplaces = useMemo(() => {
		const q = search.toLowerCase();
		return marketplaces
			.map((m) => ({
				...m,
				plugins: m.plugins.filter((p) => {
					const matchesSearch =
						!q ||
						p.name.toLowerCase().includes(q) ||
						(p.description?.toLowerCase().includes(q) ?? false);
					const matchesCategory =
						category === "all" || p.category === category;
					return matchesSearch && matchesCategory;
				}),
			}))
			.filter((m) => m.plugins.length > 0);
	}, [marketplaces, search, category]);

	const totalPlugins = marketplaces.reduce((n, m) => n + m.plugins.length, 0);
	const installedCount = marketplaces.reduce(
		(n, m) => n + m.plugins.filter((p) => p.installed).length,
		0,
	);

	const toggleCollapse = useCallback((name: string) => {
		setCollapsed((prev) => {
			const next = new Set(prev);
			if (next.has(name)) {
				next.delete(name);
			} else {
				next.add(name);
			}
			return next;
		});
	}, []);

	const handleInstall = useCallback(
		async (marketplace: Marketplace, plugin: MarketplacePlugin) => {
			await operations.installPlugin(marketplace, plugin);
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
					placeholder="Search all marketplaces..."
					value={search}
					onChange={(e) => setSearch(e.target.value)}
				/>
				<select
					className="plugins-filter-select"
					value={category}
					onChange={(e) => setCategory(e.target.value)}
				>
					{categories.map((c) => (
						<option key={c} value={c}>
							{c === "all" ? "Category: All" : c}
						</option>
					))}
				</select>
			</div>
			<div className="plugins-list">
				{filteredMarketplaces.map((marketplace) => (
					<div key={marketplace.name} className="plugins-marketplace-group">
						<button
							className="plugins-marketplace-group__header"
							onClick={() => toggleCollapse(marketplace.name)}
						>
							<span className="plugins-marketplace-group__name">
								<span className="plugins-provider-badge">
									{providerLabel(marketplace.provider)}
								</span>
								{marketplace.name}
							</span>
							<span className="plugins-marketplace-group__count">
								{marketplace.plugins.length} plugin
								{marketplace.plugins.length !== 1 ? "s" : ""}
							</span>
							<span className="plugins-marketplace-group__toggle">
								{collapsed.has(marketplace.name) ? "\u25BE" : "\u25B4"}
							</span>
						</button>
						{!collapsed.has(marketplace.name) &&
							marketplace.plugins.map((plugin) => {
								const busy = operations.inProgress.has(
									`${plugin.provider}:${plugin.plugin_id}`,
								);
								return (
									<div
										key={`${plugin.provider}:${plugin.plugin_id}`}
										className={`plugins-row${plugin.installed ? " plugins-row--installed" : ""}`}
									>
										<div className="plugins-row__info">
											<div className="plugins-row__header">
												<span className="plugins-row__name">
													{plugin.name}
												</span>
												{plugin.version && (
													<span className="plugins-row__version">
														{plugin.version}
													</span>
												)}
												{plugin.category && (
													<span className="plugins-row__category">
														{plugin.category}
													</span>
												)}
												{plugin.installed && (
													<span className="plugins-row__installed-badge">
														installed
													</span>
												)}
											</div>
											{plugin.description && (
												<div className="plugins-row__description">
													{plugin.description}
												</div>
											)}
											{plugin.author && (
												<div className="plugins-row__meta">
													by {plugin.author}
												</div>
											)}
										</div>
										<div className="plugins-row__actions">
											{plugin.installed ? (
												<span className="plugins-installed-check">
													Installed &#10003;
												</span>
											) : busy ? (
												<div className="plugins-spinner-wrap">
													<div className="plugins-spinner" />
													<span className="plugins-spinner-text">
														Installing...
													</span>
												</div>
												) : (
													<button
														className="plugins-btn plugins-btn--install"
														onClick={() => handleInstall(marketplace, plugin)}
													>
														Install
													</button>
											)}
										</div>
									</div>
								);
							})}
					</div>
				))}
			</div>
			<div className="plugins-footer">
				{totalPlugins} plugin{totalPlugins !== 1 ? "s" : ""} across{" "}
				{marketplaces.length} marketplace
				{marketplaces.length !== 1 ? "s" : ""} &middot; {installedCount}{" "}
				installed
			</div>
		</div>
	);
}

export default BrowseTab;
