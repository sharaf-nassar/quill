import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { devArgs } from "./tauri.mjs";

const read = (path) => JSON.parse(readFileSync(path, "utf8"));

// @lat: [[infrastructure#Infrastructure#Build Configuration#Tauri Configuration#Development Identity]]
test("the dev config claims a non-production Tauri identity", () => {
	const base = read("src-tauri/tauri.conf.json");
	const dev = read("src-tauri/tauri.dev.conf.json");

	assert.equal(base.identifier, "com.quilltoolkit.app");
	assert.equal(dev.identifier, "com.quilltoolkit.app.dev");
	assert.deepEqual(Object.keys(dev).sort(), ["app", "identifier"], "dev must not alter release config beyond identity and window focus");
});

// @lat: [[infrastructure#Infrastructure#Build Configuration#Tauri Configuration#Development Identity]]
test("the dev window mirrors the release window except for focus", () => {
	const base = read("src-tauri/tauri.conf.json");
	const dev = read("src-tauri/tauri.dev.conf.json");

	assert.deepEqual(Object.keys(dev.app), ["windows"]);
	assert.equal(dev.app.windows.length, base.app.windows.length);

	const [baseWindow] = base.app.windows;
	const { focus: devFocus, ...devRest } = dev.app.windows[0];
	assert.deepEqual(devRest, baseWindow, "dev window must restate the release window, not fork it");
	assert.equal(baseWindow.focus, undefined, "release window keeps Tauri's default (focused)");
	assert.equal(devFocus, false, "dev window must not steal OS focus on a file-watch relaunch");
});

// @lat: [[infrastructure#Infrastructure#Build Configuration#Tauri Configuration#Development Identity]]
test("the standard dev command loads the dev config, builds do not", () => {
	assert.equal(read("package.json").scripts.tauri, "node scripts/tauri.mjs");

	assert.deepEqual(devArgs(["dev"]), [
		"dev",
		"--config",
		"src-tauri/tauri.dev.conf.json",
	]);
	assert.deepEqual(devArgs(["dev", "--release"]), [
		"dev",
		"--release",
		"--config",
		"src-tauri/tauri.dev.conf.json",
	]);
	assert.deepEqual(devArgs(["build"]), ["build"]);
	assert.deepEqual(devArgs(["dev", "--config", "other.json"]), [
		"dev",
		"--config",
		"other.json",
	]);
});
