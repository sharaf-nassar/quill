import type { ChartSeriesVisibility } from "../../types";

interface TogglePillsProps {
	visibility: ChartSeriesVisibility;
	onChange: (updated: ChartSeriesVisibility) => void;
	hasTokenData: boolean;
}

const SERIES = [
	{ key: "utilization" as const, label: "Utilization", color: "#34d399" },
	{ key: "tokens" as const, label: "Tokens", color: "#60a5fa" },
];

function TogglePills({
	visibility,
	onChange,
	hasTokenData,
}: TogglePillsProps) {
	return (
		<div className="toggle-pills">
			{SERIES.map((series) => {
				if (series.key === "tokens" && !hasTokenData) return null;

				const active = visibility[series.key];

				return (
					<button
						key={series.key}
						className={`toggle-pill${active ? " active" : ""}`}
						onClick={() =>
							onChange({ ...visibility, [series.key]: !active })
						}
						aria-pressed={active}
						aria-label={`${active ? "Hide" : "Show"} ${series.label}`}
					>
						<span
							className="toggle-pill-dot"
							style={{
								background: active ? series.color : "transparent",
								borderColor: series.color,
							}}
						/>
						<span
							className="toggle-pill-label"
							style={{ opacity: active ? 1 : 0.4 }}
						>
							{series.label}
						</span>
					</button>
				);
			})}
		</div>
	);
}

export default TogglePills;
