import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { RetentionMaintenanceResult, RetentionPolicy } from "../types";

/**
 * Read-only view of the retention boundary, for surfaces that have to render
 * honestly around it (feature 014).
 *
 * Deliberately separate from the settings control's policy hook: this one is
 * mounted by analytics and search surfaces that must never be able to change
 * the policy, only to describe it. It exposes the **watermark** as `cutoff`
 * because the watermark is the only durable fact about what was actually
 * removed — `windowDays` is the standing intention and is carried alongside
 * purely so copy can say "90-day retention" where that helps.
 *
 * The hook re-reads on `retention-maintenance-finished` so a banner that
 * appears mid-session states the new cutoff rather than the stale one. A failed
 * read leaves `cutoff` null, which degrades to "render nothing extra" — the
 * pre-014 behaviour — rather than to a banner asserting a boundary that may not
 * exist.
 */
export interface RetentionCutoffState {
	/** Watermark instant; null when retention has never pruned anything. */
	cutoff: string | null;
	/** Configured window in days; null means "never prune". */
	windowDays: number | null;
	loading: boolean;
	/** Populated when the policy read failed; `cutoff` stays null. */
	error: string | null;
}

const NO_CUTOFF: RetentionCutoffState = {
	cutoff: null,
	windowDays: null,
	loading: true,
	error: null,
};

export function useRetentionCutoff(): RetentionCutoffState {
	const [state, setState] = useState<RetentionCutoffState>(NO_CUTOFF);

	const refresh = useCallback(async () => {
		try {
			const policy = await invoke<RetentionPolicy>("get_retention_policy");
			setState({
				cutoff: policy.watermark,
				windowDays: policy.window_days,
				loading: false,
				error: null,
			});
		} catch (e) {
			// A missing or failing policy read must not blank out the surface that
			// mounted this hook, so the failure is recorded and the cutoff stays
			// null. It is logged rather than swallowed silently.
			const message = String(e);
			console.error("Retention policy read failed:", message);
			setState({
				cutoff: null,
				windowDays: null,
				loading: false,
				error: message,
			});
		}
	}, []);

	useEffect(() => {
		void refresh();
	}, [refresh]);

	useEffect(() => {
		const unlisten = listen<RetentionMaintenanceResult>(
			"retention-maintenance-finished",
			() => {
				void refresh();
			},
		);
		return () => {
			unlisten.then((fn) => fn());
		};
	}, [refresh]);

	return state;
}
