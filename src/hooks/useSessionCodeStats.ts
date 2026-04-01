import { useState, useEffect, useRef, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { sessionRefKey, type SessionCodeStats, type SessionRef } from "../types";

type StatsMap = Record<string, SessionCodeStats>;

export function useSessionCodeStats(sessionRefs: SessionRef[]) {
	const [statsMap, setStatsMap] = useState<StatsMap>({});
	const cacheRef = useRef<StatsMap>({});

	const fetchMissing = useCallback(async (refs: SessionRef[]) => {
		const missing = refs.filter((ref) => !(sessionRefKey(ref) in cacheRef.current));
		if (missing.length === 0) return;

		try {
			const result = await invoke<StatsMap>("get_batch_session_code_stats", {
				sessionRefs: missing,
			});
			for (const ref of missing) {
				const key = sessionRefKey(ref);
				cacheRef.current[key] = result[key] ?? {
					lines_added: 0,
					lines_removed: 0,
					net_change: 0,
				};
			}
			setStatsMap({ ...cacheRef.current });
		} catch (e) {
			console.error("Session code stats fetch error:", e);
		}
	}, []);

	useEffect(() => {
		if (sessionRefs.length > 0) {
			fetchMissing(sessionRefs);
		}
	}, [sessionRefs, fetchMissing]);

	return statsMap;
}
