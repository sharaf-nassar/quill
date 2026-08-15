import assert from "node:assert/strict";
import test from "node:test";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { createServer } from "vite";

const server = await createServer({
  appType: "custom",
  server: { middlewareMode: true, hmr: false },
  optimizeDeps: { noDiscovery: true },
  plugins: [
    {
      name: "expose-cpa-row",
      transform(code, id) {
        if (id.endsWith("/LimitsSection.tsx")) {
          return `${code}\nexport { CpaRow };`;
        }
      },
    },
  ],
});

const { CpaRow } = await server.ssrLoadModule(
  "/src/components/widget/LimitsSection.tsx",
);

test.after(() => server.close());

function renderRow(states, expanded = false) {
  return renderToStaticMarkup(
    createElement(CpaRow, {
      row: {
        provider: "claude",
        state: "ready",
        cells: [],
        resetText: null,
        resetSeverity: "stale",
        detail: null,
        healthy: states.filter((state) => state === "ready").length,
        total: states.length,
        accounts: states.map((state, index) => ({
          id: String(index),
          label: `account-${index}`,
          statusMessage: null,
          state,
          cells: [],
        })),
      },
      expanded,
      controlsId: "claude-accounts",
      onToggle() {},
    }),
  );
}

// @lat: [[widget-limits-tests#Widget Limits Tests#Collapsed all-cooling pool]]
test("collapsed CPA pool labels only an entirely cooling account set", () => {
  const collapsed = renderRow(["cooling", "cooling"]);
  assert.match(
    collapsed,
    /wg-cpa-identity[\s\S]*wg-cpa-pool-state[\s\S]*COOLING/,
  );
  assert.doesNotMatch(
    renderRow(["cooling", "cooling"], true),
    /wg-cpa-pool-state/,
  );

  for (const states of [
    [],
    ["ready", "cooling"],
    ["disabled", "cooling"],
    ["unavailable", "cooling"],
  ]) {
    assert.doesNotMatch(renderRow(states), /wg-cpa-pool-state/);
  }
});
