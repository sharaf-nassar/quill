import assert from "node:assert/strict";
import test from "node:test";
import { readFileSync } from "node:fs";
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
		text: "2×Opus · 3×Sonnet",
		ariaLabel: "5 subagents observed open: 2 Opus agents, 3 Sonnet agents",
	});
	assert.deepEqual(formatObservedSubagentModels("codex", 6, [
		{ model_id: "gpt-5.6-luna", count: 2 },
		{ model_id: "gpt-5.6-sol", count: 2 },
		{ model_id: "gpt-5.6-sol-20260805", count: 1 },
		{ model_id: "gpt-5.6-terra", count: 1 },
	]), {
		text: "3×Sol · 1×Terra · 2×Luna",
		ariaLabel: "6 subagents observed open: 3 Sol agents, 1 Terra agent, 2 Luna agents",
	});
	assert.deepEqual(formatObservedSubagentModels("claude", 3, [
		{ model_id: "claude-opus-4-6", count: 1 },
	]), {
		text: "1×Opus · 2×?",
		ariaLabel: "3 subagents observed open: 1 Opus agent, 2 unresolved model agents",
	});
	assert.deepEqual(formatObservedSubagentModels("codex", 1, [
		{ model_id: "future-model", count: 2 },
	]), {
		text: "1×?",
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

// @lat: [[live-subagent-count-tests#Live Subagent Count Tests#Agent Model Row Contract]]
test("agent models keep row order and accessible icon-dot-groups structure", () => {
	const source = readFileSync("src/components/widget/views/UsageView.tsx", "utf8");
	const css = readFileSync("src/styles/index.css", "utf8");
	const name = source.indexOf('className="wg-row-name"');
	const agents = source.indexOf('className="wg-row-meta wg-num"', name);
	const provider = source.indexOf('className="wg-row-chip"', agents);
	const tokens = source.indexOf('className="wg-row-value wg-num"', provider);
	const recency = source.indexOf('className="wg-row-ago wg-num"', tokens);
	const agentMarkup = source.slice(agents, provider);

	assert.ok(name < agents && agents < provider && provider < tokens && tokens < recency);
	const icon = agentMarkup.indexOf('className="wg-row-agent-icon"');
	const separator = agentMarkup.indexOf('className="wg-row-agent-separator"');
	const models = agentMarkup.indexOf('"wg-row-agent-models"');
	assert.ok(icon < separator && separator < models);
	assert.match(source, /className="wg-row-agent-icon"[\s\S]*?aria-hidden="true"/);
	assert.match(agentMarkup, /viewBox="0 0 12 12"[\s\S]*?<rect x="1\.5" y="2\.75" width="9" height="7\.5" rx="2" \/>[\s\S]*?<circle cx="4\.25"[\s\S]*?<circle cx="7\.75"/);
	assert.match(source, /className="wg-row-meta wg-num"[\s\S]*?aria-label=\{row\.metaLabel\}[\s\S]*?title=\{row\.metaLabel\}/);
	assert.match(css, /\.wg-row-meta\[data-agent="true"\][\s\S]*?display: inline-flex;[\s\S]*?align-self: stretch;[\s\S]*?align-items: center;[\s\S]*?min-width: 0;[\s\S]*?max-width: 160px;[\s\S]*?flex: 0 1 auto;[\s\S]*?line-height: 1;/);
	assert.match(css, /\.wg-row-meta\[data-agent="true"\][\s\S]*?color: var\(--provider-agent\);/);
	assert.match(css, /\.wg-row\[data-agents="true"\] \.wg-row-name[\s\S]*?min-width: 48px;/);
	assert.match(css, /\.wg-row-agent-models[\s\S]*?overflow: hidden;[\s\S]*?text-overflow: ellipsis;[\s\S]*?white-space: nowrap;/);
});
