import { memo, useMemo } from "react";
import type {
	ModelIdentity,
	ModelIdentityKey,
	ModelProjectMatrixRow,
	ModelUsageOverviewRow,
} from "../../../types";
import { modelIdentityKey } from "../../../types";
import {
	COUNT_FORMATTER,
	type ModelShadeMap,
	ModelSection,
	hexAlpha,
	modelShade,
	providerGroupRank,
	providerLabel,
	shortModelId,
} from "./modelFormat";

/** Cell tint alpha ramp: floor + span scaled by value / max. */
const CELL_ALPHA_FLOOR = 0.14;
const CELL_ALPHA_SPAN = 0.38;

interface MatrixColumn {
	key: ModelIdentityKey;
	identity: ModelIdentity;
	color: string;
}

export interface ModelProjectMatrixProps {
	matrix: readonly ModelProjectMatrixRow[];
	models: readonly ModelUsageOverviewRow[];
	shadeMap: ModelShadeMap;
}

/** Projects × models: session counts per pairing, tinted in model shades. */
function ModelProjectMatrix({
	matrix,
	models,
	shadeMap,
}: ModelProjectMatrixProps) {
	const columns = useMemo<MatrixColumn[]>(() => {
		const present = new Set<string>();
		for (const row of matrix) {
			for (const cell of row.cells) {
				if (cell.sessions > 0) present.add(modelIdentityKey(cell.identity));
			}
		}

		const ordered: { identity: ModelIdentity; deliveredIndex: number }[] = [];
		const seen = new Set<string>();
		models.forEach((row, deliveredIndex) => {
			const key = modelIdentityKey(row.identity);
			if (present.has(key) && !seen.has(key)) {
				seen.add(key);
				ordered.push({ identity: row.identity, deliveredIndex });
			}
		});
		// Matrix identities missing from models[] (shouldn't happen) trail.
		for (const row of matrix) {
			for (const cell of row.cells) {
				const key = modelIdentityKey(cell.identity);
				if (present.has(key) && !seen.has(key)) {
					seen.add(key);
					ordered.push({
						identity: cell.identity,
						deliveredIndex: Number.MAX_SAFE_INTEGER,
					});
				}
			}
		}

		return ordered
			.sort((left, right) => {
				const groupOrder =
					providerGroupRank(left.identity.provider) -
					providerGroupRank(right.identity.provider);
				return groupOrder !== 0
					? groupOrder
					: left.deliveredIndex - right.deliveredIndex;
			})
			.map(({ identity }) => ({
				key: modelIdentityKey(identity),
				identity,
				color: modelShade(shadeMap, identity),
			}));
	}, [matrix, models, shadeMap]);

	// Precompute the tint denominator and per-row session lookups once per
	// matrix instead of rebuilding a Map for every rendered row.
	const { maxCell, rowViews } = useMemo(() => {
		const views = matrix.map((row) => {
			const cellByKey = new Map<ModelIdentityKey, number>();
			for (const cell of row.cells) {
				cellByKey.set(modelIdentityKey(cell.identity), cell.sessions);
			}
			return { row, cellByKey };
		});
		const maxCellValue = matrix.reduce(
			(max, row) =>
				row.cells.reduce(
					(rowMax, cell) => Math.max(rowMax, cell.sessions),
					max,
				),
			0,
		);
		return { maxCell: maxCellValue, rowViews: views };
	}, [matrix]);

	return (
		<ModelSection
			label="Projects × models"
			meta="sessions per pairing"
		>
			{matrix.length === 0 || columns.length === 0 ? (
				<p className="model-section__empty">
					No project activity in this range.
				</p>
			) : (
				<>
					<div
						className="model-matrix__scroller"
						role="region"
						aria-label="Projects by models matrix; scroll horizontally for all models"
						tabIndex={0}
					>
						<table className="model-matrix">
							<caption className="models-visually-hidden">
								Distinct sessions per project and model pairing.
							</caption>
							<thead>
								<tr>
									<th scope="col" className="model-matrix__project-head">
										Project
									</th>
									{columns.map((column) => (
										<th
											key={column.key}
											scope="col"
											className="model-matrix__model-head"
											title={column.identity.modelId}
										>
											<em>{providerLabel(column.identity.provider)}</em>
											<span className="model-matrix__model-head-id">
												<span
													className="model-swatch model-swatch--small"
													style={{ background: column.color }}
													aria-hidden="true"
												/>
												<code translate="no">
													<bdi dir="ltr">
														{shortModelId(column.identity.modelId)}
													</bdi>
												</code>
											</span>
										</th>
									))}
									<th scope="col" className="model-matrix__total-head">
										Sess
									</th>
								</tr>
							</thead>
							<tbody>
								{rowViews.map(({ row, cellByKey }) => {
									return (
										<tr key={row.project}>
											<th
												scope="row"
												className="model-matrix__project"
												title={row.project}
											>
												{row.project}
											</th>
											{columns.map((column) => {
												const sessions = cellByKey.get(column.key) ?? 0;
												if (sessions === 0 || maxCell === 0) {
													return (
														<td key={column.key}>
															<span
																className="model-matrix__cell model-matrix__cell--zero"
																aria-label="0 sessions"
															>
																·
															</span>
														</td>
													);
												}
												const alpha =
													CELL_ALPHA_FLOOR +
													(sessions / maxCell) * CELL_ALPHA_SPAN;
												return (
													<td key={column.key}>
														<span
															className="model-matrix__cell"
															style={{
																background: hexAlpha(column.color, alpha),
															}}
														>
															{COUNT_FORMATTER.format(sessions)}
														</span>
													</td>
												);
											})}
											<td className="model-matrix__total">
												{COUNT_FORMATTER.format(row.totalSessions)}
											</td>
										</tr>
									);
								})}
							</tbody>
						</table>
					</div>
					<p className="model-section__foot">
						Cell = distinct sessions in that project that used that model.
						Denser cell = more sessions.
					</p>
				</>
			)}
		</ModelSection>
	);
}

export default memo(ModelProjectMatrix);
