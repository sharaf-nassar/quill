import type { IntegrationProvider, ObservedSubagentModelGroup } from "../types";

/** Format a number with thousand separators: 1234567 → "1,234,567" */
export function formatNumber(n: number): string {
	return n.toLocaleString("en-US");
}

interface DisplayAgentModelGroup {
	label: string;
	count: number;
	rank: number;
}

function agentModelFamily(provider: IntegrationProvider, modelId: string) {
	const matches = (family: string) =>
		new RegExp(`(^|[-_.])${family}($|[-_.])`, "i").test(modelId);
	const known = provider === "claude"
		? [["Opus", 0], ["Sonnet", 1], ["Haiku", 2]] as const
		: provider === "codex"
			? [["Sol", 0], ["Terra", 1], ["Luna", 2]] as const
			: [];
	const family = known.find(([label]) => matches(label));
	return family ? { label: family[0], rank: family[1] } : { label: modelId, rank: 100 };
}

export function formatObservedSubagentModels(
	provider: IntegrationProvider,
	count: number | null,
	groups: readonly ObservedSubagentModelGroup[] | null,
) {
	if (count === null || count <= 0) return null;
	const byLabel = new Map<string, DisplayAgentModelGroup>();
	let validTotal = 0;
	for (const group of groups ?? []) {
		if (!Number.isInteger(group.count) || group.count <= 0) continue;
		validTotal += group.count;
		const family = group.model_id === null
			? { label: "?", rank: Number.MAX_SAFE_INTEGER }
			: agentModelFamily(provider, group.model_id);
		const current = byLabel.get(family.label);
		if (current) current.count += group.count;
		else byLabel.set(family.label, { ...family, count: group.count });
	}
	if (validTotal > count) {
		byLabel.clear();
		validTotal = 0;
	}
	if (validTotal < count) {
		const unresolved = byLabel.get("?");
		if (unresolved) unresolved.count += count - validTotal;
		else byLabel.set("?", { label: "?", count: count - validTotal, rank: Number.MAX_SAFE_INTEGER });
	}
	const displayGroups = [...byLabel.values()].sort(
		(left, right) => left.rank - right.rank || left.label.localeCompare(right.label),
	);
	const text = displayGroups.map((group) => `${group.count}×${group.label}`).join(" · ");
	const breakdown = displayGroups
		.map((group) => `${group.count} ${group.label === "?" ? "unresolved model" : group.label} agent${group.count === 1 ? "" : "s"}`)
		.join(", ");
	return {
		text,
		ariaLabel: `${count} subagent${count === 1 ? "" : "s"} observed open: ${breakdown}`,
	};
}

export function resolveSessionMetrics(
	tokens: string,
	turns: string,
	observedOnly: boolean,
): { tokens: string; turns: string | null } {
	return observedOnly ? { tokens: "—", turns: null } : { tokens, turns };
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
