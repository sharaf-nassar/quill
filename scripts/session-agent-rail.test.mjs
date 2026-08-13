import assert from "node:assert/strict";
import test from "node:test";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { createServer } from "vite";

const server = await createServer({
	appType: "custom",
	server: { middlewareMode: true, hmr: false },
	plugins: [
		{
			name: "expose-active-agent-rail",
			transform(code, id) {
				if (id.endsWith("/UsageView.tsx")) {
					return `${code}\nexport { ActiveAgentRail };`;
				}
			},
		},
	],
});

const { ActiveAgentRail } = await server.ssrLoadModule(
	"/src/components/widget/views/UsageView.tsx",
);

test.after(() => server.close());

// @lat: [[live-subagent-count-tests#Live Subagent Count Tests#Active Agent Rail Indicator]]
test("active agent rail starts with one decorative live indicator", () => {
	const agents = [
		{
			agentId: "agent-1",
			model: "Sol",
			runtime: "17m",
			ariaLabel: "gpt-5.6-sol, agent agent-1, 17m active runtime",
		},
		{
			agentId: "agent-2",
			model: "Terra",
			runtime: "4m",
			ariaLabel: "gpt-5.6-terra, agent agent-2, 4m active runtime",
		},
	];
	const markup = renderToStaticMarkup(createElement(ActiveAgentRail, { agents }));

	assert.match(
		markup,
		/wg-row-agent-rail[^>]*>[\s\S]*wg-row-agent-live-icon[^>]*aria-hidden="true"[\s\S]*role="listitem"/,
	);
	assert.equal(markup.match(/wg-row-agent-live-icon/g)?.length, 1);
	assert.equal(
		renderToStaticMarkup(createElement(ActiveAgentRail, { agents: [] })),
		"",
	);
});
