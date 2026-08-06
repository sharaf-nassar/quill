import assert from "node:assert/strict";
import test from "node:test";
import { handleInvoke } from "../src/mocks/ipcFixtures.ts";
import {
	formatObservedSubagentModels,
	resolveSessionMetrics,
} from "../src/utils/format.ts";

// @lat: [[live-subagent-count-tests#Live Subagent Count Tests#Positive-Only Sessions Rows]]
test("formats exact model groups with tier order and an honest unknown tail", () => {
	assert.equal(formatObservedSubagentModels("claude", null, null), null);
	assert.equal(formatObservedSubagentModels("codex", 0, []), null);
	assert.deepEqual(formatObservedSubagentModels("claude", 5, [
		{ model_id: "claude-sonnet-4-6", count: 3 },
		{ model_id: "claude-opus-4-6", count: 2 },
	]), {
		text: "2 × Opus · 3 × Sonnet",
		ariaLabel: "5 subagents observed open: 2 Opus agents, 3 Sonnet agents",
	});
	assert.deepEqual(formatObservedSubagentModels("codex", 6, [
		{ model_id: "gpt-5.6-luna", count: 2 },
		{ model_id: "gpt-5.6-sol", count: 2 },
		{ model_id: "gpt-5.6-sol-20260805", count: 1 },
		{ model_id: "gpt-5.6-terra", count: 1 },
	]), {
		text: "3 × Sol · 1 × Terra · 2 × Luna",
		ariaLabel: "6 subagents observed open: 3 Sol agents, 1 Terra agent, 2 Luna agents",
	});
	assert.deepEqual(formatObservedSubagentModels("claude", 3, [
		{ model_id: "claude-opus-4-6", count: 1 },
	]), {
		text: "1 × Opus · 2 × ?",
		ariaLabel: "3 subagents observed open: 1 Opus agent, 2 unresolved model agents",
	});
	assert.deepEqual(formatObservedSubagentModels("claude", 3, [
		{ model_id: "claude-fable-5", count: 1 },
		{ model_id: "future-claude", count: 2 },
	]), {
		text: "1 × Fable · 2 × future-claude",
		ariaLabel: "3 subagents observed open: 1 Fable agent, 2 future-claude agents",
	});
	assert.deepEqual(formatObservedSubagentModels("codex", 1, [
		{ model_id: "future-model", count: 2 },
	]), {
		text: "1 × ?",
		ariaLabel: "1 subagent observed open: 1 unresolved model agent",
	});
});

test("Sessions fixtures preserve null, zero, singular, and plural states", () => {
	const rows = handleInvoke("get_session_breakdown");
	assert.ok(rows.every((row) => "observed_subagent_count" in row));
	assert.ok(rows.every((row) => "observed_only" in row));
	assert.ok(rows.every((row) => !("subagent_count" in row) && !("has_subagents" in row)));
	assert.deepEqual(
		rows.map((row) => row.observed_subagent_count),
		[3, 0, null, 1, 2],
	);
	const idlePositive = rows.find((row) => row.observed_subagent_count === 1);
	assert.ok(Date.now() - new Date(idlePositive.last_active).getTime() > 5 * 60_000);
	assert.notEqual(formatObservedSubagentModels(
		idlePositive.provider,
		idlePositive.observed_subagent_count,
		idlePositive.observed_subagent_models,
	), null);
});

// @lat: [[live-subagent-count-tests#Live Subagent Count Tests#Observed-Only Sessions Presentation]]
test("observed-only session metrics stay unavailable instead of reading as zero", () => {
	assert.deepEqual(resolveSessionMetrics("0", "0 turns", true), { tokens: "—", turns: null });
	assert.deepEqual(resolveSessionMetrics("12.3k", "7 turns", false), {
		tokens: "12.3k",
		turns: "7 turns",
	});
});
