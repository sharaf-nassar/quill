import { useState, useEffect, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useSessionHealth } from "../../hooks/useSessionHealth";
import { useActivityPattern } from "../../hooks/useActivityPattern";
import { useLearningStats } from "../../hooks/useLearningStats";
import SessionHealthCard from "./SessionHealthCard";
import ActivityHeatmap from "./ActivityHeatmap";
import ProjectFocusCard from "./ProjectFocusCard";
import LearningProgressCard from "./LearningProgressCard";
import type { RangeType, ProjectTokensRaw } from "../../types";

const TREND_RANGES: RangeType[] = ["7d", "30d"];
const RANGE_LABELS: Record<string, string> = { "7d": "7D", "30d": "30D" };
const RANGE_DAYS: Record<string, number> = { "7d": 7, "30d": 30 };

interface TrendsTabProps {
	range: RangeType;
	onRangeChange: (r: RangeType) => void;
}

function useProjectTokens(days: number) {
	const [data, setData] = useState<ProjectTokensRaw[]>([]);
	const [loading, setLoading] = useState(true);

	const fetchData = useCallback(async () => {
		setLoading(true);
		try {
			const result = await invoke<ProjectTokensRaw[]>("get_project_tokens", { days });
			setData(result);
		} catch (e) {
			console.error("Project tokens fetch error:", e);
			setData([]);
		} finally {
			setLoading(false);
		}
	}, [days]);

	useEffect(() => { fetchData(); }, [fetchData]);

	return { data, loading };
}

function TrendsTab({ range, onRangeChange }: TrendsTabProps) {
	const days = RANGE_DAYS[range] ?? 7;

	const { stats: sessionHealth, loading: sessionLoading } = useSessionHealth(days);
	const { data: activityPattern, loading: activityLoading } = useActivityPattern(days);
	const { stats: learningStats, loading: learningLoading } = useLearningStats();
	const { data: projectData, loading: projectLoading } = useProjectTokens(days);

	return (
		<>
			<div className="analytics-controls">
				<div className="range-tabs">
					{TREND_RANGES.map((r) => (
						<button
							key={r}
							className={`range-tab${range === r ? " active" : ""}`}
							aria-pressed={range === r}
							onClick={() => onRangeChange(r)}
						>
							{RANGE_LABELS[r]}
						</button>
					))}
				</div>
			</div>

			{sessionLoading ? (
				<div className="trends-card-skeleton" />
			) : sessionHealth && sessionHealth.sessionCount > 0 ? (
				<SessionHealthCard stats={sessionHealth} />
			) : (
				<div className="trends-card trends-card-empty">No session data yet</div>
			)}

			{activityLoading ? (
				<div className="trends-card-skeleton" />
			) : activityPattern ? (
				<ActivityHeatmap data={activityPattern} />
			) : (
				<div className="trends-card trends-card-empty">No activity data yet</div>
			)}

			<div className="trends-bottom-row">
				{projectLoading ? (
					<div className="trends-card-skeleton" />
				) : projectData.length > 0 ? (
					<ProjectFocusCard data={projectData} />
				) : (
					<div className="trends-card trends-card-empty">No project data yet</div>
				)}

				{learningLoading ? (
					<div className="trends-card-skeleton" />
				) : learningStats && learningStats.total > 0 ? (
					<LearningProgressCard stats={learningStats} />
				) : (
					<div className="trends-card trends-card-empty">No learned rules yet</div>
				)}
			</div>
		</>
	);
}

export default TrendsTab;
