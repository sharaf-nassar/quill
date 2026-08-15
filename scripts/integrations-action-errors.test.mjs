import assert from "node:assert/strict";
import test from "node:test";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { createServer } from "vite";

const server = await createServer({
  appType: "custom",
  server: { middlewareMode: true, hmr: false },
  optimizeDeps: { noDiscovery: true },
});

const [integrationsModule, hookModule] = await Promise.all([
  server.ssrLoadModule("/src/components/settings/IntegrationsTab.tsx"),
  server.ssrLoadModule("/src/hooks/useIntegrations.ts"),
]);

test.after(() => server.close());

const timeout = "Codex app-server hooks/list timed out after 10s";
const statuses = [
  ["claude", true],
  ["codex", false],
  ["pi", true],
  ["mini_max", true],
].map(([provider, enabled]) => ({
  provider,
  detectedCli: true,
  detectedHome: true,
  enabled,
  setupState: enabled ? "installed" : "not_installed",
  userHasMadeChoice: true,
  lastError: null,
  lastVerifiedAt: null,
}));

const features = {
  features: {
    contextPreservation: false,
    activityTracking: true,
    contextTelemetry: true,
    brevity: false,
  },
  loading: false,
  saving: false,
};

// @lat: [[integration-action-error-tests#Integration Action Error Tests#Provider-local enable timeout]]
test("a Codex enable timeout keeps every provider control visible", () => {
  const markup = renderToStaticMarkup(
    createElement(integrationsModule.default, {
      integrations: {
        statuses,
        loading: false,
        error: null,
        providerActionErrors: { codex: timeout },
        inFlightProviders: new Set(),
        indicatorPrimaryProvider: null,
        rescanInFlight: false,
      },
      features,
    }),
  );

  for (const [provider, label] of [
    ["claude", "Claude"],
    ["codex", "Codex"],
    ["pi", "Pi"],
    ["mini_max", "MiniMax"],
  ]) {
    assert.match(markup, new RegExp(`data-provider="${provider}">${label}</span>`));
  }
  assert.equal(markup.split(timeout).length - 1, 1);
  assert.match(markup, /role="alert">Codex app-server hooks\/list timed out after 10s/);
  assert.match(markup, /aria-label="Retry enabling Codex"/);
  assert.doesNotMatch(markup, /aria-label="Retry enabling (Claude|Pi|MiniMax)"/);
});

// @lat: [[integration-action-error-tests#Integration Action Error Tests#Successful retry isolation]]
test("a successful Codex retry preserves another provider action error", () => {
  assert.deepEqual(
    hookModule.updateProviderActionError(
      { claude: "Claude install failed", codex: timeout },
      "codex",
      null,
    ),
    { claude: "Claude install failed" },
  );
});

// @lat: [[integration-action-error-tests#Integration Action Error Tests#Initial request states]]
test("initial loading and load failure still replace the provider rows", () => {
  const render = (loading, error) =>
    renderToStaticMarkup(
      createElement(integrationsModule.default, {
        integrations: {
          statuses: [],
          loading,
          error,
          providerActionErrors: {},
          inFlightProviders: new Set(),
          indicatorPrimaryProvider: null,
          rescanInFlight: false,
        },
        features,
      }),
    );

  assert.match(render(true, null), /settings-empty">checking…/);
  assert.match(
    render(false, "Could not load provider statuses"),
    /settings-empty settings-empty--error">Could not load provider statuses/,
  );
});
