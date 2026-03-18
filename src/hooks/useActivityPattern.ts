import { useState, useEffect, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { TokenDataPoint, ActivityPatternData } from "../types";

const RANGE_MAP: Record<number, string> = {
	7: "7d",
	30: "30d",
};

export function useActivityPattern(days: number): {
	data: ActivityPatternData | null;
	loading: boolean;
} {
	const [data, setData] = useState<ActivityPatternData | null>(null);
	const [loading, setLoading] = useState(true);

	const fetchData = useCallback(async () => {
		setLoading(true);
		try {
			const range = RANGE_MAP[days] ?? "7d";
			const history = await invoke<TokenDataPoint[]>("get_token_history", {
				range,
				hostname: null,
				sessionId: null,
				cwd: null,
			});

			const hourlyTokens = new Array(24).fill(0);
			for (const point of history) {
				const hour = new Date(point.timestamp).getHours();
				hourlyTokens[hour] += point.total_tokens;
			}

			let maxSum = 0;
			let peakStart = 0;
			let peakEnd = 0;

			for (let start = 0; start < 24; start++) {
				let sum = 0;
				for (let len = 1; len <= 6; len++) {
					const idx = (start + len - 1) % 24;
					sum += hourlyTokens[idx];
					if (len >= 2 && sum > maxSum) {
						maxSum = sum;
						peakStart = start;
						peakEnd = (start + len - 1) % 24;
					}
				}
			}

			setData({ hourlyTokens, peakStart, peakEnd });
		} catch (e) {
			console.error("Activity pattern fetch error:", e);
			setData(null);
		} finally {
			setLoading(false);
		}
	}, [days]);

	useEffect(() => {
		fetchData();
	}, [fetchData]);

	return { data, loading };
}
