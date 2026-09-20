import assert from "node:assert/strict";
import test from "node:test";
import * as React from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { createServer } from "vite";

const server = await createServer({
  appType: "custom",
  server: { middlewareMode: true, hmr: false },
  optimizeDeps: { noDiscovery: true },
  plugins: [{
    name: "observe-session-context",
    enforce: "pre",
    transform(code, id) {
      if (id.endsWith("/SessionsWindowView.tsx")) {
        return code.replace('import { invoke } from "@tauri-apps/api/core";', "const invoke = (...args) => globalThis.searchContextInvoke(...args);")
          .replace('\n\treturn (\n\t\t<div className="sessions-window">', '\n\tglobalThis.searchContextSnapshot = { handleSelect, selectedHit, context, contextErrors, hitKey };\n\treturn (\n\t\t<div className="sessions-window">');
      }
      // DOMPurify needs a browser DOM; this check exercises context state/copy,
      // not snippet sanitization (which remains unchanged in production).
      if (id.endsWith("/DetailPanel.tsx")) {
        return code.replace('import DOMPurify from "dompurify";', "const DOMPurify = { sanitize: () => '' };");
      }
      return null;
    },
  }],
});
const { default: SessionsWindowView } = await server.ssrLoadModule("/src/windows/SessionsWindowView.tsx");
const { default: DetailPanel } = await server.ssrLoadModule("/src/components/sessions/DetailPanel.tsx");
test.after(() => server.close());

// @lat: [[pipeline-search-tests#Pipeline Search Tests#Context Failure UI]]
test("context failures exit loading, stay source-local, and clear on retry", async (t) => {
  const internals = React.__CLIENT_INTERNALS_DO_NOT_USE_OR_WARN_USERS_THEY_CANNOT_UPGRADE;
  const values = [];
  let slot = 0;
  const dispatcher = {
    useState(initial) {
      const index = slot++;
      if (!(index in values)) values[index] = typeof initial === "function" ? initial() : initial;
      return [values[index], (next) => { values[index] = typeof next === "function" ? next(values[index]) : next; }];
    },
    useRef(initial) {
      const index = slot++;
      return values[index] ??= { current: initial };
    },
    useMemo: (compute) => compute(),
    useCallback: (callback) => callback,
    useEffect() {},
  };
  const pending = new Map();
  globalThis.searchContextInvoke = (command, args) => {
    assert.equal(command, "get_session_context");
    return new Promise((resolve, reject) => pending.set(args.sourceKey, { resolve, reject }));
  };
  t.after(() => {
    delete globalThis.searchContextInvoke;
    delete globalThis.searchContextSnapshot;
  });
  function render() {
    const previous = internals.H;
    slot = 0;
    internals.H = dispatcher;
    try { SessionsWindowView(); } finally { internals.H = previous; }
    return globalThis.searchContextSnapshot;
  }
  function detail(snapshot) {
    const key = snapshot.hitKey(snapshot.selectedHit);
    return renderToStaticMarkup(React.createElement(DetailPanel, {
      hit: snapshot.selectedHit,
      context: snapshot.context[key] ?? null,
      contextError: snapshot.contextErrors[key] ?? null,
      locStats: null,
      retentionCutoff: null,
      onNavigateSession() {},
    }));
  }
  const base = { provider: "claude", session_id: "shared", message_id: "same", host: "host",
    role: "user", snippet: "", project: "fixture", timestamp: "2026-08-14T00:00:00Z", git_branch: "" };
  const a = { ...base, source_key: "source:a" };
  const b = { ...base, source_key: "source:b" };
  const first = render().handleSelect(a);
  const second = render().handleSelect(b);
  pending.get(a.source_key).reject("old source unavailable");
  await first;
  assert.match(detail(render()), /Loading context/);
  assert.doesNotMatch(detail(render()), /old source unavailable/);
  pending.get(b.source_key).reject("requested source unavailable");
  await second;
  assert.match(detail(render()), /role="alert">Context unavailable: requested source unavailable/);
  assert.doesNotMatch(detail(render()), /Loading context/);
  const retry = render().handleSelect(b);
  assert.match(detail(render()), /Loading context/);
  assert.doesNotMatch(detail(render()), /role="alert"/);
  pending.get(b.source_key).resolve({ messages: [], truncated: true });
  await retry;
  assert.match(detail(render()), /role="status">Context truncated/);
  assert.doesNotMatch(detail(render()), /Loading context|role="alert"/);
});
