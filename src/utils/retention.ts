/**
 * Consumer-side degradation helpers for retention pruning (feature 014).
 *
 * Retention deletes transcript-derived `tool_actions` / `session_events` rows
 * older than the retention watermark. Nothing else in the schema is touched, so
 * a pre-cutoff session keeps its token totals and turn counts while losing the
 * rows behind its code stats and its sub-agent tree. Rendered naively that loss
 * looks like a quiet month; these helpers exist so it renders as *absent*
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
 * How a time span sits against the retention cutoff.
 *
 * - `retained` — entirely at or after the cutoff, or retention is off.
 * - `straddles` — starts before the cutoff and continues past it, so the
 *   figure mixes pruned and retained rows.
 * - `pruned` — entirely before the cutoff.
 */
export type RetentionSpan = "retained" | "straddles" | "pruned";

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

/**
 * Classify a `[firstSeen, lastActive]` span against the cutoff. A null cutoff
 * (retention never run, or disabled) classifies everything as `retained`.
 */
export function retentionSpanFor(
	firstSeen: string | null | undefined,
	lastActive: string | null | undefined,
	cutoff: string | null,
): RetentionSpan {
	const boundary = instant(cutoff);
	if (boundary === null) {
		return "retained";
	}

	const end = instant(lastActive);
	const start = instant(firstSeen) ?? end;
	if (end === null || start === null) {
		return "retained";
	}

	if (end < boundary) {
		return "pruned";
	}
	if (start < boundary) {
		return "straddles";
	}
	return "retained";
}

/** Single-instant form of {@link retentionSpanFor} — true when pre-cutoff. */
export function isPruned(
	timestamp: string | null | undefined,
	cutoff: string | null,
): boolean {
	return retentionSpanFor(timestamp, timestamp, cutoff) === "pruned";
}

/** A row paired with its classification against the retention cutoff. */
export interface RetentionMarked<T> {
	readonly row: T;
	readonly span: RetentionSpan;
}

/**
 * Mark every row in a time-ordered range against the cutoff, returning a new
 * array of `{ row, span }` pairs — the inputs are never mutated.
 *
 * This is the "mark the pre-cutoff range rather than draw it as zeros"
 * treatment in its general form. Callers that would rather drop the pre-cutoff
 * rows entirely can filter the result on `span !== "pruned"`; marking is
 * preferred because a truncated axis hides that anything was ever there.
 */
export function markPrunedRange<T>(
	rows: readonly T[],
	cutoff: string | null,
	boundsOf: (row: T) => readonly [string, string],
): readonly RetentionMarked<T>[] {
	return rows.map((row) => {
		const [from, to] = boundsOf(row);
		return { row, span: retentionSpanFor(from, to, cutoff) };
	});
}
