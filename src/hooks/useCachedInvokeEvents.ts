import { useEffect } from "react";
import { listen } from "@tauri-apps/api/event";
import type { ModelAnalyticsUpdatedEvent } from "../types";
import { cachedInvokeStore } from "./cachedInvokeStore";

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
		const listeners = [...INGEST_EVENTS, "model-analytics-updated"].map(
			(eventName) =>
				listen(eventName, (event) => {
					if (disposed) return;
					if (
						eventName === "model-analytics-updated" &&
						(event.payload as ModelAnalyticsUpdatedEvent).dataChanged === false
					) {
						return;
					}
					cachedInvokeStore.invalidateEvent(eventName, refreshMounted());
				}).catch((error: unknown) => {
					if (!disposed) {
						console.error("Cached invoke event listener failed:", error);
					}
					return () => {};
				}),
		);
		const handleVisibilityChange = () => {
			if (document.visibilityState !== "hidden") {
				cachedInvokeStore.refreshStaleSubscribers();
			}
		};
		document.addEventListener("visibilitychange", handleVisibilityChange);

		return () => {
			disposed = true;
			document.removeEventListener("visibilitychange", handleVisibilityChange);
			void Promise.all(listeners).then((unlisteners) => {
				for (const unlisten of unlisteners) unlisten();
			});
		};
	}, []);
}
