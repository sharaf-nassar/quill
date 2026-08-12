import { formatRetentionCutoff } from "../utils/retention";

interface RetentionBannerProps {
	/** Retention watermark. The banner renders nothing when this is null. */
	cutoff: string | null;
}

/**
 * States the retention cutoff wherever pruned-table data is rendered.
 *
 * Chrome-grey by design. `DESIGN.md` reserves green/amber/red for the severity
 * meter, and a boundary the user opted into is a fact about the instrument, not
 * an alarm — so this is a hairline-ruled note, not a warning strip.
 */
export function RetentionBanner({ cutoff }: RetentionBannerProps) {
	if (!cutoff) {
		return null;
	}

	const formatted = formatRetentionCutoff(cutoff);

	return (
		<div className="retention-banner" role="note">
			<div className="retention-banner-lead">
				<span className="retention-banner-tag">Retention</span>
				<span className="retention-banner-cutoff">Pruned before {formatted}</span>
			</div>
			<p className="retention-banner-body">
				Code stats recorded before this date were pruned. Marked results have no
				line counts left, which is not the same as a session that changed no code.
			</p>
			<p className="retention-banner-footnote">
				Search itself is unaffected: it reads the full-text session index, which
				retention never prunes. A result can therefore outlive the database rows
				behind its details.
			</p>
		</div>
	);
}
