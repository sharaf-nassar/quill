import type { RangeType } from "../types";

export function formatTime(timestamp: string, range: RangeType): string {
	const d = new Date(timestamp);
	if (range === "1h" || range === "24h") {
		return d.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
	}
	if (range === "7d") {
		return d.toLocaleDateString([], { weekday: "short", hour: "2-digit" });
	}
	return d.toLocaleDateString([], { month: "short", day: "numeric" });
}

interface TimestampedRecord {
	timestamp: string;
}

export function dedupeTickLabels(
	data: TimestampedRecord[],
	formatter: (v: string) => string,
): Set<number> {
	const seen = new Set<string>();
	const allowed = new Set<number>();
	for (let i = 0; i < data.length; i++) {
		const label = formatter(data[i].timestamp);
		if (!seen.has(label)) {
			seen.add(label);
			allowed.add(i);
		}
	}
	return allowed;
}

/** Minimum gap (ms) before we append a "now" anchor to extend the X-axis */
const NOW_ANCHOR_THRESHOLD_MS = 2 * 60 * 1000;

export function anchorToNow<T extends { timestamp: string }>(
	points: T[],
	defaults: Omit<T, "timestamp">,
): T[] {
	if (points.length === 0) return points;

	const lastTs = new Date(points[points.length - 1].timestamp).getTime();
	const now = Date.now();

	if (now - lastTs > NOW_ANCHOR_THRESHOLD_MS) {
		return [
			...points,
			{ ...defaults, timestamp: new Date(now).toISOString() } as T,
		];
	}
	return points;
}

export function getAreaColor(data: { utilization: number }[]): string {
	if (!data || data.length === 0) return "#34d399";
	const latest = data[data.length - 1].utilization;
	if (latest >= 80) return "#f87171";
	if (latest >= 50) return "#fbbf24";
	return "#34d399";
}
