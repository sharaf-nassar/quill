import assert from "node:assert/strict";
import test from "node:test";
import { handleInvoke } from "../src/mocks/ipcFixtures.ts";
import { formatObservedSubagentCount } from "../src/utils/format.ts";

// @lat: [[live-subagent-count-tests#Live Subagent Count Tests#Positive-Only Sessions Rows]]
test("formats observed subagent counts without inventing unknown or zero", () => {
	assert.equal(formatObservedSubagentCount(null), null);
	assert.equal(formatObservedSubagentCount(0), null);
	assert.deepEqual(formatObservedSubagentCount(1), {
		text: "+1",
		ariaLabel: "1 subagent observed open",
	});
	assert.deepEqual(formatObservedSubagentCount(12), {
		text: "+12",
		ariaLabel: "12 subagents observed open",
	});
});

test("Sessions fixtures preserve null, zero, singular, and plural states", () => {
	const rows = handleInvoke("get_session_breakdown");
	assert.ok(rows.every((row) => "observed_subagent_count" in row));
	assert.ok(rows.every((row) => !("subagent_count" in row) && !("has_subagents" in row)));
	assert.deepEqual(
		rows.map((row) => row.observed_subagent_count),
		[3, 0, null, 1],
	);
	const idlePositive = rows.find((row) => row.observed_subagent_count === 1);
	assert.ok(Date.now() - new Date(idlePositive.last_active).getTime() > 5 * 60_000);
	assert.notEqual(formatObservedSubagentCount(idlePositive.observed_subagent_count), null);
});
