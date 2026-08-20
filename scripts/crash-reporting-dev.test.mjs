import assert from "node:assert/strict";
import test from "node:test";
import { createServer } from "vite";

// The module under test imports the real SDK and the Tauri event bridge;
// neither exists here, so both are replaced by recording stubs.
const STUBS = {
	"@sentry/react": `
		export const calls = [];
		export function init(options) { calls.push(options); }
		export function close() { return Promise.resolve(true); }
		export const globalHandlersIntegration = () => ({});
		export const functionToStringIntegration = () => ({});
		export const inboundFiltersIntegration = () => ({});
		export const dedupeIntegration = () => ({});
	`,
	"@tauri-apps/api/event": "export function listen() { return Promise.resolve(() => {}); }",
};

// A dev server reports `import.meta.env.DEV` from `isProduction`, which Vite
// takes from NODE_ENV rather than from the mode, so the production half of
// this test has to set it the way `vite build` does.
async function loadCrashReporting(mode) {
	const previousNodeEnv = process.env.NODE_ENV;
	process.env.NODE_ENV = mode;
	const server = await createServer({
		mode,
		appType: "custom",
		server: { middlewareMode: true, hmr: false },
		// Externalized bare imports bypass plugin resolution, so the stubs
		// below only take effect once the SSR graph inlines them.
		ssr: { noExternal: true },
		plugins: [{
			name: "crash-reporting-stubs",
			enforce: "pre",
			resolveId: (id) => (id in STUBS ? `\0${id}` : null),
			load: (id) => (id.startsWith("\0") ? STUBS[id.slice(1)] : null),
		}],
	});
	try {
		return {
			module: await server.ssrLoadModule("/src/lib/crashReporting.ts"),
			sentry: await server.ssrLoadModule("@sentry/react"),
		};
	} finally {
		await server.close();
		if (previousNodeEnv === undefined) delete process.env.NODE_ENV;
		else process.env.NODE_ENV = previousNodeEnv;
	}
}

// @lat: [[crash-reporting-tests#Crash Reporting Test Specs#Development builds never transmit]]
test("the dev server opts every surface out of the crash transport", async () => {
	const dev = await loadCrashReporting("development");
	dev.module.setCrashReportingEnabled(true);
	assert.deepEqual(dev.sentry.calls, []);

	const production = await loadCrashReporting("production");
	production.module.setCrashReportingEnabled(true);
	assert.equal(production.sentry.calls.length, 1);
	assert.equal(production.sentry.calls[0].environment, "production");
});
