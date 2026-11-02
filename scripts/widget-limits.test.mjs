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
          return `${code}\nexport { CpaRow, cpaRows, syncStateForSource };`;
        }
      },
    },
  ],
});

const { CpaRow, cpaRows, syncStateForSource } = await server.ssrLoadModule(
  "/src/components/widget/LimitsSection.tsx",
);

test.after(() => server.close());

// @lat: [[cpa-tests#CPA Regression Tests#Truthful widget quota state]]
test("CPA failure and scoped identities remain distinct and visible", () => {
  for (const kind of ["auth", "server", "paused", "stale", "network"]) {
    assert.notEqual(syncStateForSource([{ source: "cpa", kind }], "cpa"), "live");
  }
  const bucket = (key, label, utilization) => ({
    key: `cpa/a/${key}`, label, utilization, provider: "codex", source: "cpa",
    account_id: "a", account_label: "Account", resets_at: null, sort_order: 0,
  });
  const buckets = [bucket("codex_300m", "5 hours", 25), bucket("codex_scope_61_300m", "Spark · 5 hours", 100)];
  const rows = cpaRows({
    buckets, provider_errors: [{ provider: "codex", source: "cpa", kind: "stale", message: "Showing cached quotas." }],
    cpa_accounts: [{ provider: "codex", auth_index: "a", label: "Account", status: "ready", disabled: false, unavailable: false,
      quota: { state: "cached", observed_at: "2030-01-01T00:00:00Z", retry_at: "2030-01-01T00:05:00Z", message: "Rate limited.",
        credits: { balance: "12.5", reset_available: 0 }, models: [{ model: "gpt-example", available: false, available_at: null }] } }],
    cpa_pools: [{ provider: "codex", healthy: 1, total: 1, buckets }],
  }, Date.now());
  assert.equal(rows[0].cells.length, 2);
  assert.equal(rows[0].accounts[0].cells.length, 2);
  assert.deepEqual(
    [rows[0].cells, rows[0].accounts[0].cells].map((cells) => cells.map((cell) => cell.severity)),
    [["nominal", "critical"], ["nominal", "critical"]],
  );
  assert.ok(rows[0].cells.some((cell) => cell.shortLabel === "Spark · 5 hours"));
  const markup = renderToStaticMarkup(createElement(CpaRow, { row: rows[0], expanded: true, controlsId: "quota-test", onToggle() {} }));
  for (const text of ["Observed", "Quota cached", "Retry after", "Credits: 12.5", "Reset credits: 0", "gpt-example cooling", "Showing cached quotas."]) assert.ok(!markup.includes(text), text);
  assert.doesNotMatch(markup, /<p\b/);
  assert.match(markup, /25%/);
  assert.match(markup, /100%/);
});

// @lat: [[widget-limits-tests#Widget Limits Tests#Claude 5-hour cell follows the data]]
test("Claude CPA rows show a 5-hour cell only when an account returns one", () => {
  const bucket = (account, key, label, utilization) => ({
    key: `cpa/${account}/${key}`, label, utilization, provider: "claude", source: "cpa",
    account_id: account, account_label: account, resets_at: null, sort_order: 0,
  });
  const account = (id) => ({
    provider: "claude", auth_index: id, label: id, status: "ready", disabled: false, unavailable: false, quota: null,
  });
  const rows = (buckets) => cpaRows({
    buckets, provider_errors: [], cpa_accounts: [account("ultra"), account("max")],
    cpa_pools: [{ provider: "claude", healthy: 2, total: 2, buckets: [] }],
  }, Date.now());
  const keys = (cells) => cells.map((cell) => cell.key);

  // Verified Ultra only: weekly windows, no seeded 5-hour dash.
  const ultraOnly = rows([bucket("ultra", "seven_day", "7 days", 30)]);
  assert.deepEqual(keys(ultraOnly[0].cells), ["seven_day"]);
  assert.deepEqual(keys(ultraOnly[0].accounts[0].cells), ["seven_day"]);

  // A Max sibling with a 5-hour window restores the shared cell for both rows.
  const mixed = rows([
    bucket("ultra", "seven_day", "7 days", 30),
    bucket("max", "five_hour", "5 hours", 62),
    bucket("max", "seven_day", "7 days", 10),
  ]);
  assert.deepEqual(keys(mixed[0].cells), ["five_hour", "seven_day"]);
  assert.equal(mixed[0].accounts.find((a) => a.id === "max").cells[0].percent, 62);
  assert.equal(mixed[0].accounts.find((a) => a.id === "ultra").cells[0].percent, null);
});

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
