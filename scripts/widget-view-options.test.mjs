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
			name: "expose-open-widget-view-list",
			transform(code, id) {
				if (id.endsWith("/ViewRegion.tsx")) {
					return `${code}\nexport { VIEWS };`;
				}
				if (id.endsWith("/ViewSwitcher.tsx")) {
					return code.replace(
						"const [open, setOpen] = useState(false);",
						"const [open, setOpen] = useState(true);",
					);
				}
			},
		},
	],
});

const [{ VIEWS }, { default: ViewSwitcher }] = await Promise.all([
	server.ssrLoadModule("/src/components/widget/ViewRegion.tsx"),
	server.ssrLoadModule("/src/components/widget/ViewSwitcher.tsx"),
]);

test.after(() => server.close());

// @lat: [[widget-view-tests#Widget View Tests#Available Widget Views]]
test("view switcher offers only Usage, Models, and Context", () => {
	const markup = renderToStaticMarkup(
		createElement(ViewSwitcher, {
			options: VIEWS,
			view: "usage",
			onSelect() {},
		}),
	);
	const labels = [...markup.matchAll(/role="option"[^>]*>([^<]+)/g)].map(
		([, label]) => label,
	);

	assert.deepEqual(labels, ["Usage", "Models", "Context"]);
	assert.equal(markup.match(/aria-selected="true"/g)?.length, 1);
});
