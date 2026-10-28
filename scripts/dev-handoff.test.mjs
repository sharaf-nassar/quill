import assert from "node:assert/strict";
import { fork } from "node:child_process";
import { once } from "node:events";
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { createServer } from "node:net";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { acquireLease, portAvailable, runHandoff, waitFor } from "./dev-handoff.mjs";
import { findQuitItem, interruptOwnerTree, macProduction, processIdentity, productionEnvironment, sameProcess } from "./dev-handoff-platform.mjs";

function operations(events, overrides = {}) {
	return {
		productionWasRunning: true,
		ownerAlive: () => true,
		build: async () => { events.push("build"); },
		quit: async () => { events.push("quit"); },
		waitForPorts: async () => { events.push("ports"); },
		serve: async () => { events.push("serve"); },
		stopCli: async () => { events.push("interrupt-cli"); },
		stopFrontend: async () => { events.push("stop"); },
		restore: async () => { events.push("restore"); },
		...overrides,
	};
}

// @lat: [[dev-handoff-tests#Development Handoff Tests#Lifecycle Ordering]]
test("production stays up during preparation and returns after frontend shutdown", async () => {
	const events = [];
	await runHandoff(operations(events));
	assert.deepEqual(events, ["build", "quit", "ports", "serve", "stop", "restore"]);
});

// @lat: [[dev-handoff-tests#Development Handoff Tests#Failure Recovery]]
test("startup failure, partial quit and an early owner exit preserve restoration", async () => {
	for (const stage of ["build", "quit", "waitForPorts", "serve", "stopFrontend"]) {
		const events = [];
		await assert.rejects(runHandoff(operations(events, {
			[stage]: async () => { events.push(`failed:${stage}`); throw new Error(stage); },
		})), new RegExp(stage));
		assert.equal(events.includes("restore"), stage !== "build", stage);
		if (stage !== "stopFrontend") assert.ok(events.indexOf("interrupt-cli") < events.indexOf("stop"));
	}
	const stopped = [];
	await runHandoff(operations(stopped, { ownerAlive: () => false }));
	assert.deepEqual(stopped, ["build", "stop"], "do not stop production after Ctrl+C during preparation");
	const absent = [];
	await assert.rejects(runHandoff(operations(absent, {
		productionWasRunning: false,
		build: async () => { throw new Error("build failed"); },
	})), /build failed/);
	assert.deepEqual(absent, ["interrupt-cli", "stop", "restore"], "stop failed dev before starting an initially absent production app");
});

// @lat: [[dev-handoff-tests#Development Handoff Tests#Exclusive Ownership]]
test("a live handoff cannot be stolen and a dead owner's lease can be recovered", { skip: process.platform === "win32" }, () => {
	const root = mkdtempSync(join(tmpdir(), "quill-dev-lock-"));
	const lock = join(root, "lock");
	try {
		const release = acquireLease(lock, { pid: 11, start: "first" }, () => true);
		assert.throws(() => acquireLease(lock, { pid: 22, start: "second" }, () => true), /already owns/);
		release();
		acquireLease(lock, { pid: 11, start: "dead" }, () => false);
		const recovered = acquireLease(lock, { pid: 22, start: "next" }, () => false);
		recovered();
		assert.equal(existsSync(lock), false);
		const identity = processIdentity(process.pid);
		assert.equal(sameProcess(identity), true);
		assert.equal(sameProcess({ ...identity, start: "reused-pid" }), false);
	} finally { rmSync(root, { recursive: true, force: true }); }
});

// @lat: [[dev-handoff-tests#Development Handoff Tests#Port Release Barrier]]
test("restoration cannot cross the listener release barrier", async () => {
	const server = createServer();
	server.listen(0, "127.0.0.1");
	await once(server, "listening");
	const port = server.address().port;
	try {
		assert.equal(await portAvailable(port), false);
		await assert.rejects(waitFor(() => portAvailable(port), "fixture listener", 20), /Timed out/);
	} finally { await new Promise((resolve) => server.close(resolve)); }
	assert.equal(await portAvailable(port), true);
});

// @lat: [[dev-handoff-tests#Development Handoff Tests#Native Quit And Clean Relaunch]]
test("Quit resolves only an enabled native action and production sheds dev overrides", () => {
	assert.equal(findQuitItem([0, {}, [
		{ data: [4, { label: { data: "Quit" }, enabled: { data: false } }, []] },
		{ data: [5, { label: { data: "Show Widget" } }, []] },
		{ data: [6, { label: { data: "_Quit" } }, []] },
	]]), 6);
	assert.equal(findQuitItem([0, {}, []]), undefined);
	const environment = {
		HOME: "/home/test", PATH: "/usr/bin", DISPLAY: ":1", DBUS_SESSION_BUS_ADDRESS: "session",
		TAURI_CONFIG: "dev", QUILL_DATA_DIR: "/dev-data", QUILL_PORT: "19900", QUILL_DEMO_MODE: "1",
		CARGO_TARGET_DIR: "/target", APPDIR: "/mount", APPIMAGE: "/wrong.AppImage", LD_LIBRARY_PATH: "/mount/lib",
		LD_PRELOAD: "/shim", DYLD_LIBRARY_PATH: "/dev/lib", NODE_OPTIONS: "--inspect", NODE_CHANNEL_FD: "3",
	};
	assert.deepEqual(productionEnvironment(environment), {
		HOME: "/home/test", PATH: "/usr/bin", DISPLAY: ":1", DBUS_SESSION_BUS_ADDRESS: "session",
	});
	assert.equal(environment.QUILL_PORT, "19900");
});

// @lat: [[dev-handoff-tests#Development Handoff Tests#Mac Production Identity]]
test("macOS discovery selects the production bundle and preserves paths with spaces", { skip: process.platform === "win32" }, () => {
	const run = (command, args) => {
		if (command === "ps") {
			assert.deepEqual(args.slice(0, 3), ["-x", "-U", String(process.getuid())]);
			return "10 /work/target/debug/quill\n11 /Applications/Dev.app/Contents/MacOS/quill\n12 /Users/test/My Apps/Quill.app/Contents/MacOS/quill";
		}
		assert.equal(command, "/usr/libexec/PlistBuddy");
		return args[2].includes("Dev.app") ? "com.quilltoolkit.app.dev" : "com.quilltoolkit.app";
	};
	const plan = macProduction(run, (pid) => ({ pid, start: "start" }));
	assert.equal(plan.identity.pid, 12);
	assert.deepEqual(plan.launch, { command: "open", args: ["-g", "/Users/test/My Apps/Quill.app"] });
});

// @lat: [[dev-handoff-tests#Development Handoff Tests#Detached Supervisor Lifetime]]
test("the supervisor survives hook cleanup and waits for CLI exit, including Ctrl+C", {
	skip: !["linux", "darwin"].includes(process.platform), timeout: 20_000,
}, async () => {
	const root = mkdtempSync(join(tmpdir(), "quill-dev-processes-"));
	const fixture = join(root, "fixture.mjs");
	const core = new URL("./dev-handoff.mjs", import.meta.url).href;
	const platform = new URL("./dev-handoff-platform.mjs", import.meta.url).href;
	writeFileSync(fixture, `
import { fork } from 'node:child_process';
import { writeFileSync } from 'node:fs';
import { brokerSupervisor, waitFor } from ${JSON.stringify(core)};
import { processIdentity, sameProcess } from ${JSON.stringify(platform)};
const [role, file, report, ownerJson] = process.argv.slice(2);
if (role === 'owner') {
 const backend = fork(file, ['backend'], { stdio: 'ignore' });
 const hook = fork(file, ['hook', file, report, JSON.stringify(processIdentity(process.pid))], { stdio: ['ignore', 'ignore', 'inherit', 'ipc'] });
 hook.on('message', message => process.send({ ...message, hook: processIdentity(hook.pid), backend: processIdentity(backend.pid) }));
 process.on('message', () => { backend.kill('SIGTERM'); process.exit(0); });
} else if (role === 'hook') {
 const broker = fork(file, ['broker', file, report, ownerJson], { stdio: ['ignore', 'ignore', 'inherit', 'ipc'] });
 broker.on('message', message => process.send(message));
 setInterval(() => {}, 1000);
} else if (role === 'broker') {
 await brokerSupervisor(file, ['guardian', file, report, ownerJson, JSON.stringify(processIdentity(process.pid))]);
} else if (role === 'backend') {
 setInterval(() => {}, 1000);
} else {
 const owner = JSON.parse(ownerJson);
 const broker = JSON.parse(process.argv[6]);
 process.send({ ready: processIdentity(process.pid) });
 await waitFor(() => !sameProcess(broker), 'broker detachment');
 writeFileSync(report + '.detached', JSON.stringify({ parent: process.ppid, broker: broker.pid }));
 await waitFor(() => !sameProcess(owner), 'owner exit');
 writeFileSync(report, 'restored');
}
`);
	try {
		for (const mode of ["normal", "SIGINT"]) {
			const report = join(root, mode);
			const owner = fork(fixture, ["owner", fixture, report], { stdio: ["ignore", "ignore", "inherit", "ipc"] });
			let received;
			try {
				[received] = await once(owner, "message");
				await waitFor(() => existsSync(`${report}.detached`), "fixture detachment", 5_000);
				const detached = JSON.parse(readFileSync(`${report}.detached`, "utf8"));
				assert.notEqual(detached.parent, received.hook.pid);
				assert.notEqual(detached.parent, detached.broker);
				process.kill(received.hook.pid, "SIGKILL");
				await waitFor(() => !sameProcess(received.hook), "hook termination", 5_000);
				assert.equal(sameProcess(received.ready), true, "Tauri hook cleanup must not kill the supervisor");
				assert.equal(existsSync(report), false, "a live CLI still owns the session across rebuilds");
				const exited = once(owner, "exit");
				if (mode === "normal") owner.send("exit"); else interruptOwnerTree(processIdentity(owner.pid));
				await exited;
				await waitFor(() => existsSync(report), "fixture production restoration", 5_000);
				assert.equal(readFileSync(report, "utf8"), "restored");
				await waitFor(() => !sameProcess(received.backend), "owned backend termination", 5_000);
			} finally {
				for (const identity of [received?.hook, received?.backend, received?.ready, processIdentity(owner.pid)]) {
					if (identity && sameProcess(identity)) process.kill(identity.pid, "SIGKILL");
				}
			}
		}
	} finally { rmSync(root, { recursive: true, force: true }); }
});
