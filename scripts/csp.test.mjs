import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

// @lat: [[infrastructure#Infrastructure#Build Configuration#Frontend Build#Crash Transport CSP]]
test("production CSP permits only Tauri IPC and the configured Sentry origin", () => {
	const html = readFileSync("index.html", "utf8");
	const crashReporting = readFileSync("src/lib/crashReporting.ts", "utf8");
	const csp = html.match(/Content-Security-Policy" content="([^"]+)"/)[1];
	const dsn = crashReporting.match(/const DSN\s*=\s*\n?\s*"([^"]+)"/)[1];
	const connectSrc = csp
		.split(";")
		.map((directive) => directive.trim().split(/\s+/))
		.find(([name]) => name === "connect-src");

	assert.deepEqual(connectSrc, [
		"connect-src",
		"ipc:",
		"http://ipc.localhost",
		"https://ipc.localhost",
		new URL(dsn).origin,
	]);
});
