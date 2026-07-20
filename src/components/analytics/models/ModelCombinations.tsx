import { memo } from "react";
import type { ModelCombinations as ModelCombinationsData } from "../../../types";
import { modelIdentityKey } from "../../../types";
import {
	COUNT_FORMATTER,
	type ModelShadeMap,
	ModelSection,
	ModelSwatch,
	modelShade,
} from "./modelFormat";

export interface ModelCombinationsProps {
	combinations: ModelCombinationsData;
	shadeMap: ModelShadeMap;
}

interface ComboBarProps {
	label: string;
	value: number;
	denominator: number;
	multi?: boolean;
}

function ComboBar({ label, value, denominator, multi = false }: ComboBarProps) {
	const width = denominator === 0 ? 0 : (value / denominator) * 100;
	return (
		<div className={multi ? "combo-bar combo-bar--multi" : "combo-bar"}>
			<span className="combo-bar__label">{label}</span>
			<span className="combo-bar__track" aria-hidden="true">
				<i style={{ width: `${width.toFixed(1)}%` }} />
			</span>
			<span className="combo-bar__value">
				{COUNT_FORMATTER.format(value)}
			</span>
		</div>
	);
}

/**
 * "Do I mix models within a task?" — session counts by distinct-model count
 * (orchid marks multi-model, the orchestration hue) plus the most-shared
 * model pairs.
 */
function ModelCombinations({ combinations, shadeMap }: ModelCombinationsProps) {
	const denominator =
		combinations.single + combinations.dual + combinations.threePlus;

	return (
		<ModelSection label="Model combinations" meta="per session">
			{denominator === 0 ? (
				<p className="model-section__empty">
					No sessions with model evidence in this range.
				</p>
			) : (
				<div className="combo-wrap">
					<div>
						<h3 className="model-section__sublabel">Models per session</h3>
						<div className="combo-bars">
							<ComboBar
								label="One model"
								value={combinations.single}
								denominator={denominator}
							/>
							<ComboBar
								label="Two models"
								value={combinations.dual}
								denominator={denominator}
								multi
							/>
							<ComboBar
								label="Three or more"
								value={combinations.threePlus}
								denominator={denominator}
								multi
							/>
						</div>
					</div>
					<div>
						<h3 className="model-section__sublabel">Most-shared pairs</h3>
						{combinations.topPairs.length === 0 ? (
							<p className="model-section__empty model-section__empty--inline">
								No sessions shared models.
							</p>
						) : (
							<ul className="combo-pairs">
								{combinations.topPairs.map((pair) => {
									const aKey = modelIdentityKey(pair.a);
									const bKey = modelIdentityKey(pair.b);
									return (
										<li key={`${aKey} ${bKey}`} className="combo-pair">
											<ModelSwatch color={modelShade(shadeMap, pair.a)} />
											<code title={pair.a.modelId} translate="no">
												<bdi dir="ltr">{pair.a.modelId}</bdi>
											</code>
											<span className="combo-pair__plus" aria-hidden="true">
												+
											</span>
											<ModelSwatch color={modelShade(shadeMap, pair.b)} />
											<code title={pair.b.modelId} translate="no">
												<bdi dir="ltr">{pair.b.modelId}</bdi>
											</code>
											<span className="combo-pair__count">
												{COUNT_FORMATTER.format(pair.sharedSessions)}{" "}
												{pair.sharedSessions === 1 ? "session" : "sessions"}
											</span>
										</li>
									);
								})}
							</ul>
						)}
					</div>
				</div>
			)}
		</ModelSection>
	);
}

export default memo(ModelCombinations);
