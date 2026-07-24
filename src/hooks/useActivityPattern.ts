import { useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { TokenDataPoint, ActivityPatternData } from "../types";
import { useCachedInvoke } from "./useCachedInvoke";

const RANGE_MAP: Record<number, string> = { 7: "7d", 30: "30d" };

export function useActivityPattern(days: number): { data: ActivityPatternData | null; loading: boolean } {
	const request = useCallback(async (): Promise<ActivityPatternData> => {
		const history = await invoke<TokenDataPoint[]>("get_token_history", { range: RANGE_MAP[days] ?? "7d", hostname: null, sessionId: null, cwd: null });
		const hourlyTokens = new Array(24).fill(0);
		for (const point of history) hourlyTokens[new Date(point.timestamp).getHours()] += point.total_tokens;
		let maxSum = 0, peakStart = 0, peakEnd = 0;
		for (let start = 0; start < 24; start++) {
			let sum = 0;
			for (let len = 1; len <= 6; len++) {
				sum += hourlyTokens[(start + len - 1) % 24];
				if (len >= 2 && sum > maxSum) { maxSum = sum; peakStart = start; peakEnd = (start + len - 1) % 24; }
			}
		}
		return { hourlyTokens, peakStart, peakEnd };
	}, [days]);
	const { state } = useCachedInvoke({ identity: `activity-pattern:${days}`, request, normalizeError: String });
	return { data: state.data, loading: state.initialLoading };
}
