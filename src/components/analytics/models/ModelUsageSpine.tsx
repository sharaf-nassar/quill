import { memo, useMemo } from "react";
import type {
	ModelIdentity,
	ModelRange,
	ModelUsageOverviewRow,
} from "../../../types";
import { modelIdentityKey } from "../../../types";
import { formatTokenCount } from "../../../utils/tokens";
import {
	COUNT_FORMATTER,
	type ModelShadeMap,
	ModelSection,
	ModelSwatch,
	PERCENT_FORMATTER,
	ProviderBadge,
	hexAlpha,
	modelShade,
} from "./modelFormat";

/** Rows below this share of all sessions render dimmed as long-tail. */
const MINOR_SESSION_PERCENT = 5;

export interface ModelUsageSpineProps {
	rows: readonly ModelUsageOverviewRow[];
	range: ModelRange;
	shadeMap: ModelShadeMap;
	selectedModel: ModelIdentity | null;
	onSelectModel: (identity: ModelIdentity | null) => void;
}

function DayDots({ daysActive }: { daysActive: number }) {
	return (
		<span className="usage-days" aria-hidden="true">
			{Array.from({ length: 7 }, (_, index) => (
				<i
					key={index}
					className={
						index < daysActive ? "usage-days__dot" : "usage-days__dot usage-days__dot--off"
					}
				/>
			))}
		</span>
	);
}

/**
 * The page spine: models ranked by sessions used in (delivered order), with
 * reach (projects), primacy, turns, days active, and tokens demoted to the
 * trailing column. Row activation toggles the drill-down selection.
 */
function ModelUsageSpine({
	rows,
	range,
	shadeMap,
	selectedModel,
	onSelectModel,
}: ModelUsageSpineProps) {
	const selectedKey = selectedModel ? modelIdentityKey(selectedModel) : null;
	const maxSessions = useMemo(
		() => rows.reduce((max, row) => Math.max(max, row.sessions), 0),
		[rows],
	);
	const daysHeading = range === "7d" ? "Days" : "Days active";

	return (
		<ModelSection label="Usage" meta="ranked by sessions used in">
			{rows.length === 0 ? (
				<p className="model-section__empty">
					No model usage rows match this scope.
				</p>
			) : (
				<>
					<div className="usage-grid usage-grid--head" aria-hidden="true">
						<span className="usage-grid__h usage-grid__h--l">Provider</span>
						<span className="usage-grid__h usage-grid__h--l">Model</span>
						<span className="usage-grid__h usage-grid__h--l">Sessions</span>
						<span className="usage-grid__h usage-grid__h--mid">Projects</span>
						<span className="usage-grid__h usage-grid__h--mid">Primary in</span>
						<span className="usage-grid__h usage-grid__h--wide">Turns</span>
						<span className="usage-grid__h usage-grid__h--wide">
							{daysHeading}
						</span>
						<span className="usage-grid__h">Tokens</span>
					</div>
					<ol className="usage-rows">
						{rows.map((row) => {
							const key = modelIdentityKey(row.identity);
							const selected = key === selectedKey;
							const minor =
								row.sessionPercent !== null &&
								row.sessionPercent < MINOR_SESSION_PERCENT;
							const shade = modelShade(shadeMap, row.identity);
							const barWidth =
								maxSessions === 0 ? 0 : (row.sessions / maxSessions) * 100;
							const classes = [
								"usage-grid",
								"usage-row",
								minor ? "usage-row--minor" : null,
								selected ? "usage-row--selected" : null,
							]
								.filter(Boolean)
								.join(" ");

							return (
								<li key={key} className="usage-rows__item">
									<button
										type="button"
										className={classes}
										aria-pressed={selected}
										onClick={() =>
											onSelectModel(selected ? null : row.identity)
										}
									>
										<span className="usage-row__provider">
											<ProviderBadge provider={row.identity.provider} />
										</span>
										<span
											className="usage-row__id"
											title={row.identity.modelId}
										>
											<ModelSwatch color={shade} />
											<code>
												<bdi dir="ltr" translate="no">
													{row.identity.modelId}
												</bdi>
											</code>
										</span>
										<span className="usage-row__sessions">
											<span className="usage-row__bar" aria-hidden="true">
												<i
													style={{
														width: `${barWidth.toFixed(1)}%`,
														background: hexAlpha(shade, minor ? 0.45 : 0.8),
													}}
												/>
											</span>
											<span className="usage-row__n">
												<span className="models-visually-hidden">
													sessions{" "}
												</span>
												{COUNT_FORMATTER.format(row.sessions)}
											</span>
											{row.sessionPercent !== null ? (
												<span className="models-visually-hidden">
													{" "}
													({PERCENT_FORMATTER.format(row.sessionPercent)}% of
													all sessions)
												</span>
											) : null}
										</span>
										<span className="usage-row__c usage-row__c--mid">
											<span className="models-visually-hidden">projects </span>
											{COUNT_FORMATTER.format(row.projects)}
										</span>
										<span className="usage-row__c usage-row__c--mid">
											<span className="models-visually-hidden">
												primary in{" "}
											</span>
											{COUNT_FORMATTER.format(row.primaryIn)}
											<small> /{COUNT_FORMATTER.format(row.sessions)}</small>
										</span>
										<span className="usage-row__c usage-row__c--dim usage-row__c--wide">
											<span className="models-visually-hidden">turns </span>
											{COUNT_FORMATTER.format(row.turns)}
										</span>
										<span className="usage-row__c usage-row__c--wide">
											<span className="models-visually-hidden">
												days active {COUNT_FORMATTER.format(row.daysActive)}
											</span>
											{range === "7d" ? (
												<DayDots daysActive={row.daysActive} />
											) : (
												<span aria-hidden="true">
													{COUNT_FORMATTER.format(row.daysActive)}
												</span>
											)}
										</span>
										<span
											className="usage-row__c usage-row__c--dim"
											title={`${COUNT_FORMATTER.format(row.attributedTokens)} attributed tokens`}
										>
											<span className="models-visually-hidden">tokens </span>
											{formatTokenCount(row.attributedTokens)}
										</span>
									</button>
								</li>
							);
						})}
					</ol>
					<p className="model-section__foot">
						Primary in = sessions where this model did the most work.
						{range === "7d"
							? " Days = distinct days active of 7."
							: " Days active = distinct days with activity in range."}
					</p>
				</>
			)}
		</ModelSection>
	);
}

export default memo(ModelUsageSpine);
