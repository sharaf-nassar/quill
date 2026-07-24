import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { RetentionMaintenanceResult, RetentionPolicy } from "../types";

/**
 * Read/write access to the retention policy for the settings control (feature
 * 014).
 *
 * Deliberately **not** part of `RuntimeSettings`. `PerformanceTab.update()`
 * saves that struct wholesale (`{ ...settings, ...patch }`), which is the right
 * shape for a set of independent background-task tunings and the wrong shape
 * for a destructive boundary: a retention window is consented to one value at a
 * time, and a wholesale save would let an unrelated toggle re-assert a window
 * the user never looked at. So the policy travels on its own commands, with its
 * own hook, and is never merged into a struct save.
 *
 * The counterpart {@link useRetentionCutoff} is the read-only view mounted by
 * analytics and search surfaces; this hook is the only one that can write.
 */

/**
 * The windows the backend accepts. Anything else is rejected at the command
 * boundary — 30 is a floor, not a suggestion, because `range_to_duration` caps
 * every range-based reader at 30 days and a shorter window would start starving
 * readers that have no way to say so.
 */
export const RETENTION_WINDOW_PRESETS = [30, 90, 180, 365] as const;

export type RetentionWindowPreset = (typeof RETENTION_WINDOW_PRESETS)[number];

/** A fresh database carries none of the three settings rows. */
const NO_POLICY: RetentionPolicy = {
	window_days: null,
	watermark: null,
	last_run: null,
};

export interface UseRetentionPolicyResult {
	policy: RetentionPolicy;
	loading: boolean;
	saving: boolean;
	/** Populated when the last read or write failed; the policy stays as it was. */
	error: string | null;
	/** `null` means never prune. Resolves to the policy the backend stored. */
	setWindowDays: (windowDays: RetentionWindowPreset | null) => Promise<RetentionPolicy>;
	refresh: () => Promise<void>;
}

export function useRetentionPolicy(): UseRetentionPolicyResult {
	const [policy, setPolicy] = useState<RetentionPolicy>(NO_POLICY);
	const [loading, setLoading] = useState(true);
	const [saving, setSaving] = useState(false);
	const [error, setError] = useState<string | null>(null);

	const refresh = useCallback(async () => {
		setLoading(true);
		try {
			setPolicy(await invoke<RetentionPolicy>("get_retention_policy"));
			setError(null);
		} catch (e) {
			setError(String(e));
		} finally {
			setLoading(false);
		}
	}, []);

	useEffect(() => {
		void refresh();
	}, [refresh]);

	// A completed run advances the watermark and writes the audit record, so the
	// control re-reads rather than rendering the policy it held before the run.
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

	const setWindowDays = useCallback(
		async (windowDays: RetentionWindowPreset | null) => {
			setSaving(true);
			try {
				// The backend returns the policy it actually stored, so a rejected
				// preset leaves this hook holding the old window rather than an
				// optimistic one the database never accepted.
				const resolved = await invoke<RetentionPolicy>("set_retention_policy", {
					windowDays,
				});
				setPolicy(resolved);
				setError(null);
				return resolved;
			} catch (e) {
				const message = String(e);
				setError(message);
				throw new Error(message);
			} finally {
				setSaving(false);
			}
		},
		[],
	);

	return { policy, loading, saving, error, setWindowDays, refresh };
}
