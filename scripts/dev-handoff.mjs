#!/usr/bin/env node
// Tauri may SIGKILL its hook tree. The broker exits so the supervisor is no
// longer in that tree; the hook stays alive for Tauri's normal error handling.
import { fork, spawn } from "node:child_process";
import { randomUUID } from "node:crypto";
import { mkdirSync, readFileSync, readdirSync, realpathSync, rmdirSync, statSync, unlinkSync, writeFileSync } from "node:fs";
import { createServer } from "node:net";
import { homedir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import {
	findTauriOwner, interruptOwnerTree, launchProduction, processIdentity, productionPlan,
	productionRunning, quitProduction, sameProcess,
} from "./dev-handoff-platform.mjs";

const FILE = fileURLToPath(import.meta.url);
const ROOT = resolve(dirname(FILE), "..");
const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const log = (message) => console.log(`[quill-dev] ${message}`);
class DevEnded extends Error {}

export async function waitFor(check, description, timeout = 30_000) {
	const deadline = Date.now() + timeout;
	while (!await check()) {
		if (Date.now() >= deadline) throw new Error(`Timed out waiting for ${description}.`);
		await delay(250);
	}
}

export function portAvailable(port) {
	return new Promise((resolve, reject) => {
		const server = createServer();
		server.once("error", (error) => error.code === "EADDRINUSE" ? resolve(false) : reject(error));
		server.listen({ host: "127.0.0.1", port, exclusive: true }, () => server.close(() => resolve(true)));
	});
}

// A unique owner filename makes stale cleanup safe against another claimant:
// only the process that removes that exact file may remove its directory.
export function acquireLease(directory, identity, alive = sameProcess) {
	mkdirSync(dirname(directory), { recursive: true, mode: 0o700 });
	const token = `${randomUUID()}.json`;
	for (let attempt = 0; attempt < 4; attempt++) {
		try {
			mkdirSync(directory, { mode: 0o700 });
		} catch (error) {
			if (error.code !== "EEXIST") throw error;
			const files = readdirSync(directory);
			if (files.length > 1) throw new Error("Dev handoff lock is inconsistent; refusing another session.");
			if (files.length === 1) {
				const old = join(directory, files[0]);
				if (alive(JSON.parse(readFileSync(old, "utf8")))) throw new Error("Another Quill dev session already owns the production handoff.");
				try { unlinkSync(old); } catch (failure) { if (failure.code === "ENOENT") continue; throw failure; }
			}
			try { rmdirSync(directory); } catch (failure) { if (!["ENOENT", "ENOTEMPTY"].includes(failure.code)) throw failure; }
			continue;
		}
		const inode = statSync(directory).ino;
		const file = join(directory, token);
		writeFileSync(file, JSON.stringify(identity), { flag: "wx", mode: 0o600 });
		if (statSync(directory).ino !== inode) {
			unlinkSync(file);
			throw new Error("Dev handoff lock changed during acquisition; retry the command.");
		}
		return () => {
			unlinkSync(file);
			try { rmdirSync(directory); } catch (error) { if (!["ENOENT", "ENOTEMPTY"].includes(error.code)) throw error; }
		};
	}
	throw new Error("Could not acquire the Quill dev handoff lock; retry the command.");
}

export async function stopChildGroup(child) {
	const signal = (name) => {
		try { process.kill(-child.pid, name); return true; }
		catch (error) { if (error.code === "ESRCH") return false; throw error; }
	};
	if (!child.pid || !signal("SIGTERM")) return;
	const deadline = Date.now() + 3_000;
	while (signal(0)) {
		if (Date.now() >= deadline) { signal("SIGKILL"); break; }
		await delay(50);
	}
}

function frontendCommand(args, owner, children, serving = false) {
	return new Promise((resolve, reject) => {
		const child = spawn("npm", args, { cwd: ROOT, stdio: "inherit", detached: true });
		children.add(child);
		const timer = setInterval(() => {
			if (!sameProcess(owner)) { clearInterval(timer); reject(new DevEnded()); }
		}, 250);
		const finish = () => clearInterval(timer);
		child.once("error", (error) => { finish(); children.delete(child); reject(error); });
		child.once("exit", (code, signal) => {
			finish();
			if (!serving && code === 0) { children.delete(child); resolve(); }
			else reject(new Error(`Frontend ${args.join(" ")} exited (${signal || code}).`));
		});
	});
}

// Keep the restoration decision in one place, including partial quit failures.
export async function runHandoff(operations) {
	let restore = !operations.productionWasRunning;
	try {
		await operations.build();
		if (!operations.ownerAlive()) return;
		restore = true;
		await operations.quit();
		await operations.waitForPorts();
		if (operations.ownerAlive()) await operations.serve();
	} catch (error) {
		if (!(error instanceof DevEnded)) {
			await operations.stopCli();
			throw error;
		}
	} finally {
		try { await operations.stopFrontend(); }
		finally { if (restore) await operations.restore(); }
	}
}

async function supervise(owner, broker) {
	const cache = process.platform === "darwin" ? join(homedir(), "Library/Caches")
		: process.env.XDG_RUNTIME_DIR || process.env.XDG_CACHE_HOME || join(homedir(), ".cache");
	const release = acquireLease(join(cache, "quill-dev-handoff", "session.lock"), processIdentity(process.pid));
	const children = new Set();
	try {
		const config = JSON.parse(readFileSync(join(ROOT, "src-tauri/tauri.conf.json"), "utf8"));
		const frontendPort = Number(new URL(config.build.devUrl).port);
		if (!await portAvailable(frontendPort)) throw new Error(`Port ${frontendPort} is already occupied. Stop the existing dev server first.`);
		const plan = productionPlan();
		if (!plan) log("No installed Quill found; this session cannot restore production.");
		process.send({ ready: processIdentity(process.pid) });
		await waitFor(() => !sameProcess(broker), "supervisor detachment", 10_000);
		const ports = [...new Set([19876, 19877, Number(process.env.QUILL_PORT || 19876), Number(process.env.QUILL_CONTEXT_PORT || 19877)])];
		const portsFree = async () => (await Promise.all(ports.map(portAvailable))).every(Boolean);
		await runHandoff({
			productionWasRunning: Boolean(plan?.identity),
			ownerAlive: () => sameProcess(owner),
			build: () => frontendCommand(["run", "build:web"], owner, children),
			quit: async () => {
				if (plan?.identity) {
					log("Asking production Quill to quit gracefully.");
					quitProduction(plan);
					await waitFor(() => !sameProcess(plan.identity), "production Quill to exit");
				}
			},
			waitForPorts: () => waitFor(portsFree, "Quill's shared ports"),
			serve: () => frontendCommand(["run", "dev"], owner, children, true),
			stopCli: () => interruptOwnerTree(owner),
			stopFrontend: () => Promise.all([...children].map(stopChildGroup)),
			restore: async () => {
				if (!plan || productionRunning()) return;
				await waitFor(portsFree, "dev Quill to release its ports before restoring production");
				log("Restoring production Quill.");
				await launchProduction(plan);
				await waitFor(async () => productionRunning() && !await portAvailable(19876), "production Quill to start", 90_000);
				log("Production Quill is running.");
			},
		});
	} finally {
		release();
	}
}

function childReady(child) {
	return new Promise((resolve, reject) => {
		child.once("error", reject);
		child.once("exit", (code, signal) => reject(new Error(`Dev supervisor failed to start (${signal || code}).`)));
		child.once("message", (message) => message.ready ? resolve(message.ready) : reject(new Error(message.error)));
	});
}

export async function brokerSupervisor(file, args) {
	const child = fork(file, args, { detached: true, stdio: ["ignore", "inherit", "inherit", "ipc"] });
	const ready = await childReady(child);
	process.send({ ready });
	child.disconnect();
	child.unref();
	process.disconnect();
}

async function main() {
	if (process.argv[2] === "--supervisor") {
		await supervise(JSON.parse(process.argv[3]), JSON.parse(process.argv[4]));
	} else if (process.argv[2] === "--broker") {
		await brokerSupervisor(FILE, ["--supervisor", process.argv[3], JSON.stringify(processIdentity(process.pid))]);
	} else if (!["linux", "darwin"].includes(process.platform)) {
		const child = spawn("npm run build:web && npm run dev", { cwd: ROOT, shell: true, stdio: "inherit" });
		child.once("exit", (code) => process.exit(code ?? 1));
	} else {
		const owner = findTauriOwner();
		const broker = fork(FILE, ["--broker", JSON.stringify(owner)], { stdio: ["ignore", "inherit", "inherit", "ipc"] });
		const guardian = await childReady(broker);
		await waitFor(() => !sameProcess(guardian), "dev supervisor exit", Infinity);
		throw new Error("Dev supervisor exited; stopping the Tauri dev command.");
	}
}

if (process.argv[1] && realpathSync(process.argv[1]) === FILE) {
	main().catch((error) => {
		console.error(`[quill-dev] ${error.message}`);
		if (error.code === "ENOENT") console.error("Install the missing system command before retrying; nothing is installed automatically.");
		process.exitCode = 1;
	});
}
