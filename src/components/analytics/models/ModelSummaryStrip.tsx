import type { ModelAnalyticsSummary } from "../../../types";

const COUNT_FORMATTER = new Intl.NumberFormat("en-US");
const PERCENT_FORMATTER = new Intl.NumberFormat("en-US", {
	maximumFractionDigits: 1,
});

interface ModelSummaryStripProps {
	summary: ModelAnalyticsSummary | null;
	isLoading?: boolean;
	isRefreshing?: boolean;
}

function formatCoverage(value: number | null): string {
	if (value === null) return "Unavailable";
	return `${PERCENT_FORMATTER.format(value)}%`;
}

function ModelSummaryStrip({
	summary,
	isLoading = false,
	isRefreshing = false,
}: ModelSummaryStripProps) {
	const initialLoading = isLoading && summary === null;
	const refreshing = isRefreshing || (isLoading && summary !== null);
	const empty =
		summary !== null &&
		summary.totalTokens === 0 &&
		summary.distinctModels === 0 &&
		summary.multiModelSessions === 0;
	const stateClasses = [
		"model-summary-strip",
		initialLoading ? "model-summary-strip--loading" : null,
		refreshing ? "model-summary-strip--refreshing" : null,
		empty ? "model-summary-strip--empty" : null,
	]
		.filter(Boolean)
		.join(" ");

	if (initialLoading) {
		return (
			<section className={stateClasses} aria-label="Model usage summary">
				<p className="model-summary-strip__status" role="status">
					{"Loading model summary\u2026"}
				</p>
				<dl
					className="model-summary-strip__metrics"
					aria-busy="true"
					aria-hidden="true"
				>
					<div className="model-summary-strip__metric">
						<dt className="model-summary-strip__label">Attributed coverage</dt>
						<dd className="model-summary-strip__value model-summary-strip__value--skeleton">
							—
						</dd>
					</div>
					<div className="model-summary-strip__metric">
						<dt className="model-summary-strip__label">Distinct models</dt>
						<dd className="model-summary-strip__value model-summary-strip__value--skeleton">
							—
						</dd>
					</div>
					<div className="model-summary-strip__metric">
						<dt className="model-summary-strip__label">Multi-model sessions</dt>
						<dd className="model-summary-strip__value model-summary-strip__value--skeleton">
							—
						</dd>
					</div>
				</dl>
			</section>
		);
	}

	if (summary === null) {
		return (
			<section
				className={`${stateClasses} model-summary-strip--unavailable`}
				aria-label="Model usage summary"
			>
				<p className="model-summary-strip__status" role="status">
					Model summary unavailable.
				</p>
			</section>
		);
	}

	const coverageDetail =
		summary.attributedCoveragePercent === null
			? "Coverage denominator is zero"
			: `${COUNT_FORMATTER.format(summary.attributedTokens)} of ${COUNT_FORMATTER.format(summary.totalTokens)} tokens attributed`;

	return (
		<section className={stateClasses} aria-label="Model usage summary">
			{refreshing ? (
				<p
					className="model-summary-strip__status"
					role="status"
					aria-live="polite"
					aria-atomic="true"
				>
					Refreshing model summary.
				</p>
			) : null}

			<dl className="model-summary-strip__metrics" aria-busy={refreshing}>
				<div className="model-summary-strip__metric">
					<dt className="model-summary-strip__label">Attributed coverage</dt>
					<dd className="model-summary-strip__reading">
						<span className="model-summary-strip__value">
							{formatCoverage(summary.attributedCoveragePercent)}
						</span>
						<span className="model-summary-strip__detail">{coverageDetail}</span>
					</dd>
				</div>

				<div className="model-summary-strip__metric">
					<dt className="model-summary-strip__label">Distinct models</dt>
					<dd className="model-summary-strip__reading">
						<span className="model-summary-strip__value">
							{COUNT_FORMATTER.format(summary.distinctModels)}
						</span>
						<span className="model-summary-strip__detail">
							Provider-qualified identities
						</span>
					</dd>
				</div>

				<div className="model-summary-strip__metric">
					<dt className="model-summary-strip__label">Multi-model sessions</dt>
					<dd className="model-summary-strip__reading">
						<span className="model-summary-strip__value">
							{COUNT_FORMATTER.format(summary.multiModelSessions)}
						</span>
						<span className="model-summary-strip__detail">Sessions in scope</span>
					</dd>
				</div>
			</dl>
		</section>
	);
}

export default ModelSummaryStrip;
