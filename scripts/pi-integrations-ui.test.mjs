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
      name: "expose-pi-ui-helpers",
      transform(code, id) {
        if (id.endsWith("/IntegrationsTab.tsx")) {
          return `${code}\nexport { integrationToggleState, providerActionCopy };`;
        }
        if (id.endsWith("/UsageView.tsx")) {
          return `${code}\nexport { buildRows, ProviderCounts };`;
        }
        if (id.endsWith("/LimitsSection.tsx")) {
          return `${code}\nexport { directRows };`;
        }
      },
    },
  ],
});

const [integrationsModule, usageModule, limitsModule, contextModule] =
  await Promise.all([
    server.ssrLoadModule("/src/components/settings/IntegrationsTab.tsx"),
    server.ssrLoadModule("/src/components/widget/views/UsageView.tsx"),
    server.ssrLoadModule("/src/components/widget/LimitsSection.tsx"),
    server.ssrLoadModule("/src/components/settings/ContextTab.tsx"),
  ]);

test.after(() => server.close());

const baseStatus = {
  provider: "pi",
  detectedCli: true,
  detectedHome: true,
  enabled: false,
  setupState: "not_installed",
  userHasMadeChoice: false,
  lastError: null,
  lastVerifiedAt: null,
};

// @lat: [[pi-integrations-ui-tests#Pi Integrations UI Tests#Pi card states]]
test("Pi card exposes detected, enabled, and error states", () => {
  assert.deepEqual(
    integrationsModule.integrationToggleState(baseStatus, false),
    { tone: "off", label: "OFF", disabled: false },
  );
  assert.deepEqual(
    integrationsModule.integrationToggleState(
      { ...baseStatus, enabled: true, setupState: "installed" },
      false,
    ),
    { tone: "on", label: "ON", disabled: false },
  );
  assert.deepEqual(
    integrationsModule.integrationToggleState(
      { ...baseStatus, setupState: "error", lastError: "Extension is stale" },
      false,
    ),
    { tone: "error", label: "ERROR", disabled: false },
  );

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
  for (const [status, expectedClass] of [
    [baseStatus, "settings-toggle--off"],
    [{ ...baseStatus, enabled: true, setupState: "installed" }, "settings-toggle--on"],
    [
      { ...baseStatus, setupState: "error", lastError: "Extension is stale" },
      "settings-toggle--error",
    ],
  ]) {
    const markup = renderToStaticMarkup(
      createElement(integrationsModule.default, {
        integrations: {
          statuses: [status],
          loading: false,
          error: null,
          inFlightProviders: new Set(),
          indicatorPrimaryProvider: null,
          rescanInFlight: false,
        },
        features,
      }),
    );
    assert.match(markup, />Pi</);
    assert.match(markup, new RegExp(expectedClass));
  }
});

// @lat: [[pi-integrations-ui-tests#Pi Integrations UI Tests#Executable extension consent]]
test("Pi consent discloses installed files, credentials, repair, and reload", () => {
  const copy = integrationsModule.providerActionCopy({
    provider: "pi",
    nextEnabled: true,
  }).description;

  assert.equal(
    copy,
    "Quill will install quill.ts in Pi's extensions directory and write the local server URL, context URL, hostname, and authentication secret to ~/.config/quill/config.json. Quill repairs both files automatically. A running Pi process loads updates after /reload.",
  );
});

// @lat: [[pi-integrations-ui-tests#Pi Integrations UI Tests#Provider breakdown counts]]
test("skill and hook rows render Claude, Codex, and Pi counts", () => {
  const now = Date.parse("2026-08-14T12:00:00Z");
  const common = {
    total_count: 12,
    claude_count: 5,
    codex_count: 4,
    pi_count: 3,
  };
  const skill = usageModule.buildRows(
    "skills",
    [
      {
        ...common,
        skill_name: "audit",
        project_count: 2,
        last_used: "2026-08-14T11:00:00Z",
      },
    ],
    now,
  )[0];
  const hook = usageModule.buildRows(
    "hooks",
    [
      {
        ...common,
        hook_identity: "quill:observe",
        hook_event: "PreToolUse",
        tool_name: "Bash",
        is_quill: true,
        last_fired_at: "2026-08-14T11:00:00Z",
      },
    ],
    now,
  )[0];

  assert.deepEqual(skill.providerCounts, [
    { provider: "claude", count: 5 },
    { provider: "codex", count: 4 },
    { provider: "pi", count: 3 },
  ]);
  assert.deepEqual(hook.providerCounts, skill.providerCounts);

  const markup = renderToStaticMarkup(
    createElement(usageModule.ProviderCounts, {
      counts: skill.providerCounts,
    }),
  );
  assert.match(markup, /aria-label="Claude 5"/);
  assert.match(markup, /aria-label="Codex 4"/);
  assert.match(markup, /aria-label="Pi 3"/);
});

// @lat: [[pi-integrations-ui-tests#Pi Integrations UI Tests#Excluded settings copy]]
test("brevity settings explicitly exclude Pi", () => {
  const integrations = {
    contextPreservation: { enabled: false, hasContextSavingsEvents: false },
    contextPreservationInFlight: false,
    setContextPreservationEnabled: async () => {},
    hasEnabledProvider: true,
    loading: false,
  };
  const features = {
    features: {
      contextPreservation: false,
      activityTracking: true,
      contextTelemetry: true,
      brevity: false,
    },
    loading: false,
    saving: false,
    setContextTelemetry: async () => {},
    setBrevity: async () => {},
  };
  const markup = renderToStaticMarkup(
    createElement(contextModule.default, { integrations, features }),
  );

  assert.match(markup, /Pi and MiniMax are excluded/);
});

// @lat: [[pi-integrations-ui-tests#Pi Integrations UI Tests#Limits omission]]
test("Limits omits Pi without an unavailable row or N/A copy", () => {
  assert.deepEqual(limitsModule.directRows([baseStatus], null, Date.now()), []);
});
