import { memo } from "react";
import type { ModelRunningNowEntry } from "../../../types";
import {
	type ModelShadeMap,
	ModelSection,
	ModelSwatch,
	ProviderBadge,
	formatDateTime,
	formatSince,
	modelShade,
	relativeTime,
} from "./modelFormat";

export interface ModelRunningNowProps {
	entries: readonly ModelRunningNowEntry[];
	shadeMap: ModelShadeMap;
}

/** "What am I on right now?" — one cell per provider with a live model. */
function ModelRunningNow({ entries, shadeMap }: ModelRunningNowProps) {
	return (
		<ModelSection label="Running now" meta="per provider">
			{entries.length === 0 ? (
				<p className="model-section__empty">
					No recent model activity in this range.
				</p>
			) : (
				<ul className="running-now">
					{entries.map((entry) => (
						<li
							key={`${entry.provider} ${entry.modelId}`}
							className="running-now__cell"
						>
							<div className="running-now__line1">
								<ProviderBadge provider={entry.provider} />
								<ModelSwatch
									color={modelShade(shadeMap, {
										provider: entry.provider,
										modelId: entry.modelId,
									})}
								/>
								<code className="running-now__model" title={entry.modelId}>
									<bdi dir="ltr" translate="no">
										{entry.modelId}
									</bdi>
								</code>
								<time
									className="running-now__ago"
									dateTime={entry.lastSeenAt}
									title={formatDateTime(entry.lastSeenAt)}
								>
									{relativeTime(entry.lastSeenAt)}
								</time>
							</div>
							<div className="running-now__line2">
								since {formatSince(entry.runningSinceAt)}
								{entry.previousModelId !== null ? (
									<>
										{" · "}
										<span className="running-now__prev" translate="no">
											replaced {entry.previousModelId}
										</span>
									</>
								) : null}
							</div>
						</li>
					))}
				</ul>
			)}
		</ModelSection>
	);
}

export default memo(ModelRunningNow);
