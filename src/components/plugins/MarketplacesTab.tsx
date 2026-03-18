import { useState, useCallback } from "react";
import type { Marketplace } from "../../types";

function formatLastUpdated(ts: string | null): string {
	if (!ts) return "Never";
	const date = new Date(ts);
	const diff = Date.now() - date.getTime();
	const hours = Math.floor(diff / 3_600_000);
	if (hours < 1) return "< 1h ago";
	if (hours < 24) return `${hours}h ago`;
	const days = Math.floor(hours / 24);
	return `${days}d ago`;
}

interface MarketplacesTabProps {
	marketplaces: Marketplace[];
	onAdd: (repo: string) => Promise<void>;
	onRemove: (name: string) => Promise<void>;
	onRefresh: (name: string) => Promise<void>;
	onRefreshAll: () => Promise<void>;
}

function MarketplacesTab({
	marketplaces,
	onAdd,
	onRemove,
	onRefresh,
	onRefreshAll,
}: MarketplacesTabProps) {
	const [addInput, setAddInput] = useState("");
	const [adding, setAdding] = useState(false);
	const [refreshing, setRefreshing] = useState<Set<string>>(new Set());
	const [refreshingAll, setRefreshingAll] = useState(false);

	const handleAdd = useCallback(async () => {
		if (!addInput.trim()) return;
		setAdding(true);
		try {
			await onAdd(addInput.trim());
			setAddInput("");
		} finally {
			setAdding(false);
		}
	}, [addInput, onAdd]);

	const handleRefresh = useCallback(
		async (name: string) => {
			setRefreshing((prev) => new Set(prev).add(name));
			try {
				await onRefresh(name);
			} finally {
				setRefreshing((prev) => {
					const next = new Set(prev);
					next.delete(name);
					return next;
				});
			}
		},
		[onRefresh],
	);

	const handleRefreshAll = useCallback(async () => {
		setRefreshingAll(true);
		try {
			await onRefreshAll();
		} finally {
			setRefreshingAll(false);
		}
	}, [onRefreshAll]);

	const handleKeyDown = useCallback(
		(e: React.KeyboardEvent) => {
			if (e.key === "Enter") handleAdd();
		},
		[handleAdd],
	);

	return (
		<div className="plugins-tab-content">
			<div className="plugins-search-bar">
				<input
					type="text"
					className="plugins-search-input"
					placeholder="GitHub repo (e.g., org/marketplace-repo)..."
					value={addInput}
					onChange={(e) => setAddInput(e.target.value)}
					onKeyDown={handleKeyDown}
					disabled={adding}
				/>
				<button
					className="plugins-btn plugins-btn--install"
					onClick={handleAdd}
					disabled={adding || !addInput.trim()}
				>
					{adding ? "Adding..." : "+ Add"}
				</button>
			</div>
			<div className="plugins-list">
				{marketplaces.map((marketplace) => {
					const busy = refreshing.has(marketplace.name);
					const installedCount = marketplace.plugins.filter(
						(p) => p.installed,
					).length;
					return (
						<div
							key={marketplace.name}
							className="plugins-marketplace-card"
						>
							<div className="plugins-marketplace-card__header">
								<div className="plugins-marketplace-card__info">
									<div className="plugins-marketplace-card__name-row">
										<span className="plugins-marketplace-card__name">
											{marketplace.name}
										</span>
										<span className="plugins-marketplace-card__source">
											{marketplace.source_type}
										</span>
									</div>
									<div className="plugins-marketplace-card__repo">
										{marketplace.repo}
									</div>
								</div>
								<div className="plugins-marketplace-card__actions">
									<span className="plugins-marketplace-card__updated">
										Updated {formatLastUpdated(marketplace.last_updated)}
									</span>
									{busy ? (
										<div className="plugins-spinner-wrap">
											<div className="plugins-spinner" />
											<span className="plugins-spinner-text">
												Refreshing...
											</span>
										</div>
									) : (
										<>
											<button
												className="plugins-btn plugins-btn--secondary"
												onClick={() => handleRefresh(marketplace.name)}
											>
												Refresh
											</button>
											<button
												className="plugins-btn plugins-btn--danger"
												onClick={() => onRemove(marketplace.name)}
											>
												Remove
											</button>
										</>
									)}
								</div>
							</div>
							<div className="plugins-marketplace-card__stats">
								<span>{marketplace.plugins.length} plugins</span>
								<span>{installedCount} installed</span>
							</div>
						</div>
					);
				})}
			</div>
			<div className="plugins-footer">
				<span>
					{marketplaces.length} marketplace
					{marketplaces.length !== 1 ? "s" : ""} configured
				</span>
				<button
					className="plugins-btn-link"
					onClick={handleRefreshAll}
					disabled={refreshingAll}
				>
					{refreshingAll ? "Refreshing..." : "Refresh All"}
				</button>
			</div>
		</div>
	);
}

export default MarketplacesTab;
