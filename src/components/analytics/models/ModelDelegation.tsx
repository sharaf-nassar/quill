import { memo } from "react";
import type {
	ModelDelegation as ModelDelegationData,
	ModelDelegationTop,
} from "../../../types";
import { formatTokenCount } from "../../../utils/tokens";
import {
	COUNT_FORMATTER,
	type ModelShadeMap,
	ModelSection,
	ModelSwatch,
	PERCENT_FORMATTER,
	modelShade,
} from "./modelFormat";

export interface ModelDelegationProps {
	delegation: ModelDelegationData;
	shadeMap: ModelShadeMap;
}

function TopModelDetail({
	top,
	shadeMap,
}: {
	top: ModelDelegationTop | null;
	shadeMap: ModelShadeMap;
}) {
	if (top === null) {
		return (
			<p className="delegation__detail delegation__detail--empty">
				no attributed model
			</p>
		);
	}
	return (
		<p className="delegation__detail">
			mostly <ModelSwatch color={modelShade(shadeMap, top.identity)} />
			<code title={top.identity.modelId} translate="no">
				<bdi dir="ltr">{top.identity.modelId}</bdi>
			</code>
			{" · "}
			{PERCENT_FORMATTER.format(top.sharePercent)}%
		</p>
	);
}

/**
 * Parent-vs-subagent token split. The subagent side carries the fixed orchid
 * orchestration hue, never a provider or model color.
 */
function ModelDelegation({ delegation, shadeMap }: ModelDelegationProps) {
	const total = delegation.parentTokens + delegation.subagentTokens;
	const parentWidth =
		total === 0 ? 0 : (delegation.parentTokens / total) * 100;
	const subagentWidth =
		total === 0 ? 0 : (delegation.subagentTokens / total) * 100;

	return (
		<ModelSection label="Delegation" meta="who runs what">
			{total === 0 ? (
				<p className="model-section__empty">
					No attributed chain tokens in this range.
				</p>
			) : (
				<>
					<div
						className="delegation__meter"
						role="img"
						aria-label={`Token split: parent chains ${COUNT_FORMATTER.format(delegation.parentTokens)} tokens, subagent chains ${COUNT_FORMATTER.format(delegation.subagentTokens)} tokens.`}
					>
						<span
							className="delegation__meter-parent"
							style={{ width: `${parentWidth.toFixed(1)}%` }}
						/>
						<span
							className="delegation__meter-subagent"
							style={{ width: `${subagentWidth.toFixed(1)}%` }}
						/>
					</div>
					<div className="delegation__grid">
						<div className="delegation__cell">
							<h3 className="delegation__who">Parent chains</h3>
							<p
								className="delegation__big"
								title={`${COUNT_FORMATTER.format(delegation.parentTokens)} tokens`}
							>
								{formatTokenCount(delegation.parentTokens)}
							</p>
							<TopModelDetail top={delegation.parentTop} shadeMap={shadeMap} />
						</div>
						<div className="delegation__cell">
							<h3 className="delegation__who delegation__who--subagent">
								Subagent chains
							</h3>
							<p
								className="delegation__big"
								title={`${COUNT_FORMATTER.format(delegation.subagentTokens)} tokens`}
							>
								{formatTokenCount(delegation.subagentTokens)}
							</p>
							<TopModelDetail
								top={delegation.subagentTop}
								shadeMap={shadeMap}
							/>
						</div>
					</div>
				</>
			)}
		</ModelSection>
	);
}

export default memo(ModelDelegation);
