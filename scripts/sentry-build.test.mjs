import assert from "node:assert/strict";
import { mkdtempSync, mkdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { loadConfigFromFile } from "vite";
import { verifySentryOutput } from "./verify-sentry-output.mjs";

async function loadViteConfig(authToken) {
	const previousNodeEnv = process.env.NODE_ENV;
	const previousToken = process.env.SENTRY_AUTH_TOKEN;
	process.env.NODE_ENV = "production";
	if (authToken) process.env.SENTRY_AUTH_TOKEN = authToken;
	else delete process.env.SENTRY_AUTH_TOKEN;

	try {
		return (await loadConfigFromFile(
			{ command: "build", mode: "production" },
			"vite.config.ts",
			process.cwd(),
		)).config;
	} finally {
		if (previousNodeEnv === undefined) delete process.env.NODE_ENV;
		else process.env.NODE_ENV = previousNodeEnv;
		if (previousToken === undefined) delete process.env.SENTRY_AUTH_TOKEN;
		else process.env.SENTRY_AUTH_TOKEN = previousToken;
	}
}

// @lat: [[crash-reporting-tests#Crash Reporting Test Specs#Release matrix symbolication contract]]
test("release matrix uploads exact bundles and never packages source maps", async () => {
	const unauthenticated = await loadViteConfig();
	assert.equal(unauthenticated.build.sourcemap, false);
	assert.ok(!unauthenticated.plugins.flat().some(({ name }) => name === "sentry-vite-plugin"));

	const authenticated = await loadViteConfig("test-token");
	assert.equal(authenticated.build.sourcemap, true);
	assert.equal(authenticated.plugins.flat().at(-1).name, "sentry-vite-plugin");

	const fixture = mkdtempSync(join(tmpdir(), "quill-sentry-build-"));
	try {
		const assets = join(fixture, "assets");
		mkdirSync(assets);
		writeFileSync(join(assets, "app.js"), "globalThis._sentryDebugIds = {};\n");
		assert.throws(() => verifySentryOutput(fixture, true), /debug ID missing/);
		writeFileSync(
			join(assets, "app.js"),
			'globalThis._sentryDebugIdIdentifier = "sentry-dbid-not-a-uuid";\n',
		);
		assert.throws(() => verifySentryOutput(fixture, true), /debug ID missing/);
		writeFileSync(
			join(assets, "app.js"),
			'globalThis._sentryDebugIdIdentifier = "sentry-dbid-123e4567-e89b-42d3-a456-426614174000";\n',
		);
		verifySentryOutput(fixture, true);

		writeFileSync(join(assets, "app.js.map"), "{}");
		assert.throws(() => verifySentryOutput(fixture, true), /Source map remained/);
		rmSync(join(assets, "app.js.map"));
		writeFileSync(join(assets, "app.js"), "console.log('missing');\n");
		assert.throws(() => verifySentryOutput(fixture, true), /debug ID missing/);
		verifySentryOutput(fixture, false);
	} finally {
		rmSync(fixture, { recursive: true, force: true });
	}
});
