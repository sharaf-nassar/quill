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
  piExtensionHealth: null,
};

// @lat: [[pi-integrations-ui-tests#Pi Integrations UI Tests#Extension health presentation]]
test("Pi health renders current status and typed error detail without red", () => {
  const features = {
    features: { contextPreservation: false, activityTracking: true, contextTelemetry: true, brevity: false },
    loading: false,
    saving: false,
  };
  for (const [state, label] of [
    ["never_connected", "Never connected"],
    ["alive", "Alive"],
    ["idle", "Idle"],
    ["stale", "Stale"],
  ]) {
    const markup = renderToStaticMarkup(createElement(integrationsModule.default, {
      integrations: {
        statuses: [{
          ...baseStatus,
          enabled: true,
          setupState: "installed",
          piExtensionHealth: {
            state,
            lastSeen: "2026-08-14T08:00:00Z",
            protocol: "2",
            extensionVersion: "0.1.0",
            minQuillVersion: "0.9.0",
            lastError: state === "stale" ? "protocol_mismatch" : null,
          },
        }],
        loading: false,
        error: null,
        providerActionErrors: {},
        inFlightProviders: new Set(),
        indicatorPrimaryProvider: null,
        rescanInFlight: false,
      },
      features,
    }));
    assert.match(markup, new RegExp(`Extension: ${label}`));
    assert.doesNotMatch(markup, /meter-red|settings-toggle--error/);
    if (state === "stale") {
      assert.match(markup, /Exact reporter mismatch/);
      assert.match(markup, /protocol 2/);
    }
  }
});

// @lat: [[pi-integrations-ui-tests#Pi Integrations UI Tests#Typed extension error detail]]
test("Pi health keeps remediation, affected counts, recovery, and no-session absence compact", () => {
  const features = {
    features: { contextPreservation: false, activityTracking: true, contextTelemetry: true, brevity: false },
    loading: false,
    saving: false,
  };
  for (const [kind, copy] of [
    ["protocol_mismatch", /Exact reporter mismatch/],
    ["unknown_session", /Unknown session/],
    ["child_reporter_missing", /Configured child reporter missing/],
    ["source_recovering", /Recovering persisted source/],
    ["reconciliation_failed", /Reconciliation failed/],
    ["telemetry_rejected", /Telemetry rejected/],
    ["saturated", /Reporter health saturated/],
  ]) {
    const markup = renderToStaticMarkup(createElement(integrationsModule.default, {
      integrations: {
        statuses: [{
          ...baseStatus,
          enabled: true,
          setupState: "installed",
          piExtensionHealth: {
            state: "alive",
            lastSeen: "2026-08-14T08:00:00Z",
            protocol: "1",
            extensionVersion: "0.1.0",
            minQuillVersion: "0.9.0",
            lastError: kind,
            affectedReporters: 2,
            affectedSessions: 3,
            remediation: "Reload Pi after repair.",
            lastRecoveredAt: "2026-08-14T07:55:00Z",
            requiredProtocol: "2",
            requiredExtensionVersion: "0.2.0",
            requiredQuillVersion: "1.0.0",
          },
        }],
        loading: false,
        error: null,
        providerActionErrors: {},
        inFlightProviders: new Set(),
        indicatorPrimaryProvider: null,
        rescanInFlight: false,
      },
      features,
    }));
    assert.match(markup, copy);
    assert.match(markup, /2 reporters affected/);
    assert.match(markup, /3 sessions affected/);
    assert.match(markup, /Reload Pi after repair/);
    assert.match(markup, /Recovery verified/);
    assert.match(markup, /No-session runs are intentionally absent/);
    assert.match(markup, /role="status"/);
    assert.doesNotMatch(markup, /meter-red|settings-toggle--error/);
  }
});

// @lat: [[pi-integrations-ui-tests#Pi Integrations UI Tests#Missing health fallback]]
test("enabled Pi without health data renders a slate unavailable detail", () => {
  const markup = renderToStaticMarkup(createElement(integrationsModule.default, {
    integrations: {
      statuses: [{ ...baseStatus, enabled: true, setupState: "installed" }],
      loading: false,
      error: null,
      providerActionErrors: {},
      inFlightProviders: new Set(),
      indicatorPrimaryProvider: null,
      rescanInFlight: false,
    },
    features: {
      features: { contextPreservation: false, activityTracking: true, contextTelemetry: true, brevity: false },
      loading: false,
      saving: false,
    },
  }));
  assert.match(markup, /Extension: Status unavailable/);
  assert.doesNotMatch(markup, /meter-red|settings-toggle--error/);
});

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
  for (const [status, expectedClass, expectedCopy] of [
    [baseStatus, "settings-toggle--off", /Quill integration disabled or not configured\./],
    [
      { ...baseStatus, setupState: "installed" },
      "settings-toggle--off",
      /Quill integration disabled or not configured\./,
    ],
    [
      { ...baseStatus, setupState: "missing", detectedHome: false },
      "settings-toggle--setup",
      /Auto-deployment pending; click to run\./,
    ],
    [
      { ...baseStatus, enabled: true, setupState: "installed" },
      "settings-toggle--on",
      /Quill assets installed and active\./,
    ],
    [
      { ...baseStatus, setupState: "error", lastError: "Extension is stale" },
      "settings-toggle--error",
      /Extension is stale/,
    ],
  ]) {
    const markup = renderToStaticMarkup(
      createElement(integrationsModule.default, {
        integrations: {
          statuses: [status],
          loading: false,
          error: null,
          providerActionErrors: {},
          inFlightProviders: new Set(),
          indicatorPrimaryProvider: null,
          rescanInFlight: false,
        },
        features,
      }),
    );
    assert.match(markup, />Pi</);
    assert.match(markup, new RegExp(expectedClass));
    assert.match(markup, expectedCopy);
    assert.doesNotMatch(markup, /Quill assets not installed/);
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

// @lat: [[pi-integrations-ui-tests#Pi Integrations UI Tests#Extension health presentation]]
test("Sessions names recovering and configured-child gaps without a new row", () => {
  const common = {
    provider: "pi",
    session_id: "pi-session",
    parent_session_id: null,
    hostname: "mbp",
    total_tokens: 0,
    turn_count: 0,
    first_seen: "2026-08-14T08:00:00Z",
    last_active: "2026-08-14T08:00:00Z",
    ended_at: null,
    project: "quill",
    active_runtime_secs: null,
    agent_count: null,
    agent_runtime_secs: null,
    current_turn_runtime_secs: null,
    current_turn_runtime_active: false,
    runtime_as_of_ms: null,
    active_runtime_rate: 0,
    observed_agents: null,
    live_linked_sessions: null,
    observed_only: true,
  };
  const [recovering, childGap] = usageModule.buildRows(
    "sessions",
    [
      { ...common, pi_lineage: { kind: "unresolved", reason: "recovering" } },
      {
        ...common,
        session_id: "pi-child",
        pi_lineage: { kind: "unresolved", reason: "subagent_parent_unavailable" },
      },
    ],
    Date.parse("2026-08-14T08:00:30Z"),
  );
  assert.deepEqual(recovering.lineageStatus, {
    label: "recovering",
    detail: "Persisted Pi source is recovering; live state is not yet verified",
  });
  assert.deepEqual(childGap.lineageStatus, {
    label: "child gap",
    detail: "Configured Pi child reporter could not verify its parent session",
  });
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
