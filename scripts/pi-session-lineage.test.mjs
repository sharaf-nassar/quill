import assert from "node:assert/strict";
import test from "node:test";
import * as React from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { createServer } from "vite";

const { createElement } = React;

const server = await createServer({
	appType: "custom",
	server: { middlewareMode: true, hmr: false },
	plugins: [{
		name: "expose-pi-lineage-ui",
		transform(code, id) {
			if (id.endsWith("/UsageView.tsx")) {
				return `${code}\nexport { sessionRow, LiveLinkedSessionRail, SessionIdentity };`;
			}
			return null;
		},
	}],
});

const [{ sessionRow, LiveLinkedSessionRail, SessionIdentity }, { ParentSessionLink }, { default: SearchBar }] =
	await Promise.all([
		server.ssrLoadModule("/src/components/widget/views/UsageView.tsx"),
		server.ssrLoadModule("/src/components/sessions/ResultCard.tsx"),
		server.ssrLoadModule("/src/components/sessions/SearchBar.tsx"),
	]);

test.after(() => server.close());

const hit = {
	provider: "pi",
	message_id: "message",
	session_id: "child-session",
	parent_session_id: "parent-session",
	content: "needle",
	snippet: "needle",
	role: "user",
	project: "quill",
	host: "host",
	git_branch: "",
	timestamp: "2026-08-14T08:00:00Z",
	tools_used: "",
	files_modified: "",
	code_changes: "",
	commands_run: "",
	tool_details: "",
	score: 1,
};

// @lat: [[pi-lineage-ui-tests#Pi Lineage UI Tests#Search Parent Navigation]]
test("Pi search rows and details render accessible parent navigation", () => {
	const markup = renderToStaticMarkup(createElement(ParentSessionLink, {
		parentSessionId: hit.parent_session_id,
		onNavigateSession() {},
	}));

	assert.match(markup, /aria-label="Open parent Pi session parent-session"/);
	assert.match(markup, />parent parent-se</);
});

// @lat: [[pi-lineage-ui-tests#Pi Lineage UI Tests#Immediate Search Input]]
test("session search reports each input change immediately", (t) => {
	const internals = React.__CLIENT_INTERNALS_DO_NOT_USE_OR_WARN_USERS_THEY_CANNOT_UPGRADE;
	const previousDispatcher = internals.H;
	const cleanups = [];
	internals.H = {
		useCallback: (callback) => callback,
		useEffect: (effect) => cleanups.push(effect()),
		useRef: (initialValue) => ({ current: initialValue }),
	};

	let input;
	try {
		input = SearchBar({ value: "", onSearch: (value) => searches.push(value) });
	} finally {
		internals.H = previousDispatcher;
	}
	const searches = [];
	t.after(() => cleanups.forEach((cleanup) => cleanup?.()));

	input.props.onChange({ target: { value: "pi" } });

	assert.deepEqual(searches, ["pi"]);
});

// @lat: [[pi-lineage-ui-tests#Pi Lineage UI Tests#Live Linked Session Copy]]
test("Pi live lineage uses linked-session copy and never agent-count copy", () => {
	const row = sessionRow({
		provider: "pi",
		session_id: "parent-session",
		parent_session_id: null,
		hostname: "host",
		total_tokens: 10,
		turn_count: 1,
		first_seen: "2026-08-14T08:00:00Z",
		last_active: "2099-01-01T00:00:00Z",
		ended_at: null,
		project: "/work/quill",
		active_runtime_secs: null,
		agent_count: null,
		agent_runtime_secs: null,
		current_turn_runtime_secs: null,
		current_turn_runtime_active: false,
		runtime_as_of_ms: null,
		active_runtime_rate: 0,
		observed_agents: [],
		live_linked_sessions: [
			{ session_id: "child-a", model_id: "claude-sonnet-4-5" },
			{ session_id: "child-b", model_id: "gpt-5.6" },
		],
		observed_only: false,
	}, Date.now());
	const identity = renderToStaticMarkup(createElement(SessionIdentity, { row }));
	const rail = renderToStaticMarkup(createElement(LiveLinkedSessionRail, {
		sessions: row.linkedSessions,
	}));
	const markup = identity + rail;

	assert.match(markup, /aria-label="2 live linked sessions"/);
	assert.match(markup, /aria-label="Live linked sessions"/);
	assert.match(markup, />2 live linked sessions</);
	assert.doesNotMatch(markup, /subagent|native agent|total agents/i);
});

// @lat: [[pi-lineage-ui-tests#Pi Lineage UI Tests#Singular Linked Session Copy]]
test("Pi live lineage uses singular copy for one child", () => {
	const markup = renderToStaticMarkup(createElement(LiveLinkedSessionRail, {
		sessions: [{ sessionId: "only-child", modelId: null }],
	}));
	assert.match(markup, /aria-label="1 live linked session"/);
	assert.match(markup, />1 live linked session</);
});
