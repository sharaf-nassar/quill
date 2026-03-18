import { useState, useEffect, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { LearnedRule, LearningStatsData } from "../types";

export function useLearningStats(): {
	stats: LearningStatsData | null;
	loading: boolean;
} {
	const [stats, setStats] = useState<LearningStatsData | null>(null);
	const [loading, setLoading] = useState(true);

	const fetchData = useCallback(async () => {
		setLoading(true);
		try {
			const rules = await invoke<LearnedRule[]>("get_learned_rules");

			let emerging = 0;
			let confirmed = 0;
			const confidenceBuckets = [0, 0, 0, 0, 0];
			const weekAgo = Date.now() - 7 * 24 * 60 * 60 * 1000;
			let newThisWeek = 0;

			for (const rule of rules) {
				if (rule.state === "emerging") emerging++;
				else if (rule.state === "confirmed") confirmed++;

				const conf = rule.confidence;
				const bucketIdx = Math.min(Math.floor(conf * 5), 4);
				confidenceBuckets[bucketIdx]++;

				if (new Date(rule.created_at).getTime() >= weekAgo) {
					newThisWeek++;
				}
			}

			setStats({
				total: rules.length,
				emerging,
				confirmed,
				confidenceBuckets,
				newThisWeek,
			});
		} catch (e) {
			console.error("Learning stats fetch error:", e);
			setStats(null);
		} finally {
			setLoading(false);
		}
	}, []);

	useEffect(() => {
		fetchData();
	}, [fetchData]);

	useEffect(() => {
		const interval = setInterval(fetchData, 60_000);
		return () => clearInterval(interval);
	}, [fetchData]);

	return { stats, loading };
}
