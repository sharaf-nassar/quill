import assert from "node:assert/strict";
import test from "node:test";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { createServer } from "vite";

globalThis.window = { location: { search: "?view=manage" } };
globalThis.localStorage = {
	getItem() {
		return null;
	},
};

const server = await createServer({
	appType: "custom",
	server: { middlewareMode: true, hmr: false },
	optimizeDeps: { noDiscovery: true },
	plugins: [
		{
			name: "expose-parse-target",
			transform(code, id) {
				if (id.endsWith("/ManageWindowView.tsx")) {
					return `${code}\nexport { parseTarget };`;
				}
			},
		},
	],
});

const { default: ManageWindowView, parseTarget } = await server.ssrLoadModule(
	"/src/windows/ManageWindowView.tsx",
);

test.after(() => server.close());

// @lat: [[manage-section-tests#Manage Section Tests#Available Manage Sections]]
test("Manage offers Sessions, Learning, and Settings", () => {
	const markup = renderToStaticMarkup(createElement(ManageWindowView));
	const labels = [...markup.matchAll(/manage-rail-label[^>]*>([^<]+)/g)].map(
		([, label]) => label,
	);

	assert.deepEqual(labels, ["Sessions", "Learning", "Settings", "Live"]);
});

// @lat: [[manage-section-tests#Manage Section Tests#Section Deep Links]]
test("deep links carry an optional settings tab", () => {
	assert.deepEqual(parseTarget("settings:integrations"), {
		section: "settings",
		tab: "integrations",
	});
	assert.deepEqual(parseTarget("settings"), { section: "settings", tab: null });
	assert.deepEqual(parseTarget("bogus:integrations"), {
		section: null,
		tab: null,
	});
	assert.deepEqual(parseTarget(null), { section: null, tab: null });
});
