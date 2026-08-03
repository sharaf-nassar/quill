import { useEffect } from "react";
import { listen } from "@tauri-apps/api/event";
import type { ModelAnalyticsUpdatedEvent } from "../types";
import {
	cachedInvokeStore,
	cleanupInvokeListeners,
	registerInvokeEventListeners,
} from "./cachedInvokeStore";

const INGEST_EVENTS = [
	"tokens-updated",
	"sessions-index-updated",
	"transcript-analytics-updated",
	"context-savings-updated",
	"hooks-observed-updated",
] as const;

/**
 * Keeps process-lifetime query entries honest even while their view is
 * unmounted. Only mounted subscribers join the shared refresh batch; inactive
 * entries stay stale until a later cache-first remount.
 */
export function useCachedInvokeEvents(): void {
	useEffect(() => {
		let disposed = false;
		const refreshMounted = () => document.visibilityState !== "hidden";
		const listeners = registerInvokeEventListeners(
			[...INGEST_EVENTS, "model-analytics-updated"],
			listen,
			(eventName, event) => {
				if (disposed) return;
				if (
					eventName === "model-analytics-updated" &&
					(event.payload as ModelAnalyticsUpdatedEvent).dataChanged === false
				) {
					return;
				}
				cachedInvokeStore.invalidateEvent(
					eventName,
					refreshMounted(),
				);
			},
		);

		const cleanupListeners = cleanupInvokeListeners(listeners, (error) => {
			console.error("Cached invoke event listener failed:", error);
		});
		const handleVisibilityChange = () => {
			if (document.visibilityState !== "hidden") {
				cachedInvokeStore.refreshStaleSubscribers();
			}
		};
		document.addEventListener("visibilitychange", handleVisibilityChange);

		return () => {
			disposed = true;
			document.removeEventListener("visibilitychange", handleVisibilityChange);
			cleanupListeners();
		};
	}, []);
}
