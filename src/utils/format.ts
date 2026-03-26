/** Format a number with thousand separators: 1234567 → "1,234,567" */
export function formatNumber(n: number): string {
	return n.toLocaleString("en-US");
}

/** Format seconds to human-readable: 45 → "45s", 125 → "2m 5s", 120 → "2m" */
export function formatDurationSecs(secs: number | null): string {
	if (secs === null) return "—";
	if (secs < 60) return `${Math.round(secs)}s`;
	const m = Math.floor(secs / 60);
	const s = Math.round(secs % 60);
	return s === 0 ? `${m}m` : `${m}m ${s}s`;
}
