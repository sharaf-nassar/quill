import { formatRetentionCutoff } from "../utils/retention";

/**
 * Which pruned-table surface the banner is describing (feature 014).
 *
 * The grounded scope is small on purpose. `range_to_duration` caps every
 * range-based reader at 30 days and the retention preset floor is 30 days, so
 * `get_code_stats`, `get_code_stats_history` and `get_llm_runtime_stats` can
 * never ask for pruned rows and must not carry this banner — claiming loss
 * where there is none is as dishonest as hiding loss where there is. Only
 * Session Search still needs the banner:
 *
 * - `session-search` — `get_batch_session_code_stats`, the per-hit code stats
 *   in the Session Search window.
 */
export type RetentionSurface = "session-search";

interface SurfaceCopy {
	/** What was lost on this surface, in product language. */
	readonly lead: string;
	/** The accepted, documented limitation this surface has to admit to. */
	readonly footnote: string;
}

const SURFACE_COPY: Record<RetentionSurface, SurfaceCopy> = {
	"session-search": {
		lead:
			"Code stats recorded before this date were pruned. Marked results have no line counts left, which is not the same as a session that changed no code.",
		// The Tantivy-hit-with-empty-drilldown case. Session search reads the
		// full-text index, which retention never touches, so hits survive their
		// own SQL drilldown.
		footnote:
			"Search itself is unaffected: it reads the full-text session index, which retention never prunes. A result can therefore outlive the database rows behind its details.",
	},
};

interface RetentionBannerProps {
	/** Retention watermark. The banner renders nothing when this is null. */
	cutoff: string | null;
	surface: RetentionSurface;
}

/**
 * States the retention cutoff wherever pruned-table data is rendered.
 *
 * Chrome-grey by design. `DESIGN.md` reserves green/amber/red for the severity
 * meter, and a boundary the user opted into is a fact about the instrument, not
 * an alarm — so this is a hairline-ruled note, not a warning strip.
 */
export function RetentionBanner({ cutoff, surface }: RetentionBannerProps) {
	if (!cutoff) {
		return null;
	}

	const copy = SURFACE_COPY[surface];
	const formatted = formatRetentionCutoff(cutoff);

	return (
		<div className="retention-banner" role="note">
			<div className="retention-banner-lead">
				<span className="retention-banner-tag">Retention</span>
				<span className="retention-banner-cutoff">Pruned before {formatted}</span>
			</div>
			<p className="retention-banner-body">{copy.lead}</p>
			<p className="retention-banner-footnote">{copy.footnote}</p>
		</div>
	);
}
