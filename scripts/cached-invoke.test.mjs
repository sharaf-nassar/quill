import assert from "node:assert/strict";
import test from "node:test";
import {
	CACHED_INVOKE_COALESCE_MS,
	CachedInvokeStore,
	cachedInvokeKey,
	cleanupInvokeListeners,
	registerInvokeEventListeners,
} from "../src/hooks/cachedInvokeStore.ts";

class FakeClock {
	nowMs = 0;
	nextId = 1;
	timers = new Map();

	now = () => this.nowMs;

	setTimer = (callback, delayMs) => {
		const id = this.nextId++;
		this.timers.set(id, { at: this.nowMs + delayMs, callback });
		return id;
	};

	clearTimer = (id) => {
		this.timers.delete(id);
	};

	tick(ms) {
		const target = this.nowMs + ms;
		for (;;) {
			const due = [...this.timers.entries()]
				.filter(([, timer]) => timer.at <= target)
				.sort((left, right) => left[1].at - right[1].at || left[0] - right[0])[0];
			if (!due) break;
			const [id, timer] = due;
			this.timers.delete(id);
			this.nowMs = timer.at;
			timer.callback();
		}
		this.nowMs = target;
	}
}

async function settle() {
	for (let index = 0; index < 8; index += 1) await Promise.resolve();
}

function makeStore(clock) {
	return new CachedInvokeStore({ clock });
}

// @lat: [[frontend-cache-tests#Frontend Invoke Cache Tests#Fresh remount skips fan-out]]
test("fresh remount renders cached data without another request", async () => {
	const clock = new FakeClock();
	const store = makeStore(clock);
	const key = cachedInvokeKey({ command: "get_token_stats", args: { range: "6h" } });
	const queryLog = [];
	const request = async () => {
		queryLog.push({ atMs: clock.now(), command: "get_token_stats", range: "6h" });
		return { total: 42 };
	};

	const stopFirst = store.subscribe(key, request, () => {}, ["tokens-updated"]);
	await settle();
	stopFirst();

	const snapshots = [];
	const stopSecond = store.subscribe(
		key,
		request,
		(notification) => snapshots.push(notification.snapshot),
		["tokens-updated"],
	);
	await settle();

	assert.equal(queryLog.length, 1);
	assert.deepEqual(snapshots[0].data, { total: 42 });
	assert.equal(snapshots[0].initialLoading, false);
	stopSecond();

	store.invalidateEvent("tokens-updated");
	const staleSnapshots = [];
	const stopThird = store.subscribe(
		key,
		request,
		(notification) => staleSnapshots.push(notification.snapshot),
		["tokens-updated"],
	);
	await settle();
	assert.deepEqual(staleSnapshots[0].data, { total: 42 });
	assert.equal(queryLog.length, 1);
	clock.tick(5_000);
	await settle();
	assert.equal(queryLog.length, 2);
	stopThird();
});

// @lat: [[frontend-cache-tests#Frontend Invoke Cache Tests#Concurrent subscribers coalesce]]
test("concurrent subscribers share one in-flight request", async () => {
	const clock = new FakeClock();
	const store = makeStore(clock);
	const key = cachedInvokeKey({ command: "get_activity_series", args: { range: "6h" } });
	let resolveRequest;
	let calls = 0;
	const request = () => {
		calls += 1;
		return new Promise((resolve) => {
			resolveRequest = resolve;
		});
	};

	const stopFirst = store.subscribe(key, request, () => {});
	const stopSecond = store.subscribe(key, request, () => {});
	await settle();
	assert.equal(calls, 1);

	resolveRequest({ values: [1, 2] });
	await settle();
	assert.deepEqual(store.snapshot(key).data, { values: [1, 2] });
	stopFirst();
	stopSecond();
});

// @lat: [[frontend-cache-tests#Frontend Invoke Cache Tests#Listener setup stays query-silent]]
test("listener registration does not synthesize cache invalidations", async () => {
	const clock = new FakeClock();
	const store = makeStore(clock);
	const key = cachedInvokeKey({ command: "get_token_stats", args: { range: "6h" } });
	const queryLog = [];
	const callbacks = new Map();
	const stop = store.subscribe(
		key,
		async () => {
			queryLog.push({ atMs: clock.now(), command: "get_token_stats" });
			return { total: 42 };
		},
		() => {},
		["tokens-updated"],
	);
	await settle();

	const listenerPromises = registerInvokeEventListeners(
		["tokens-updated"],
		async (eventName, callback) => {
			callbacks.set(eventName, callback);
			return () => {};
		},
		(eventName) => store.invalidateEvent(eventName),
	);
	await Promise.all(listenerPromises);
	clock.tick(CACHED_INVOKE_COALESCE_MS + 1);
	await settle();
	assert.deepEqual(queryLog.map((row) => row.atMs), [0]);

	callbacks.get("tokens-updated")({ payload: undefined });
	clock.tick(0);
	await settle();
	assert.deepEqual(queryLog.map((row) => row.atMs), [0, 5_001]);
	console.info(`[registration-query-window] ${JSON.stringify(queryLog)}`);
	stop();
});

// @lat: [[frontend-cache-tests#Frontend Invoke Cache Tests#Ingest storms keep one cadence]]
test("continuous ingest storms refresh one mounted fan-out every 5000ms or later", async () => {
	assert.ok(CACHED_INVOKE_COALESCE_MS >= 5_000);
	const clock = new FakeClock();
	const store = makeStore(clock);
	const queryLog = [];
	const descriptors = [
		{ command: "get_token_stats", args: { range: "6h" } },
		{ command: "get_project_breakdown", args: { range: "6h" } },
	];
	const stops = descriptors.map((descriptor) => {
		const key = cachedInvokeKey(descriptor);
		return store.subscribe(
			key,
			async () => {
				queryLog.push({ atMs: clock.now(), ...descriptor });
				return { ok: true };
			},
			() => {},
			["tokens-updated"],
		);
	});
	await settle();

	for (let second = 1; second <= 12; second += 1) {
		clock.tick(1_000);
		store.invalidateEvent("tokens-updated");
		await settle();
	}

	for (const descriptor of descriptors) {
		const times = queryLog
			.filter((row) => row.command === descriptor.command)
			.map((row) => row.atMs);
		assert.deepEqual(times, [0, 5_000, 10_000]);
		assert.ok(times.slice(1).every((time, index) => time - times[index] >= 5_000));
	}

	stops[0]();
	assert.equal(store.debugState().hasFanoutTimer, true);
	clock.tick(3_000);
	await settle();
	assert.equal(
		queryLog.filter((row) => row.command === descriptors[0].command).length,
		3,
	);
	assert.equal(
		queryLog.filter((row) => row.command === descriptors[1].command).at(-1).atMs,
		15_000,
	);
	console.info(`[query-window] ${JSON.stringify(queryLog)}`);
	stops[1]();
});

// @lat: [[frontend-cache-tests#Frontend Invoke Cache Tests#Arguments isolate cache entries]]
test("stable argument keys coalesce object order but isolate changed ranges", async () => {
	const sixHourA = cachedInvokeKey({
		command: "get_token_history",
		args: { range: "6h", provider: null },
	});
	const sixHourB = cachedInvokeKey({
		command: "get_token_history",
		args: { provider: null, range: "6h" },
	});
	const day = cachedInvokeKey({
		command: "get_token_history",
		args: { provider: null, range: "24h" },
	});

	assert.equal(sixHourA, sixHourB);
	assert.notEqual(sixHourA, day);
});

// @lat: [[frontend-cache-tests#Frontend Invoke Cache Tests#Errors retry without poisoning cache]]
test("a rejected request retains stale data and immediate retry recovers", async () => {
	const clock = new FakeClock();
	const store = makeStore(clock);
	const key = cachedInvokeKey({ command: "get_context_savings_analytics", args: { range: "6h" } });
	let attempt = 0;
	const request = async () => {
		attempt += 1;
		if (attempt === 2) throw new Error("transient");
		return { attempt };
	};
	const stop = store.subscribe(key, request, () => {}, ["context-savings-updated"]);
	await settle();
	assert.deepEqual(store.snapshot(key).data, { attempt: 1 });

	clock.tick(5_000);
	store.invalidateEvent("context-savings-updated");
	clock.tick(0);
	await settle();
	assert.deepEqual(store.snapshot(key).data, { attempt: 1 });
	assert.match(String(store.snapshot(key).error), /transient/);

	store.retry(key, request);
	await settle();
	assert.deepEqual(store.snapshot(key).data, { attempt: 3 });
	assert.equal(store.snapshot(key).error, null);
	stop();
});

// @lat: [[frontend-cache-tests#Frontend Invoke Cache Tests#Strict Mode cleanup releases resources]]
test("Strict Mode-style cleanup leaks neither registrations nor timers", async () => {
	const clock = new FakeClock();
	const store = makeStore(clock);
	const key = cachedInvokeKey({ command: "get_llm_runtime_stats", args: { range: "6h" } });
	let calls = 0;
	const request = async () => ({ calls: ++calls });

	const stopProbe = store.subscribe(key, request, () => {});
	stopProbe();
	const stopMounted = store.subscribe(key, request, () => {});
	await settle();
	assert.equal(calls, 1);
	store.refresh(key, request);
	stopMounted();
	assert.deepEqual(store.debugState(), {
		entries: 1,
		subscribers: 0,
		pendingRefreshes: 0,
		hasFanoutTimer: false,
	});

	let resolveListener;
	let unlistenCalls = 0;
	const listener = new Promise((resolve) => {
		resolveListener = resolve;
	});
	const cleanup = cleanupInvokeListeners([listener]);
	cleanup();
	cleanup();
	resolveListener(() => {
		unlistenCalls += 1;
	});
	await settle();
	assert.equal(unlistenCalls, 1);
});
