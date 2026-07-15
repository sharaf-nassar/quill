import { useId } from "react";
import type {
	ModelAnalyticsError,
	ModelBackfillStatus as ModelBackfillStatusData,
} from "../../../types";

const COUNT_FORMATTER = new Intl.NumberFormat("en-US");

const STATE_LABELS: Record<ModelBackfillStatusData["status"], string> = {
	pending: "Pending",
	running: "Running",
	complete: "Complete",
	partial: "Partial",
	failed: "Failed",
};

export interface ModelBackfillStatusProps {
	status: ModelBackfillStatusData | null;
	isRetrying?: boolean;
	retryError?: ModelAnalyticsError | null;
	onRetry: () => void;
}

function formatCount(value: number): string {
	return COUNT_FORMATTER.format(value);
}

function coverageLabel(status: ModelBackfillStatusData): string {
	if (status.status === "complete") return "Inventory complete";
	if (status.status === "pending") return "Incomplete \u00b7 scan pending";

	if (status.status === "running") {
		if (status.failedRoots > 0) {
			return "Incomplete \u00b7 provider root discovery errors";
		}
		if (status.completedRoots < status.totalRoots) {
			return "Incomplete \u00b7 root discovery in progress";
		}
		if (status.remainingSources > 0) {
			return "Incomplete \u00b7 source processing in progress";
		}
		return "Incomplete \u00b7 finalizing history";
	}

	if (status.inventoryComplete) {
		return status.failedSources > 0
			? "Incomplete \u00b7 inventory enumerated, source failures"
			: "Incomplete \u00b7 inventory enumerated";
	}

	if (status.failedRoots > 0) {
		return "Incomplete \u00b7 provider roots unavailable";
	}

	return status.status === "failed"
		? "Incomplete \u00b7 history processing failed"
		: "Incomplete \u00b7 history inventory unfinished";
}

function liveAnnouncement(
	status: ModelBackfillStatusData,
	isRetrying: boolean,
): string {
	const retryState = isRetrying
		? " Retry in progress."
		: status.status === "partial" || status.status === "failed"
			? " Retry available."
			: "";
	const diagnostic = status.lastError
		? ` Latest history error: ${status.lastError}`
		: "";

	return `Retained history ${STATE_LABELS[status.status].toLowerCase()}. ${coverageLabel(status)}. Roots: ${formatCount(status.completedRoots)} of ${formatCount(status.totalRoots)} complete, ${formatCount(status.failedRoots)} failed. Sources: ${formatCount(status.processedSources)} of ${formatCount(status.totalSources)} processed, ${formatCount(status.failedSources)} failed, ${formatCount(status.remainingSources)} remaining.${retryState}${diagnostic}`;
}

// @lat: [[frontend#Frontend#Components#Models Composition]]
function ModelBackfillStatus({
	status,
	isRetrying = false,
	retryError = null,
	onRetry,
}: ModelBackfillStatusProps) {
	const titleId = useId();

	if (status === null) return null;

	const incomplete = status.status !== "complete";
	const retryAvailable =
		status.status === "partial" || status.status === "failed";
	const stateClasses = [
		"model-backfill-status",
		`model-backfill-status--${status.status}`,
		incomplete ? "model-backfill-status--incomplete" : null,
	]
		.filter(Boolean)
		.join(" ");

	return (
		<section
			className={stateClasses}
			aria-labelledby={titleId}
		>
			<div className="model-backfill-status__header">
				<h2 id={titleId} className="model-backfill-status__title">
					Retained history
				</h2>
				<span className="model-backfill-status__state">
					{STATE_LABELS[status.status]}
				</span>
				<span className="model-backfill-status__coverage">
					{coverageLabel(status)}
				</span>
			</div>

			<dl
				className="model-backfill-status__metrics"
				aria-busy={status.status === "running" || isRetrying}
			>
				<div className="model-backfill-status__metric">
					<dt className="model-backfill-status__metric-label">Roots</dt>
					<dd className="model-backfill-status__metric-values">
						<span>
							<strong>{formatCount(status.completedRoots)}</strong>
							{" / "}
							{formatCount(status.totalRoots)} complete
						</span>
						<span>
							<strong>{formatCount(status.failedRoots)}</strong> failed
						</span>
					</dd>
				</div>

				<div className="model-backfill-status__metric">
					<dt className="model-backfill-status__metric-label">Sources</dt>
					<dd className="model-backfill-status__metric-values">
						<span>
							<strong>{formatCount(status.processedSources)}</strong>{" "}
							/ {formatCount(status.totalSources)} processed
						</span>
						<span>
							<strong>{formatCount(status.failedSources)}</strong> failed
						</span>
						<span>
							<strong>{formatCount(status.remainingSources)}</strong>{" "}
							remaining
						</span>
					</dd>
				</div>
			</dl>

			{status.lastError ? (
				<p className="model-backfill-status__diagnostic">
					Latest history error: {status.lastError}
				</p>
			) : null}

			{retryError ? (
				<p className="model-backfill-status__retry-error" role="alert">
					{retryError.message}
				</p>
			) : null}

			{retryAvailable ? (
				<button
					type="button"
					className="model-backfill-status__retry"
					onClick={onRetry}
					disabled={isRetrying}
				>
					{isRetrying ? "Retrying history\u2026" : "Retry history"}
				</button>
			) : null}

			<p
				className="model-backfill-status__announcement models-visually-hidden"
				role="status"
				aria-live="polite"
				aria-atomic="true"
			>
				{liveAnnouncement(status, isRetrying)}
			</p>
		</section>
	);
}

export default ModelBackfillStatus;
