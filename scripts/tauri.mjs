#!/usr/bin/env node
// `npm run tauri -- <cmd>` entry point.
//
// A `dev` run must never claim the production Tauri identity: that identity is
// what every Quill-owned data path (usage.db, auth_secret, session-index) and
// the single-instance gate derive from. Injecting the dev-identity config here
// — rather than in a separate opt-in script — means the standard dev command
// cannot mutate the installed app's state by omission. `build` and every other
// subcommand are passed through untouched, so release output is unchanged.
import { spawn } from "node:child_process";
import { realpathSync } from "node:fs";
import { fileURLToPath } from "node:url";

const DEV_CONFIG = "src-tauri/tauri.dev.conf.json";

export function devArgs(args) {
	if (args[0] !== "dev" || args.includes("--config") || args.includes("-c")) {
		return args;
	}
	return [...args, "--config", DEV_CONFIG];
}

if (process.argv[1] && realpathSync(process.argv[1]) === fileURLToPath(import.meta.url)) {
	const child = spawn("tauri", devArgs(process.argv.slice(2)), {
		stdio: "inherit",
		shell: process.platform === "win32",
	});
	child.on("exit", (code, signal) => process.exit(signal ? 1 : (code ?? 1)));
}
