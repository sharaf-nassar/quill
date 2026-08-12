import type { IntegrationProvider, ObservedSessionAgent } from "../types";

/** Five minutes covers a long turn while bounding missing-hook crash fallback. */
const SESSION_LIVE_WINDOW_MS = 5 * 60_000;

export function isSessionLive(
	lastActive: string,
	endedAt: string | null,
	nowMs: number,
): boolean {
	const activity = new Date(lastActive).getTime();
	if (!Number.isFinite(activity)) return false;
	const ended = endedAt === null ? Number.NaN : new Date(endedAt).getTime();
	return !(Number.isFinite(ended) && ended >= activity)
		&& nowMs - activity < SESSION_LIVE_WINDOW_MS;
}

/** Format a number with thousand separators: 1234567 → "1,234,567" */
export function formatNumber(n: number): string {
	return n.toLocaleString("en-US");
}

/** Compact recency for narrow widget columns: `now`, `42m`, `3h`, `2d`. */
export function formatRecency(timestamp: string, nowMs: number): string {
	const then = new Date(timestamp).getTime();
	if (!Number.isFinite(then)) return "—";
	const minutes = Math.floor((nowMs - then) / 60_000);
	if (minutes < 1) return "now";
	if (minutes < 60) return `${minutes}m`;
	const hours = Math.floor(minutes / 60);
	if (hours < 24) return `${hours}h`;
	return `${Math.floor(hours / 24)}d`;
}

function agentModelFamily(provider: IntegrationProvider, modelId: string) {
	const matches = (family: string) =>
		new RegExp(`(^|[-_.])${family}($|[-_.])`, "i").test(modelId);
	const known = provider === "claude"
		? [["Opus", 0], ["Sonnet", 1], ["Haiku", 2], ["Fable", 3]] as const
		: provider === "codex"
			? [["Sol", 0], ["Terra", 1], ["Luna", 2]] as const
			: [];
	const family = known.find(([label]) => matches(label));
	return family ? { label: family[0], rank: family[1] } : { label: modelId, rank: 100 };
}

export function formatExtrapolatedRuntime(
	runtimeSecs: number | null,
	runtimeAsOfMs: number | null,
	rate: number,
	nowMs: number,
	format: (secs: number | null) => string = formatDurationSecs,
): string {
	if (runtimeSecs === null) return "—";
	const elapsedSecs = runtimeAsOfMs === null
		? 0
		: Math.max(0, nowMs - runtimeAsOfMs) / 1_000;
	return format(runtimeSecs + elapsedSecs * rate);
}

export function formatObservedSessionAgents(
	provider: IntegrationProvider,
	agents: readonly ObservedSessionAgent[] | null,
	runtimeAsOfMs: number | null,
	nowMs: number,
) {
	return (agents ?? []).map((agent) => {
		const family = agent.model_id === null
			? agent.agent_type
				? { label: agent.agent_type, rank: 100 }
				: { label: "?", rank: Number.MAX_SAFE_INTEGER }
			: agentModelFamily(provider, agent.model_id);
		const runtime = formatExtrapolatedRuntime(
			agent.runtime_secs,
			runtimeAsOfMs,
			agent.runtime_active ? 1 : 0,
			nowMs,
		);
		const identity = agent.model_id ?? agent.agent_type ?? "Unknown model";
		return {
			agentId: agent.agent_id,
			model: family.label,
			runtime,
			ariaLabel: `${identity}, agent ${agent.agent_id}, ${runtime === "—" ? "runtime unavailable" : `${runtime} active runtime`}`,
			rank: family.rank,
		};
	}).sort((left, right) =>
		left.rank - right.rank
		|| left.model.localeCompare(right.model)
		|| left.agentId.localeCompare(right.agentId)
	).map(({ rank: _, ...agent }) => agent);
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

/** Format seconds as compact days:hours:minutes, flooring partial minutes. */
export function formatClockDurationSecs(secs: number | null): string {
	if (secs === null) return "—";
	const { days, hours, minutes } = clockParts(secs);
	if (days > 0) return `${days}:${String(hours).padStart(2, "0")}:${String(minutes).padStart(2, "0")}`;
	if (hours > 0) return `${hours}:${String(minutes).padStart(2, "0")}`;
	return `${minutes} m`;
}

/** Format seconds as an adaptive clock, flooring partial seconds. */
export function formatAdaptiveClockDurationSecs(secs: number | null): string {
	if (secs === null) return "—";
	const { days, hours, minutes, seconds } = clockParts(secs);
	const mm = String(minutes).padStart(2, "0");
	const ss = String(seconds).padStart(2, "0");
	if (days > 0) return `${days}:${String(hours).padStart(2, "0")}:${mm}:${ss}`;
	if (hours > 0) return `${hours}:${mm}:${ss}`;
	if (minutes > 0) return `${minutes}:${ss}`;
	return `${seconds}s`;
}

function clockParts(secs: number) {
	const totalSeconds = Math.floor(secs);
	return {
		days: Math.floor(totalSeconds / 86_400),
		hours: Math.floor(totalSeconds / 3_600) % 24,
		minutes: Math.floor(totalSeconds / 60) % 60,
		seconds: totalSeconds % 60,
	};
}
