/** Format a number with thousand separators: 1234567 → "1,234,567" */
export function formatNumber(n: number): string {
	return n.toLocaleString("en-US");
}

export function formatObservedSubagentCount(count: number | null) {
	if (count === null || count <= 0) return null;
	return {
		text: `+${count}`,
		ariaLabel: `${count} subagent${count === 1 ? "" : "s"} observed open`,
	};
}

/** Format a byte count using binary units. */
export function formatBytes(bytes: number): string {
	if (bytes < 1024) {
		return `${formatNumber(bytes)} B`;
	}
	const units = ["KB", "MB", "GB", "TB"];
	let scaled = bytes / 1024;
	let unitIndex = 0;
	while (scaled >= 1024 && unitIndex < units.length - 1) {
		scaled /= 1024;
		unitIndex += 1;
	}
	return `${scaled >= 10 ? scaled.toFixed(0) : scaled.toFixed(1)} ${units[unitIndex]}`;
}

/** Format seconds to human-readable: 45 → "45s", 125 → "2m 5s", 3661 → "1h 1m", 90000 → "1d 1h" */
export function formatDurationSecs(secs: number | null): string {
	if (secs === null) return "—";
	if (secs < 60) return `${Math.round(secs)}s`;
	if (secs < 3600) {
		const m = Math.floor(secs / 60);
		const s = Math.round(secs % 60);
		return s === 0 ? `${m}m` : `${m}m ${s}s`;
	}
	if (secs < 86400) {
		const h = Math.floor(secs / 3600);
		const m = Math.round((secs % 3600) / 60);
		return m === 0 ? `${h}h` : `${h}h ${m}m`;
	}
	const d = Math.floor(secs / 86400);
	const h = Math.round((secs % 86400) / 3600);
	return h === 0 ? `${d}d` : `${d}d ${h}h`;
}
