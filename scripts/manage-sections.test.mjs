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
});

const { default: ManageWindowView } = await server.ssrLoadModule(
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
