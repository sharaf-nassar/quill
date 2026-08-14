/**
 * Consumer-side degradation helpers for retention pruning (feature 014).
 *
 * Retention deletes transcript-derived `tool_actions` / `session_events` rows
 * older than the retention watermark. Nothing else in the schema is touched, so
 * a pre-cutoff session keeps its token totals and turn counts while losing the
 * rows behind its code stats and its sub-agent tree. Rendered naively that loss
 * looks like measured inactivity; these helpers exist so it renders as *absent*
 * instead of as zero.
 *
 * The boundary these helpers compare against is the **watermark**, not the
 * configured window. The window is a plan ("prune anything older than 90 days
 * the next time you ask me to"); the watermark is the fact ("everything older
 * than this instant is gone, and inserts below it are suppressed"). Only the
 * fact may be shown to a user as a cutoff date.
 *
 * Two deliberate conservatisms, both erring towards *not* marking:
 *
 * - A timestamp that cannot be parsed is reported as retained. The delete
 *   engine's conformance guard (`length(timestamp) = 24 AND timestamp LIKE
 *   '%Z'`) refuses to delete rows it cannot compare, so an unparseable
 *   timestamp really is retained.
 * - "Pruned" means *pre-cutoff*, never *provably empty*. Live rows
 *   (`source_key IS NULL`) and non-conforming timestamps survive below the
 *   watermark, so a marked figure may still be partially populated. All copy
 *   built on these helpers must say "may be incomplete", never "is empty".
 */

/**
 * Rendered in place of a numeric zero that is really absent data. An em dash
 * rather than "0" is the whole point of this module: a zero is a measurement,
 * a dash is an admission.
 */
export const PRUNED_PLACEHOLDER = "—";

/** Parse an ISO instant, returning null rather than NaN for anything unusable. */
function instant(timestamp: string | null | undefined): number | null {
	if (!timestamp) {
		return null;
	}
	const parsed = Date.parse(timestamp);
	return Number.isFinite(parsed) ? parsed : null;
}

/**
 * Format a retention cutoff for display. Date-only on purpose: the watermark
 * carries millisecond precision, but a user reasons about "before 12 Mar 2026",
 * and showing seconds invites the false belief that the boundary is exact for
 * every row (it is not — see the conformance-guard note above).
 */
export function formatRetentionCutoff(cutoff: string): string {
	const parsed = instant(cutoff);
	if (parsed === null) {
		return cutoff;
	}
	return new Date(parsed).toLocaleDateString(undefined, {
		year: "numeric",
		month: "short",
		day: "numeric",
	});
}

/** True when a valid timestamp falls before a valid retention cutoff. */
export function isPruned(
	timestamp: string | null | undefined,
	cutoff: string | null,
): boolean {
	const at = instant(timestamp);
	const boundary = instant(cutoff);
	return at !== null && boundary !== null && at < boundary;
}
