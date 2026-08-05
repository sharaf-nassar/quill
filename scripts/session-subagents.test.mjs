import assert from "node:assert/strict";
import test from "node:test";
import { readFileSync } from "node:fs";
import { handleInvoke } from "../src/mocks/ipcFixtures.ts";
import {
	formatObservedSubagentCount,
	resolveSessionMetrics,
} from "../src/utils/format.ts";

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
	assert.ok(rows.every((row) => "observed_only" in row));
	assert.ok(rows.every((row) => !("subagent_count" in row) && !("has_subagents" in row)));
	assert.deepEqual(
		rows.map((row) => row.observed_subagent_count),
		[3, 0, null, 1, 2],
	);
	const idlePositive = rows.find((row) => row.observed_subagent_count === 1);
	assert.ok(Date.now() - new Date(idlePositive.last_active).getTime() > 5 * 60_000);
	assert.notEqual(formatObservedSubagentCount(idlePositive.observed_subagent_count), null);
});

// @lat: [[live-subagent-count-tests#Live Subagent Count Tests#Observed-Only Sessions Presentation]]
test("observed-only session metrics stay unavailable instead of reading as zero", () => {
	assert.deepEqual(resolveSessionMetrics("0", "0 turns", true), { tokens: "—", turns: null });
	assert.deepEqual(resolveSessionMetrics("12.3k", "7 turns", false), {
		tokens: "12.3k",
		turns: "7 turns",
	});
});

// @lat: [[live-subagent-count-tests#Live Subagent Count Tests#Agent Count Icon Contract]]
test("agent count keeps row order and one centered accessible icon-count cluster", () => {
	const source = readFileSync("src/components/widget/views/UsageView.tsx", "utf8");
	const css = readFileSync("src/styles/index.css", "utf8");
	const name = source.indexOf('className="wg-row-name"');
	const agents = source.indexOf('className="wg-row-meta wg-num"', name);
	const provider = source.indexOf('className="wg-row-chip"', agents);
	const tokens = source.indexOf('className="wg-row-value wg-num"', provider);
	const recency = source.indexOf('className="wg-row-ago wg-num"', tokens);

	assert.ok(name < agents && agents < provider && provider < tokens && tokens < recency);
	assert.match(source, /className="wg-row-agent-icon"[\s\S]*?aria-hidden="true"/);
	assert.match(source, /className="wg-row-meta wg-num"[\s\S]*?aria-label=\{row\.metaLabel\}/);
	assert.match(css, /\.wg-row-meta\[data-agent="true"\][\s\S]*?display: inline-flex;[\s\S]*?align-items: center;[\s\S]*?justify-content: center;/);
	assert.match(css, /\.wg-row-meta\[data-agent="true"\][\s\S]*?color: var\(--provider-agent\);/);
});
