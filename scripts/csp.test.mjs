import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { loadConfigFromFile } from "vite";

const DESKTOP_CSP =
	"default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; connect-src ipc: http://ipc.localhost https://ipc.localhost https://o1373069.ingest.us.sentry.io;";
const WEB_CSP =
	"default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; font-src 'self' data:; connect-src 'self';";

function readCsp(document) {
	const html = readFileSync(document, "utf8");
	return html.match(/Content-Security-Policy" content="([^"]+)"/)[1];
}

async function loadBuildConfig(mode) {
	return (await loadConfigFromFile({ command: "build", mode }, "vite.config.ts", process.cwd()))
		.config;
}

// @lat: [[infrastructure#Infrastructure#Build Configuration#Frontend Build#Crash Transport CSP]]
test("desktop production CSP stays pinned to Tauri IPC and Sentry", () => {
	assert.equal(readCsp("index.html"), DESKTOP_CSP);
});

// @lat: [[infrastructure#Infrastructure#Build Configuration#Frontend Build#Crash Transport CSP]]
test("web production CSP stays pinned to its same-origin bridge", () => {
	assert.equal(readCsp("web.html"), WEB_CSP);
	assert.doesNotMatch(WEB_CSP, /sentry/i);
});

// @lat: [[infrastructure#Infrastructure#Build Configuration#Frontend Build#Crash Transport CSP]]
test("Vite builds the isolated web document", async () => {
	const desktop = await loadBuildConfig("production");
	const web = await loadBuildConfig("web");

	assert.equal(desktop.build.rollupOptions?.input, undefined);
	assert.equal(web.build.outDir, "dist-web");
	assert.equal(web.build.rollupOptions.input, "web.html");
});
