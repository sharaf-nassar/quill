import assert from "node:assert/strict";
import test from "node:test";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { createServer } from "vite";

const NOW = Date.now();
const baseRow = {
	provider: "claude",
	session_id: "session",
	hostname: "host",
	total_tokens: 10,
	turn_count: 0,
	first_seen: "2026-08-13T00:00:00Z",
	last_active: "2099-01-01T00:00:00Z",
	ended_at: null,
	project: "active-retained",
	active_runtime_secs: 60,
	agent_count: 0,
	agent_runtime_secs: 0,
	current_turn_runtime_secs: 10,
	current_turn_runtime_active: true,
	runtime_as_of_ms: NOW,
	active_runtime_rate: 1,
	observed_agents: [],
	observed_only: false,
};

const rows = [
	baseRow,
	{
		...baseRow,
		session_id: "active-retained-plural",
		project: "active-retained-plural",
		turn_count: 2,
	},
	{
		...baseRow,
		session_id: "inactive",
		project: "inactive",
		turn_count: 3,
		last_active: "2020-01-01T00:00:00Z",
		ended_at: "2020-01-01T00:00:00Z",
		current_turn_runtime_active: false,
	},
	{
		...baseRow,
		session_id: "observed-active",
		project: "observed-active",
		total_tokens: 0,
		observed_only: true,
	},
	{
		...baseRow,
		session_id: "observed-no-root",
		project: "observed-no-root",
		total_tokens: 0,
		current_turn_runtime_secs: null,
		current_turn_runtime_active: false,
		active_runtime_rate: 0,
		observed_only: true,
	},
];

const server = await createServer({
	appType: "custom",
	server: { middlewareMode: true, hmr: false },
	plugins: [{
		name: "session-row-fixture",
		transform(code, id) {
			return id.endsWith("/UsageView.tsx")
				? `${code}\nexport { sessionRow, SessionMetrics, SessionIdentity };`
				: null;
		},
	}],
});

const { sessionRow, SessionMetrics, SessionIdentity } = await server.ssrLoadModule(
	"/src/components/widget/views/UsageView.tsx",
);

test.after(() => server.close());

function rowMarkup(row) {
	return renderToStaticMarkup(createElement(SessionMetrics, { row: sessionRow(row, NOW) }));
}

// @lat: [[live-subagent-count-tests#Live Subagent Count Tests#Active Root Turn Presentation]]
test("session rows include only evidenced active root turns and cue their total runtime", () => {
	const active = rowMarkup(rows[0]);
	const activePlural = rowMarkup(rows[1]);
	const inactive = rowMarkup(rows[2]);
	const observedActive = rowMarkup(rows[3]);
	const observedNoRoot = rowMarkup(rows[4]);

	assert.match(active, /aria-label="1 main-session turn including active turn"/);
	assert.match(active, /data-live="true" data-tooltip="Total runtime"/);
	assert.match(activePlural, /aria-label="3 main-session turns including active turn"/);
	assert.match(inactive, /aria-label="3 completed main-session turns"/);
	assert.doesNotMatch(inactive, /data-live="true"/);
	assert.match(observedActive, /aria-label="1 main-session turn including active turn"/);
	assert.match(observedNoRoot, /aria-label="Main-session turn count unavailable"/);
});

// @lat: [[live-subagent-count-tests#Live Subagent Count Tests#Session Row Column Layout]]
test("session identity places the full provider name directly after the session name", () => {
	const markup = (provider) => renderToStaticMarkup(createElement(SessionIdentity, {
		row: sessionRow({ ...baseRow, provider }, NOW),
	}));
	const claude = markup("claude");

	assert.match(
		claude,
		/^<span class="wg-row-session-identity"><span class="wg-row-name-tip[\s\S]*<span class="wg-row-session-provider wg-row-datum"[^>]*aria-label="Provider CLAUDE">CLAUDE<\/span>/,
	);
	assert.match(markup("codex"), /aria-label="Provider CODEX">CODEX<\/span>/);
	assert.match(markup("mini_max"), /aria-label="Provider MINIMAX">MINIMAX<\/span>/);
	assert.doesNotMatch(claude, /wg-row-chip/);
});

// @lat: [[pi-live-session-tests#Pi Live Session Test Specs#Persisted Source Presentation]]
test("session identity never presents persisted Pi work as ephemeral", () => {
	const markup = renderToStaticMarkup(createElement(SessionIdentity, {
		row: sessionRow({ ...baseRow, provider: "pi", ephemeral: true }, NOW),
	}));

	assert.doesNotMatch(markup, /EPHEMERAL|wg-row-session-ephemeral/);
});
