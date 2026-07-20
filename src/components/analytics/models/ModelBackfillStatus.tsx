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
	if (status.status === "pending") return "Incomplete · scan pending";

	if (status.status === "running") {
		if (status.failedRoots > 0) {
			return "Incomplete · provider root discovery errors";
		}
		if (status.completedRoots < status.totalRoots) {
			return "Incomplete · root discovery in progress";
		}
		if (status.remainingSources > 0) {
			return "Incomplete · source processing in progress";
		}
		return "Incomplete · finalizing history";
	}

	if (status.inventoryComplete) {
		return status.failedSources > 0
			? "Incomplete · inventory enumerated, source failures"
			: "Incomplete · inventory enumerated";
	}

	if (status.failedRoots > 0) {
		return "Incomplete · provider roots unavailable";
	}

	return status.status === "failed"
		? "Incomplete · history processing failed"
		: "Incomplete · history inventory unfinished";
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

/**
 * Retained-history annunciator. Renders as a one-line strip only while the
 * backfill demands operator attention (pending / running / partial / failed,
 * or a retained error); a clean complete state keeps only the visually hidden
 * live announcement so screen readers still hear the state change, while the
 * controls row shows the faint "N sources · complete" caption instead.
 */
// @lat: [[frontend#Frontend#Components#Models Composition]]
function ModelBackfillStatus({
	status,
	isRetrying = false,
	retryError = null,
	onRetry,
}: ModelBackfillStatusProps) {
	const titleId = useId();

	if (status === null) return null;

	const announcement = (
		<p
			className="model-annunciator__announcement models-visually-hidden"
			role="status"
			aria-live="polite"
			aria-atomic="true"
		>
			{liveAnnouncement(status, isRetrying)}
		</p>
	);

	const needsAttention =
		status.status !== "complete" ||
		status.lastError !== null ||
		retryError !== null ||
		isRetrying;
	if (!needsAttention) return announcement;

	const retryAvailable =
		status.status === "partial" || status.status === "failed";
	const totalFailed = status.failedRoots + status.failedSources;

	return (
		<section
			className={`model-annunciator model-annunciator--${status.status}`}
			aria-labelledby={titleId}
		>
			<div className="model-annunciator__line">
				<span id={titleId} className="model-annunciator__label">
					Retained history
				</span>
				<span className="model-annunciator__state">
					{STATE_LABELS[status.status]}
				</span>
				<span
					className="model-annunciator__counts"
					title={coverageLabel(status)}
				>
					roots {formatCount(status.completedRoots)}/
					{formatCount(status.totalRoots)}
					{" · "}
					sources {formatCount(status.processedSources)}/
					{formatCount(status.totalSources)}
					{" · "}
					{formatCount(totalFailed)} failed
				</span>
				{retryAvailable ? (
					<button
						type="button"
						className="model-annunciator__retry"
						onClick={onRetry}
						disabled={isRetrying}
					>
						{isRetrying ? "Retrying…" : "Retry"}
					</button>
				) : null}
			</div>

			{status.lastError ? (
				<p className="model-annunciator__diagnostic">
					Latest history error: {status.lastError}
				</p>
			) : null}

			{retryError ? (
				<p
					className="model-annunciator__diagnostic model-annunciator__diagnostic--alert"
					role="alert"
				>
					{retryError.message}
				</p>
			) : null}

			{announcement}
		</section>
	);
}

export default ModelBackfillStatus;
