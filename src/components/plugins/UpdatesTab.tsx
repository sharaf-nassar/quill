import { useCallback } from "react";
import type { UpdateCheckResult, BulkUpdateProgress } from "../../types";

function projectName(path: string | null): string {
	if (!path) return "";
	const name = path.split("/").filter(Boolean).pop();
	return name === "~" ? "~" : name ?? path;
}

function formatTime(ts: string | null): string {
	if (!ts) return "Never";
	const date = new Date(ts);
	const diff = Date.now() - date.getTime();
	const mins = Math.floor(diff / 60_000);
	if (mins < 1) return "Just now";
	if (mins < 60) return `${mins} min ago`;
	const hours = Math.floor(mins / 60);
	return `${hours}h ago`;
}

interface UpdatesTabProps {
	updates: {
		result: UpdateCheckResult;
		loading: boolean;
		checkNow: () => Promise<void>;
	};
	operations: {
		inProgress: Set<string>;
		updatePlugin: (name: string, marketplace: string, scope: string, projectPath: string | null) => Promise<void>;
	};
	bulkUpdate: {
		progress: BulkUpdateProgress | null;
		running: boolean;
		updateAll: () => Promise<void>;
		reset: () => void;
	};
	onChanged: () => void;
}

function UpdatesTab({
	updates,
	operations,
	bulkUpdate,
	onChanged,
}: UpdatesTabProps) {
	const { result, loading } = updates;
	const { progress, running } = bulkUpdate;

	const handleUpdate = useCallback(
		async (name: string, marketplace: string, scope: string, projectPath: string | null) => {
			await operations.updatePlugin(name, marketplace, scope, projectPath);
			onChanged();
		},
		[operations, onChanged],
	);

	const handleUpdateAll = useCallback(async () => {
		await bulkUpdate.updateAll();
		onChanged();
	}, [bulkUpdate, onChanged]);

	return (
		<div className="plugins-tab-content">
			<div className="plugins-updates-header">
				<div className="plugins-updates-header__info">
					<span className="plugins-updates-header__count">
						{result.plugin_updates.length} update
						{result.plugin_updates.length !== 1 ? "s" : ""} available
					</span>
					<span className="plugins-updates-header__time">
						Last checked {formatTime(result.last_checked)}
					</span>
				</div>
				<div className="plugins-updates-header__actions">
					<button
						className="plugins-btn plugins-btn--secondary"
						onClick={updates.checkNow}
						disabled={loading}
					>
						{loading ? "Checking..." : "Check Now"}
					</button>
					<button
						className="plugins-btn plugins-btn--install"
						onClick={handleUpdateAll}
						disabled={running || result.plugin_updates.length === 0}
					>
						{running ? "Updating..." : "Update All"}
					</button>
				</div>
			</div>

			{progress && running && (
				<div className="plugins-bulk-progress">
					<div className="plugins-bulk-progress__header">
						<span>Updating plugins...</span>
						<span className="plugins-bulk-progress__count">
							{progress.completed} / {progress.total}
						</span>
					</div>
					<div className="plugins-progress-bar">
						<div
							className="plugins-progress-bar__fill"
							style={{
								width: `${progress.total > 0 ? (progress.completed / progress.total) * 100 : 0}%`,
							}}
						/>
					</div>
					<div className="plugins-bulk-progress__items">
						{progress.results.map((item) => (
							<div
								key={item.name}
								className={`plugins-bulk-progress__item plugins-bulk-progress__item--${item.status}`}
							>
								{item.status === "success" ? "\u2713" : "\u2717"}{" "}
								{item.name}
								{item.error && (
									<span className="plugins-bulk-progress__error">
										{item.error}
									</span>
								)}
							</div>
						))}
						{progress.current_plugin && (
							<div className="plugins-bulk-progress__item plugins-bulk-progress__item--active">
								<div className="plugins-spinner plugins-spinner--small" />{" "}
								{progress.current_plugin}
							</div>
						)}
					</div>
				</div>
			)}

			<div className="plugins-list">
				{result.plugin_updates.length === 0 && !running && (
					<div className="plugins-empty">
						{result.last_checked
							? "All plugins are up to date \u2713"
							: "Click Check Now for updates\u2026"}
					</div>
				)}
				{result.plugin_updates.map((update) => {
					const busy =
						running || operations.inProgress.has(update.name);
					return (
						<div key={update.name} className="plugins-row">
							<div className="plugins-row__info">
								<div className="plugins-row__header">
									<span className="plugins-row__name">{update.name}</span>
									<span className="plugins-row__version">
										{update.current_version}
									</span>
									<span className="plugins-row__arrow">&rarr;</span>
									<span className="plugins-row__new-version">
										{update.available_version}
									</span>
								</div>
								<div className="plugins-row__meta">
									<span>{update.marketplace}</span>
									<span className="plugins-scope-badge" title={update.project_path ?? undefined}>
										{update.scope}
										{update.scope === "project" && update.project_path && (
											<> &middot; {projectName(update.project_path)}</>
										)}
									</span>
								</div>
							</div>
							<div className="plugins-row__actions">
								{busy ? (
									<div className="plugins-spinner-wrap">
										<div className="plugins-spinner" />
										<span className="plugins-spinner-text">
											Updating...
										</span>
									</div>
								) : (
									<button
										className="plugins-btn plugins-btn--install"
										onClick={() =>
											handleUpdate(update.name, update.marketplace, update.scope, update.project_path)
										}
									>
										Update
									</button>
								)}
							</div>
						</div>
					);
				})}
			</div>
		</div>
	);
}

export default UpdatesTab;
