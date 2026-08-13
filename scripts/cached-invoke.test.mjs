import assert from "node:assert/strict";
import test from "node:test";
import {
	CACHED_INVOKE_COALESCE_MS,
	CachedInvokeStore,
	cachedInvokeKey,
} from "../src/hooks/cachedInvokeStore.ts";
import {
	breakdownQuery,
	codeInsightsHistoryQueries,
	usageBreakdownQueries,
	weeklyTrendQueries,
} from "../src/hooks/widgetQueryPlan.ts";

async function settle() {
	for (let index = 0; index < 8; index += 1) await Promise.resolve();
}

function mockClock(t) {
	t.mock.timers.enable({ apis: ["setTimeout", "Date"] });
	return t.mock.timers;
}

// @lat: [[frontend-cache-tests#Frontend Invoke Cache Tests#Fresh remount skips fan-out]]
test("fresh remount renders cached data without another request", async (t) => {
	const clock = mockClock(t);
	const store = new CachedInvokeStore();
	const key = cachedInvokeKey({ command: "get_token_stats", args: { range: "6h" } });
	const queryLog = [];
	const request = async () => {
		queryLog.push({ atMs: Date.now(), command: "get_token_stats", range: "6h" });
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
	const store = new CachedInvokeStore();
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
test("listener registration does not synthesize cache invalidations", async (t) => {
	const clock = mockClock(t);
	const store = new CachedInvokeStore();
	const key = cachedInvokeKey({ command: "get_token_stats", args: { range: "6h" } });
	const queryLog = [];
	const callbacks = new Map();
	const stop = store.subscribe(
		key,
		async () => {
			queryLog.push({ atMs: Date.now(), command: "get_token_stats" });
			return { total: 42 };
		},
		() => {},
		["tokens-updated"],
	);
	await settle();

	const listenerPromises = ["tokens-updated"].map(async (eventName) => {
		const callback = () => store.invalidateEvent(eventName);
		callbacks.set(eventName, callback);
		return () => {};
	});
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
test("continuous ingest storms refresh one mounted fan-out every 5000ms or later", async (t) => {
	assert.ok(CACHED_INVOKE_COALESCE_MS >= 5_000);
	const clock = mockClock(t);
	const store = new CachedInvokeStore();
	const queryLog = [];
	const descriptors = [
		{ command: "get_token_stats", args: { range: "6h" }, event: "tokens-updated" },
		{
			command: "get_project_breakdown",
			args: { range: "6h" },
			event: "tokens-updated",
		},
		{
			command: "get_session_breakdown",
			args: { range: "6h" },
			event: "sessions-live-updated",
		},
	];
	const stops = descriptors.map(({ event, ...descriptor }) => {
		const key = cachedInvokeKey(descriptor);
		return store.subscribe(
			key,
			async () => {
				queryLog.push({ atMs: Date.now(), ...descriptor });
				return { ok: true };
			},
			() => {},
			[event],
		);
	});
	await settle();

	for (let second = 1; second <= 12; second += 1) {
		clock.tick(1_000);
		store.invalidateEvent("tokens-updated");
		store.invalidateEvent("sessions-live-updated");
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
	for (const descriptor of descriptors.slice(1)) {
		assert.equal(
			queryLog.filter((row) => row.command === descriptor.command).at(-1).atMs,
			15_000,
		);
	}
	console.info(`[query-window] ${JSON.stringify(queryLog)}`);
	stops[1]();
	stops[2]();
});

// @lat: [[frontend-cache-tests#Frontend Invoke Cache Tests#Transcript runtime refresh is immediate]]
test("transcript runtime refresh bypasses fan-out only for session breakdown", async (t) => {
	const clock = mockClock(t);
	const store = new CachedInvokeStore();
	const queryLog = [];
	let sessionCalls = 0;
	let resolveSession;
	const subscribe = (command) =>
		store.subscribe(
			cachedInvokeKey({ command, args: { range: "6h" } }),
			async () => {
				queryLog.push({ atMs: Date.now(), command });
				if (command === "get_session_breakdown" && ++sessionCalls > 1) {
					return new Promise((resolve) => {
						resolveSession = resolve;
					});
				}
				return { ok: true };
			},
			() => {},
			["transcript-analytics-updated"],
		);
	const stopSessions = subscribe("get_session_breakdown");
	const stopProjects = subscribe("get_project_breakdown");
	await settle();

	clock.tick(1_000);
	store.invalidateEvent(
		"transcript-analytics-updated",
		true,
		"get_session_breakdown",
	);
	await settle();
	assert.deepEqual(queryLog, [
		{ atMs: 0, command: "get_session_breakdown" },
		{ atMs: 0, command: "get_project_breakdown" },
		{ atMs: 1_000, command: "get_session_breakdown" },
	]);
	store.invalidateEvent(
		"transcript-analytics-updated",
		true,
		"get_session_breakdown",
	);
	store.invalidateEvent(
		"transcript-analytics-updated",
		true,
		"get_session_breakdown",
	);
	await settle();
	assert.equal(sessionCalls, 2);
	resolveSession({ ok: true });
	await settle();
	assert.equal(sessionCalls, 3);
	resolveSession({ ok: true });
	await settle();

	clock.tick(4_000);
	await settle();
	assert.deepEqual(queryLog.at(-1), {
		atMs: 5_000,
		command: "get_project_breakdown",
	});
	stopSessions();
	stopProjects();
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
	const comparisonA = cachedInvokeKey({
		command: "get_token_history",
		args: { range: "12h", provider: null },
	});
	const comparisonB = cachedInvokeKey({
		command: "get_token_history",
		args: { provider: null, range: "12h" },
	});

	assert.equal(sixHourA, sixHourB);
	assert.notEqual(sixHourA, day);
	assert.equal(comparisonA, comparisonB);
	assert.notEqual(sixHourA, comparisonA);
});

// @lat: [[widget-range-tests#Widget Range Query Tests#Displayed Windows Bound Every Query]]
test("widget query plans request only the displayed range or its exact prior period", () => {
	const queryLog = [];
	for (const [displayedRange, requestedRange] of [
		["1h", "2h"],
		["6h", "12h"],
		["24h", "2d"],
		["7d", "14d"],
	]) {
		const queries = codeInsightsHistoryQueries(displayedRange).map(
			({ command, args }) => ({ command, args }),
		);
		assert.deepEqual(queries, [
			{
				command: "get_token_history",
				args: { range: requestedRange, hostname: null, sessionId: null, cwd: null },
			},
			{ command: "get_code_stats_history", args: { range: requestedRange } },
			{ command: "get_llm_runtime_stats", args: { range: requestedRange } },
		]);
		queryLog.push({ displayedRange, queries });
	}

	assert.deepEqual(
		weeklyTrendQueries().map(({ command, args }) => ({ command, args })),
		[
			{
				command: "get_token_history",
				args: {
					range: "14d",
					provider: null,
					hostname: null,
					sessionId: null,
					cwd: null,
				},
			},
			{ command: "get_code_stats_history", args: { range: "14d" } },
			{ command: "get_llm_runtime_stats", args: { range: "14d" } },
		],
	);
	assert.deepEqual(breakdownQuery("skills", "6h"), {
		command: "get_skill_breakdown",
		args: { range: "6h", provider: null, allTime: false, limit: 100 },
	});
	console.info(`[range-query-window] ${JSON.stringify(queryLog)}`);
});

// @lat: [[widget-range-tests#Widget Range Query Tests#Breakdown Transitions Issue Unique Reads]]
test("breakdown transitions keep one project request and range-scope skills", () => {
	const transitionLog = ["sessions", "projects", "skills"].map((mode) => {
		const queries = usageBreakdownQueries(mode, "6h");
		const keys = queries.map(({ command, args }) => cachedInvokeKey({ command, args }));
		assert.equal(new Set(keys).size, keys.length);
		assert.equal(
			queries.filter(({ command }) => command === "get_project_breakdown").length,
			1,
		);
		return {
			mode,
			queries: queries.map(({ command, args }) => ({ command, args })),
		};
	});

	assert.deepEqual(transitionLog[1], {
		mode: "projects",
		queries: [{ command: "get_project_breakdown", args: { range: "6h" } }],
	});
	assert.deepEqual(transitionLog[2].queries[0], {
		command: "get_skill_breakdown",
		args: { range: "6h", provider: null, allTime: false, limit: 100 },
	});
	console.info(`[breakdown-query-transition] ${JSON.stringify(transitionLog)}`);
});

// @lat: [[frontend-cache-tests#Frontend Invoke Cache Tests#Errors retry without poisoning cache]]
test("a rejected request retains stale data and immediate retry recovers", async (t) => {
	const clock = mockClock(t);
	const store = new CachedInvokeStore();
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
test("Strict Mode-style cleanup releases cache timers", async () => {
	const store = new CachedInvokeStore();
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
});
