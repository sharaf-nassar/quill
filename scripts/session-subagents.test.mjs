import assert from "node:assert/strict";
import test from "node:test";
import { handleInvoke } from "../src/mocks/ipcFixtures.ts";
import {
	formatAdaptiveClockDurationSecs,
	formatClockDurationSecs,
	formatExtrapolatedRuntime,
	formatObservedSessionAgents,
	isSessionLive,
	resolveSessionMetrics,
} from "../src/utils/format.ts";

// @lat: [[live-subagent-count-tests#Live Subagent Count Tests#Terminal Hook Liveness]]
test("terminal hooks win ties while newer activity reopens", () => {
	const now = new Date("2030-01-01T00:04:00Z").getTime();
	assert.equal(isSessionLive("2030-01-01T00:00:00Z", null, now), true);
	assert.equal(isSessionLive(
		"2030-01-01T00:00:00Z",
		null,
		new Date("2030-01-01T00:05:00Z").getTime(),
	), false);
	assert.equal(isSessionLive(
		"2030-01-01T00:00:00Z",
		"2030-01-01T00:00:00Z",
		now,
	), false);
	assert.equal(isSessionLive(
		"2030-01-01T00:00:01Z",
		"2030-01-01T00:00:00Z",
		now,
	), true);
	assert.equal(isSessionLive("2030-01-01T00:00:00Z", "invalid", now), true);
});

// @lat: [[live-subagent-count-tests#Live Subagent Count Tests#Live Runtime Extrapolation]]
test("extrapolates runtime from producer time without running backward", () => {
	assert.equal(formatExtrapolatedRuntime(null, 1_000, 2, 3_500), "—");
	assert.equal(formatExtrapolatedRuntime(10, 1_000, 2, 3_500), "15s");
	assert.equal(formatExtrapolatedRuntime(10, 4_000, 2, 3_500), "10s");
	assert.equal(formatExtrapolatedRuntime(10, null, 2, 3_500), "10s");
	assert.equal(formatClockDurationSecs(null), "—");
	assert.equal(formatClockDurationSecs(0), "0 m");
	assert.equal(formatClockDurationSecs(420), "7 m");
	assert.equal(formatClockDurationSecs(12_179), "3:22");
	assert.equal(formatClockDurationSecs(101_279), "1:04:07");
	assert.equal(formatClockDurationSecs(8_640_000), "100:00:00");
	assert.equal(formatAdaptiveClockDurationSecs(null), "—");
	assert.equal(formatAdaptiveClockDurationSecs(0.9), "0s");
	assert.equal(formatAdaptiveClockDurationSecs(45.9), "45s");
	assert.equal(formatAdaptiveClockDurationSecs(202.9), "3:22");
	assert.equal(formatAdaptiveClockDurationSecs(3_847.9), "1:04:07");
	assert.equal(formatAdaptiveClockDurationSecs(101_229.9), "1:04:07:09");
	assert.equal(
		formatExtrapolatedRuntime(12_120, 1_000, 2, 30_500, formatClockDurationSecs),
		"3:22",
	);
	assert.equal(
		formatExtrapolatedRuntime(180, 1_000, 1, 23_900, formatAdaptiveClockDurationSecs),
		"3:22",
	);
});

// @lat: [[live-subagent-count-tests#Live Subagent Count Tests#Sessions Agent Runtime Rows]]
test("formats every open agent with ordered model and runtime identity", () => {
	assert.deepEqual(formatObservedSessionAgents("claude", null, 1_000, 3_000), []);
	assert.deepEqual(formatObservedSessionAgents("claude", [
		{ agent_id: "sonnet", model_id: "claude-sonnet-4-6", agent_type: null, runtime_secs: null, runtime_active: true },
		{ agent_id: "opus-b", model_id: "claude-opus-4-6", agent_type: null, runtime_secs: 272, runtime_active: false },
		{ agent_id: "opus-a", model_id: "claude-opus-4-6", agent_type: null, runtime_secs: 3_840, runtime_active: true },
	], 1_000, 3_000), [
		{
			agentId: "opus-a",
			model: "Opus",
			runtime: "1h 4m",
			ariaLabel: "claude-opus-4-6, agent opus-a, 1h 4m active runtime",
		},
		{
			agentId: "opus-b",
			model: "Opus",
			runtime: "4m 32s",
			ariaLabel: "claude-opus-4-6, agent opus-b, 4m 32s active runtime",
		},
		{
			agentId: "sonnet",
			model: "Sonnet",
			runtime: "—",
			ariaLabel: "claude-sonnet-4-6, agent sonnet, runtime unavailable",
		},
	]);
	assert.deepEqual(
		formatObservedSessionAgents("codex", [
			{ agent_id: "luna", model_id: "gpt-5.6-luna", agent_type: null, runtime_secs: 1, runtime_active: true },
			{ agent_id: "unknown", model_id: null, agent_type: null, runtime_secs: 2, runtime_active: false },
			{ agent_id: "terra", model_id: "gpt-5.6-terra", agent_type: null, runtime_secs: 3, runtime_active: false },
			{ agent_id: "sol", model_id: "gpt-5.6-sol", agent_type: null, runtime_secs: 4, runtime_active: true },
		], 1_000, 3_000).map(({ agentId, model, runtime }) => ({ agentId, model, runtime })),
		[
			{ agentId: "sol", model: "Sol", runtime: "6s" },
			{ agentId: "terra", model: "Terra", runtime: "3s" },
			{ agentId: "luna", model: "Luna", runtime: "3s" },
			{ agentId: "unknown", model: "?", runtime: "2s" },
		],
	);
});

// @lat: [[live-subagent-count-tests#Live Subagent Count Tests#Pi Agent Model Families]]
// @lat: [[pi-lineage-ui-tests#Pi Lineage UI Tests#Agent Role Identity]]
test("Pi agent rails reuse native model family labels", () => {
	assert.deepEqual(
		formatObservedSessionAgents("pi", [
			{ agent_id: "sol", model_id: "gpt-5.6-sol", agent_type: null, runtime_secs: 1, runtime_active: false },
			{ agent_id: "opus", model_id: "claude-opus-5", agent_type: null, runtime_secs: 1, runtime_active: false },
		], 1_000, 3_000).map(({ model, ariaLabel }) => ({ model, ariaLabel })),
		[
			{ model: "Opus", ariaLabel: "claude-opus-5, agent opus, 1s active runtime" },
			{ model: "Sol", ariaLabel: "gpt-5.6-sol, agent sol, 1s active runtime" },
		],
	);
	// A validated launcher role arrives as the agent type: the chip keeps the
	// model family ordering while the label names both role and raw model.
	assert.deepEqual(
		formatObservedSessionAgents("pi", [
			{ agent_id: "researcher", model_id: "gpt-5.6-sol", agent_type: "researcher", runtime_secs: 1, runtime_active: false },
			{ agent_id: "reviewer", model_id: "claude-opus-5", agent_type: "reviewer", runtime_secs: 1, runtime_active: false },
		], 1_000, 3_000).map(({ model, ariaLabel }) => ({ model, ariaLabel })),
		[
			{ model: "Opus", ariaLabel: "reviewer · claude-opus-5, agent reviewer, 1s active runtime" },
			{ model: "Sol", ariaLabel: "researcher · gpt-5.6-sol, agent researcher, 1s active runtime" },
		],
	);
});

// @lat: [[live-subagent-count-tests#Live Subagent Count Tests#Agent Rail Tooltip Identity]]
test("agent tooltips name the agent alongside its model", () => {
	assert.deepEqual(
		formatObservedSessionAgents("codex", [
			{ agent_id: "a", model_id: "gpt-5.6-sol", agent_type: "Curie", runtime_secs: 1, runtime_active: false },
			{ agent_id: "b", model_id: "gpt-5.6-sol", agent_type: null, runtime_secs: 1, runtime_active: false },
			{ agent_id: "c", model_id: null, agent_type: "Kepler", runtime_secs: 1, runtime_active: false },
			{ agent_id: "d", model_id: null, agent_type: null, runtime_secs: 1, runtime_active: false },
		], 1_000, 3_000).map(({ model, ariaLabel }) => ({ model, ariaLabel })),
		[
			// Both known: the chip still shows the model, so the name is only
			// readable in the tooltip.
			{ model: "Sol", ariaLabel: "Curie · gpt-5.6-sol, agent a, 1s active runtime" },
			{ model: "Sol", ariaLabel: "gpt-5.6-sol, agent b, 1s active runtime" },
			// No model: the name is the chip label and the whole identity.
			{ model: "Kepler", ariaLabel: "Kepler, agent c, 1s active runtime" },
			{ model: "?", ariaLabel: "Unknown model, agent d, 1s active runtime" },
		],
	);
});

test("Sessions fixtures expose lifetime and current-turn runtime evidence", () => {
	const rows = handleInvoke("get_session_breakdown");
	assert.ok(rows.every((row) => "active_runtime_secs" in row));
	assert.ok(rows.every((row) => "runtime_as_of_ms" in row));
	assert.ok(rows.every((row) => "active_runtime_rate" in row));
	assert.ok(rows.every((row) => "observed_agents" in row));
	assert.ok(rows.every((row) => "observed_only" in row));
	assert.ok(rows.every((row) => "ended_at" in row));
	assert.ok(rows.every((row) => !("observed_subagent_count" in row) && !("observed_subagent_models" in row)));
	assert.deepEqual(
		rows.map((row) => row.observed_agents?.length ?? row.observed_agents),
		[3, 2, 0, null, 0, 2],
	);
	assert.deepEqual(rows.map((row) => row.agent_count), [5, 2, 3, null, 0, null]);
	const now = Date.now();
	for (const row of rows) {
		const liveRoot = row.current_turn_runtime_active ? 1 : 0;
		const activeAgents = (row.observed_agents ?? []).filter(
			(agent) => agent.runtime_secs !== null && agent.runtime_active,
		).length;
		assert.equal(row.active_runtime_rate, liveRoot + activeAgents);
	}
	const liveAgents = rows.find((row) => row.observed_agents?.length === 3);
	assert.equal(liveAgents.active_runtime_secs, 4_823);
	assert.equal(liveAgents.current_turn_runtime_secs, 41);
	assert.equal(liveAgents.current_turn_runtime_active, true);
	assert.equal(liveAgents.agent_runtime_secs, 4_212);
	assert.equal(formatObservedSessionAgents(
		liveAgents.provider,
		liveAgents.observed_agents,
		liveAgents.runtime_as_of_ms,
		Date.now(),
	).length, 3);
});

// @lat: [[live-subagent-count-tests#Live Subagent Count Tests#Observed-Only Sessions Presentation]]
test("observed-only session metrics stay unavailable instead of reading as zero", () => {
	assert.deepEqual(resolveSessionMetrics("0", "0 turns", true), { tokens: "—", turns: null });
	assert.deepEqual(resolveSessionMetrics("12.3k", "7 turns", false), {
		tokens: "12.3k",
		turns: "7 turns",
	});
});

// @lat: [[pi-live-session-tests#Pi Live Session Test Specs#Cumulative Token Display]]
test("live Pi session tokens show pushed cumulative usage", () => {
	assert.deepEqual(resolveSessionMetrics("12.3k", "7 turns", false, "pi", true), {
		tokens: "12.3k",
		turns: "7 turns",
	});
});
