export const CACHED_INVOKE_TTL_MS = 45_000;
export const CACHED_INVOKE_COALESCE_MS = 5_000;

export interface CachedInvokeDescriptor {
	command: string;
	args?: unknown;
}

export interface CachedInvokeSnapshot<T> {
	data: T | null;
	hasData: boolean;
	initialLoading: boolean;
	refreshing: boolean;
	error: unknown | null;
}

export interface CachedInvokeNotification<T> {
	kind: "snapshot" | "loading" | "accepted" | "error";
	snapshot: CachedInvokeSnapshot<T>;
}

interface CachedInvokeClock {
	now: () => number;
	setTimer: (callback: () => void, delayMs: number) => unknown;
	clearTimer: (timer: unknown) => void;
}

interface CachedInvokeEntry<T> {
	key: string;
	hasData: boolean;
	data: T | undefined;
	serialized: string | null;
	acceptedAt: number;
	error: unknown | null;
	inFlight: Promise<void> | null;
	generation: number;
	request: (() => Promise<T>) | null;
	invalidationEvents: Set<string>;
	subscribers: Set<(notification: CachedInvokeNotification<T>) => void>;
}

const SYSTEM_CLOCK: CachedInvokeClock = {
	now: Date.now,
	setTimer: (callback, delayMs) => setTimeout(callback, delayMs),
	clearTimer: (timer) => clearTimeout(timer as ReturnType<typeof setTimeout>),
};

function stableSerialize(value: unknown, seen = new Set<object>()): string {
	if (value === null) return "null";

	switch (typeof value) {
		case "string":
			return JSON.stringify(value);
		case "boolean":
			return value ? "true" : "false";
		case "number":
			if (Number.isNaN(value)) return '"$nan"';
			if (value === Number.POSITIVE_INFINITY) return '"$infinity"';
			if (value === Number.NEGATIVE_INFINITY) return '"$-infinity"';
			if (Object.is(value, -0)) return '"$-0"';
			return String(value);
		case "bigint":
			return JSON.stringify(`$bigint:${value}`);
		case "undefined":
			return '"$undefined"';
		case "object": {
			if (value instanceof Date) return JSON.stringify(`$date:${value.toISOString()}`);
			if (seen.has(value)) {
				throw new TypeError("Cached invoke arguments must not contain cycles");
			}
			seen.add(value);
			const serialized = Array.isArray(value)
				? `[${value.map((item) => stableSerialize(item, seen)).join(",")}]`
				: `{${Object.keys(value)
						.sort()
						.map(
							(key) =>
								`${JSON.stringify(key)}:${stableSerialize(
									(value as Record<string, unknown>)[key],
									seen,
								)}`,
						)
						.join(",")}}`;
			seen.delete(value);
			return serialized;
		}
		default:
			throw new TypeError(
				`Cached invoke arguments cannot contain ${typeof value} values`,
			);
	}
}

export function cachedInvokeKey({
	command,
	args,
}: CachedInvokeDescriptor): string {
	return `${JSON.stringify(command)}:${stableSerialize(args)}`;
}

/**
 * Process-lifetime cache for widget IPC reads.
 *
 * Entries survive React unmounts, while subscribers and scheduled refreshes do
 * not. One global fixed-window scheduler gathers every mounted hook signalled
 * by the same ingest burst, keeping the whole view fan-out on one >=5s cadence.
 */
export class CachedInvokeStore {
	readonly ttlMs: number;
	readonly coalesceMs: number;

	private readonly clock: CachedInvokeClock;
	private readonly entries = new Map<string, CachedInvokeEntry<unknown>>();
	private readonly pendingKeys = new Set<string>();
	private fanoutTimer: unknown | null = null;
	private lastFanoutStartedAt = Number.NEGATIVE_INFINITY;

	constructor({
		ttlMs = CACHED_INVOKE_TTL_MS,
		coalesceMs = CACHED_INVOKE_COALESCE_MS,
		clock = SYSTEM_CLOCK,
	}: {
		ttlMs?: number;
		coalesceMs?: number;
		clock?: CachedInvokeClock;
	} = {}) {
		if (coalesceMs < 5_000) {
			throw new RangeError("Cached invoke coalescing must be at least 5000ms");
		}
		this.ttlMs = ttlMs;
		this.coalesceMs = coalesceMs;
		this.clock = clock;
	}

	snapshot<T>(key: string): CachedInvokeSnapshot<T> {
		const entry = this.entries.get(key) as CachedInvokeEntry<T> | undefined;
		if (entry === undefined) {
			return {
				data: null,
				hasData: false,
				initialLoading: true,
				refreshing: false,
				error: null,
			};
		}
		return this.entrySnapshot(entry);
	}

	subscribe<T>(
		key: string,
		request: () => Promise<T>,
		subscriber: (notification: CachedInvokeNotification<T>) => void,
		invalidationEvents: readonly string[] = [],
	): () => void {
		const entry = this.entry<T>(key);
		entry.request = request;
		entry.invalidationEvents = new Set(invalidationEvents);
		entry.subscribers.add(subscriber);
		subscriber({ kind: "snapshot", snapshot: this.entrySnapshot(entry) });

		const fresh =
			entry.hasData && this.clock.now() - entry.acceptedAt <= this.ttlMs;
		if (!fresh && entry.inFlight === null) {
			if (entry.hasData) {
				this.pendingKeys.add(key);
				this.scheduleFanout();
			} else {
				this.start(entry);
			}
		}

		let subscribed = true;
		return () => {
			if (!subscribed) return;
			subscribed = false;
			entry.subscribers.delete(subscriber);
			if (entry.subscribers.size > 0) return;

			entry.request = null;
			this.pendingKeys.delete(key);
			this.cancelUnusedFanoutTimer();
		};
	}

	refresh<T>(key: string, request: () => Promise<T>): void {
		const entry = this.entry<T>(key);
		entry.request = request;
		if (entry.subscribers.size === 0) return;
		this.pendingKeys.add(key);
		this.scheduleFanout();
	}

	retry<T>(key: string, request: () => Promise<T>): void {
		const entry = this.entry<T>(key);
		entry.request = request;
		if (entry.subscribers.size === 0 || entry.inFlight !== null) return;
		this.pendingKeys.delete(key);
		this.start(entry);
	}

	invalidateEvent(eventName: string, refreshMounted = true): void {
		for (const [key, entry] of this.entries) {
			if (!entry.invalidationEvents.has(eventName)) continue;
			entry.acceptedAt = Number.NEGATIVE_INFINITY;
			entry.error = null;
			if (refreshMounted && entry.subscribers.size > 0) {
				this.pendingKeys.add(key);
			}
		}
		if (refreshMounted) this.scheduleFanout();
	}

	refreshStaleSubscribers(): void {
		for (const [key, entry] of this.entries) {
			if (
				entry.subscribers.size > 0 &&
				this.clock.now() - entry.acceptedAt > this.ttlMs
			) {
				this.pendingKeys.add(key);
			}
		}
		this.scheduleFanout();
	}

	debugState(): {
		entries: number;
		subscribers: number;
		pendingRefreshes: number;
		hasFanoutTimer: boolean;
	} {
		let subscribers = 0;
		for (const entry of this.entries.values()) {
			subscribers += entry.subscribers.size;
		}
		return {
			entries: this.entries.size,
			subscribers,
			pendingRefreshes: this.pendingKeys.size,
			hasFanoutTimer: this.fanoutTimer !== null,
		};
	}

	private entry<T>(key: string): CachedInvokeEntry<T> {
		let entry = this.entries.get(key) as CachedInvokeEntry<T> | undefined;
		if (entry !== undefined) return entry;

		entry = {
			key,
			hasData: false,
			data: undefined,
			serialized: null,
			acceptedAt: Number.NEGATIVE_INFINITY,
			error: null,
			inFlight: null,
			generation: 0,
			request: null,
			invalidationEvents: new Set(),
			subscribers: new Set(),
		};
		this.entries.set(key, entry as CachedInvokeEntry<unknown>);
		return entry;
	}

	private entrySnapshot<T>(
		entry: CachedInvokeEntry<T>,
	): CachedInvokeSnapshot<T> {
		return {
			data: entry.hasData ? (entry.data as T) : null,
			hasData: entry.hasData,
			initialLoading:
				!entry.hasData && (entry.inFlight !== null || entry.error === null),
			refreshing: entry.hasData && entry.inFlight !== null,
			error: entry.error,
		};
	}

	private notify<T>(
		entry: CachedInvokeEntry<T>,
		kind: CachedInvokeNotification<T>["kind"],
	): void {
		const notification = { kind, snapshot: this.entrySnapshot(entry) };
		for (const subscriber of entry.subscribers) subscriber(notification);
	}

	private start<T>(entry: CachedInvokeEntry<T>): boolean {
		const request = entry.request;
		if (
			request === null ||
			entry.subscribers.size === 0 ||
			entry.inFlight !== null
		) {
			return false;
		}

		this.pendingKeys.delete(entry.key);
		this.lastFanoutStartedAt = this.clock.now();
		entry.error = null;
		entry.generation += 1;
		const generation = entry.generation;
		const inFlight = Promise.resolve()
			.then(request)
			.then(
				(data) => {
					if (entry.generation !== generation) return;
					const serialized = stableSerialize(data);
					if (!entry.hasData || entry.serialized !== serialized) {
						entry.data = data;
						entry.serialized = serialized;
					}
					entry.hasData = true;
					entry.acceptedAt = this.clock.now();
					entry.error = null;
				},
				(error: unknown) => {
					if (entry.generation !== generation) return;
					entry.error = error;
				},
			)
			.finally(() => {
				if (entry.generation !== generation) return;
				entry.inFlight = null;
				this.notify(entry, entry.error === null ? "accepted" : "error");
				if (this.pendingKeys.has(entry.key)) this.scheduleFanout();
			});
		entry.inFlight = inFlight;
		this.notify(entry, "loading");
		return true;
	}

	private scheduleFanout(): void {
		if (this.fanoutTimer !== null || this.pendingKeys.size === 0) return;
		const elapsed = this.clock.now() - this.lastFanoutStartedAt;
		const delay = Math.max(0, this.coalesceMs - elapsed);
		this.fanoutTimer = this.clock.setTimer(() => {
			this.fanoutTimer = null;
			let blockedByInFlight = false;
			for (const key of [...this.pendingKeys]) {
				const entry = this.entries.get(key);
				if (entry === undefined || entry.subscribers.size === 0) {
					this.pendingKeys.delete(key);
					continue;
				}
				if (entry.inFlight !== null) {
					blockedByInFlight = true;
					continue;
				}
				this.start(entry);
			}
			if (this.pendingKeys.size > 0 && !blockedByInFlight) {
				this.scheduleFanout();
			}
		}, delay);
	}

	private cancelUnusedFanoutTimer(): void {
		if (this.pendingKeys.size > 0 || this.fanoutTimer === null) return;
		this.clock.clearTimer(this.fanoutTimer);
		this.fanoutTimer = null;
	}
}

export const cachedInvokeStore = new CachedInvokeStore();
