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
	assert.deepEqual(Object.keys(dev), ["identifier"], "dev must not alter release config");
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
