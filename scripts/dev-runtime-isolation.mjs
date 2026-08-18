#!/usr/bin/env node
// Live regression for development runtime isolation (Linux only).
//
// Builds a production-identity and a dev-identity Quill, then runs both at the
// same time under one throwaway HOME, one Xvfb display, and one session bus,
// each on explicitly overridden ports (the two identities share the published
// pair, so only an override lets this fixture hold both up at once).
// It asserts the pair keeps separate data — context store, cache root, app
// data — while sharing the one provider handshake, and that starting the dev
// build leaves every production-owned byte, including the shared contract and
// the real provider integration assets, untouched.
//
// Heavyweight by nature, so it is not part of `npm test`. Run it directly:
//
//   npm run test:dev-isolation
//
// Reuses an existing build when QUILL_PROD_BIN / QUILL_DEV_BIN are set.
import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { DatabaseSync } from "node:sqlite";
import {
	copyFileSync,
	existsSync,
	mkdirSync,
	mkdtempSync,
	readFileSync,
	readdirSync,
	rmSync,
	statSync,
	writeFileSync,
} from "node:fs";
import { createServer } from "node:net";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { reporterCandidateForPath } from "../src-tauri/pi-integration/quill.ts";

const REPO = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const PROD_ID = "com.quilltoolkit.app";
const DEV_ID = `${PROD_ID}.dev`;
const DISPLAY = ":97";
const BOOT_TIMEOUT_MS = 90_000;

const cleanups = [];
const step = (message) => console.log(`[dev-isolation] ${message}`);

function requireBinary(name) {
	const found = spawnSync("sh", ["-c", `command -v ${name}`], { encoding: "utf8" });
	if (found.status !== 0) throw new Error(`required tool not found: ${name}`);
	return found.stdout.trim();
}

function run(command, args, options = {}) {
	const result = spawnSync(command, args, { stdio: "inherit", ...options });
	if (result.status !== 0) {
		throw new Error(`${command} ${args.join(" ")} exited with ${result.status ?? result.signal}`);
	}
}

function buildBinaries() {
	const fromEnv = { prod: process.env.QUILL_PROD_BIN, dev: process.env.QUILL_DEV_BIN };
	if (fromEnv.prod && fromEnv.dev) return fromEnv;

	if (!existsSync(join(REPO, "dist", "index.html"))) {
		step("building the frontend bundle");
		run("npx", ["vite", "build"], { cwd: REPO });
	}
	const built = join(REPO, "src-tauri", "target", "debug", "quill");
	const staged = mkdtempSync(join(tmpdir(), "quill-isolation-bin-"));
	cleanups.push(() => rmSync(staged, { recursive: true, force: true }));

	// One target dir, two sequential builds: changing the embedded identity
	// only rebuilds the app crate, and running them in series keeps cargo's
	// build lock uncontended.
	const out = {};
	for (const [name, identifier] of [
		["prod", PROD_ID],
		["dev", DEV_ID],
	]) {
		step(`building the ${name} binary (${identifier})`);
		run("cargo", ["build", "--bin", "quill"], {
			cwd: join(REPO, "src-tauri"),
			env: { ...process.env, TAURI_CONFIG: JSON.stringify({ identifier }) },
		});
		out[name] = join(staged, `quill-${name}`);
		copyFileSync(built, out[name]);
	}
	return out;
}

function seedFixtureHome(ports) {
	const home = mkdtempSync(join(tmpdir(), "quill-isolation-home-"));
	cleanups.push(() => rmSync(home, { recursive: true, force: true }));

	const files = {
		// Production's Quill-owned runtime state.
		config: join(home, ".config/quill/config.json"),
		contextDb: join(home, ".config/quill/context/context.db"),
		contextSentinel: join(home, ".config/quill/context/production.sentinel"),
		cache: join(home, ".cache/quill/state.json"),
		// The agents' own directories: read as inputs, never written by Quill dev.
		piExtension: join(home, ".pi/agent/extensions/quill.ts"),
		claudeSettings: join(home, ".claude/settings.json"),
		codexConfig: join(home, ".codex/config.toml"),
	};
	const contents = {
		config: JSON.stringify(
			{
				url: `http://localhost:${ports.prod.main}`,
				context_url: `http://localhost:${ports.prod.context}`,
				hostname: "fixture-host",
				secret: "production-secret-placeholder",
			},
			null,
			2,
		),
		contextSentinel: "production context sidecar\n",
		cache: '{"production":"cache"}\n',
		piExtension: "// production-installed Quill extension\n",
		claudeSettings: JSON.stringify({ hooks: { SessionStart: [] } }, null, 2),
		codexConfig: 'model = "production"\n',
	};
	for (const [key, path] of Object.entries(files)) {
		if (!(key in contents)) continue;
		mkdirSync(dirname(path), { recursive: true });
		writeFileSync(path, contents[key]);
	}
	return { home, files };
}

function initDatabase(binary, home, identifier) {
	const dbPath = join(fixtureRoots(home).data, identifier, "usage.db");
	mkdirSync(dirname(dbPath), { recursive: true });
	run(binary, ["--init-database", dbPath], { env: childEnv(home) });
	// The context listener is setting-gated, so enable it up front: an
	// unbound context port would prove nothing about isolation.
	const db = new DatabaseSync(dbPath);
	db.prepare("INSERT OR REPLACE INTO settings (key, value) VALUES (?, ?)").run(
		"context_http.enabled",
		"true",
	);
	db.close();
	return dbPath;
}

/// The fixture roots every assertion derives from. `HOME` alone is not
/// enough: an inherited `XDG_DATA_HOME` / `XDG_CONFIG_HOME` / `XDG_CACHE_HOME`
/// wins over it and would drag the fixture processes back onto the real ones.
function fixtureRoots(home) {
	return {
		data: join(home, ".local/share"),
		config: join(home, ".config"),
		cache: join(home, ".cache"),
		state: join(home, ".local/state"),
		runtime: join(home, "run"),
	};
}

function childEnv(home, ports) {
	const roots = fixtureRoots(home);
	const env = { ...process.env, HOME: home, DISPLAY };
	for (const key of Object.keys(env)) {
		if (key.startsWith("XDG_")) delete env[key];
	}
	for (const [key, path] of Object.entries(roots)) {
		mkdirSync(path, { recursive: true });
		env[`XDG_${key.toUpperCase()}${key === "runtime" ? "_DIR" : "_HOME"}`] = path;
	}
	delete env.QUILL_DEMO_MODE;
	delete env.QUILL_DEV_INTEGRATIONS;
	if (ports) {
		env.QUILL_PORT = String(ports.main);
		env.QUILL_CONTEXT_PORT = String(ports.context);
	} else {
		delete env.QUILL_PORT;
		delete env.QUILL_CONTEXT_PORT;
	}
	return env;
}

function startXvfb() {
	requireBinary("Xvfb");
	const proc = spawn("Xvfb", [DISPLAY, "-screen", "0", "1280x1024x24", "-nolisten", "tcp"], {
		stdio: "ignore",
	});
	cleanups.push(() => proc.kill("SIGKILL"));
	return proc;
}

function startSessionBus() {
	const dbus = requireBinary("dbus-daemon");
	const result = spawnSync(dbus, ["--session", "--print-address", "--fork", "--print-pid"], {
		encoding: "utf8",
	});
	if (result.status !== 0) throw new Error(`dbus-daemon failed: ${result.stderr}`);
	const [address, pid] = result.stdout.trim().split("\n");
	cleanups.push(() => {
		try {
			process.kill(Number(pid), "SIGKILL");
		} catch {
			/* already gone */
		}
	});
	return address;
}

function launch(binary, home, ports, busAddress, label) {
	const proc = spawn(binary, [], {
		env: { ...childEnv(home, ports), DBUS_SESSION_BUS_ADDRESS: busAddress },
		stdio: ["ignore", "pipe", "pipe"],
	});
	const log = [];
	proc.stdout.on("data", (chunk) => log.push(String(chunk)));
	proc.stderr.on("data", (chunk) => log.push(String(chunk)));
	proc.on("exit", (code, signal) => log.push(`\n<${label} exited code=${code} signal=${signal}>`));
	cleanups.push(() => proc.kill("SIGKILL"));
	return { proc, label, log };
}

const sleep = (ms) => new Promise((done) => setTimeout(done, ms));

async function waitForHealth(instance, port) {
	const deadline = Date.now() + BOOT_TIMEOUT_MS;
	while (Date.now() < deadline) {
		if (instance.proc.exitCode !== null) {
			throw new Error(`${instance.label} exited before binding ${port}:\n${instance.log.join("")}`);
		}
		try {
			const response = await fetch(`http://127.0.0.1:${port}/api/v1/health`);
			if (response.ok) return;
		} catch {
			/* not listening yet */
		}
		await sleep(250);
	}
	throw new Error(`${instance.label} never answered on ${port}:\n${instance.log.join("")}`);
}

async function waitForContext(instance, port, secret) {
	const deadline = Date.now() + BOOT_TIMEOUT_MS;
	while (Date.now() < deadline) {
		const status = await contextStatus(port, secret);
		if (status === 200) return;
		await sleep(250);
	}
	throw new Error(`${instance.label} never bound context port ${port}:\n${instance.log.join("")}`);
}

async function contextStatus(port, secret) {
	try {
		const response = await fetch(`http://127.0.0.1:${port}/api/v1/context/stats`, {
			method: "POST",
			headers: { "Content-Type": "application/json", Authorization: `Bearer ${secret}` },
			body: "{}",
		});
		return response.status;
	} catch {
		return 0;
	}
}

function readSecret(home, identifier) {
	return readFileSync(join(fixtureRoots(home).data, identifier, "auth_secret"), "utf8").trim();
}

/// Hash + mtime for every file below `root`, so a rewrite that happens to
/// preserve the bytes is still caught.
function snapshot(root) {
	const entries = {};
	if (existsSync(root)) {
		for (const relativePath of readdirSync(root, { recursive: true })) {
			const path = join(root, relativePath);
			const stat = statSync(path);
			if (stat.isFile()) {
				entries[path] = `${createHash("sha256").update(readFileSync(path)).digest("hex")}@${stat.mtimeMs}`;
			}
		}
	}
	return entries;
}

function snapshotProduction(home) {
	const roots = [".config/quill", ".cache/quill", ".pi", ".claude", ".codex"];
	return Object.fromEntries(roots.map((root) => [root, snapshot(join(home, root))]));
}

/// Ports are assigned by the kernel, never hardcoded: the published defaults
/// belong to whatever Quill is already running on this host, and probing
/// those would measure — or disturb — someone's real instance. The unit
/// tests own the default-and-override resolution; this fixture only has to
/// keep the two processes on distinct endpoints and prove the overrides land.
async function assignPort() {
	return new Promise((done, fail) => {
		const probe = createServer();
		probe.once("error", fail);
		probe.listen(0, "127.0.0.1", () => {
			const { port } = probe.address();
			probe.close(() => done(port));
		});
	});
}

async function main() {
	if (process.platform !== "linux") {
		throw new Error("this regression needs Xvfb and a session bus; run it on Linux");
	}
	const candidateHome = join(tmpdir(), "quill-reporter-policy");
	const candidateCwd = join(candidateHome, "project");
	for (const path of [
		join(candidateCwd, ".pi/extensions/quill.ts"),
		join(REPO, "src-tauri/pi-integration/quill.ts"),
	]) {
		assert.equal(
			reporterCandidateForPath(path, {
				home: candidateHome,
				cwd: candidateCwd,
				agentDir: join(candidateHome, ".pi/agent"),
			}).eligible,
			false,
			"project/development reporters stay inert without exact path selection",
		);
		assert.equal(
			reporterCandidateForPath(path, {
				home: candidateHome,
				cwd: candidateCwd,
				agentDir: join(candidateHome, ".pi/agent"),
				selectedPath: path,
			}).eligible,
			true,
			"exact path selection opts one unofficial reporter in",
		);
	}

	const binaries = buildBinaries();
	const ports = {
		prod: { main: await assignPort(), context: await assignPort() },
		dev: { main: await assignPort(), context: await assignPort() },
	};
	const assigned = [ports.prod.main, ports.prod.context, ports.dev.main, ports.dev.context];
	assert.equal(new Set(assigned).size, 4, "each listener needs its own port");
	const { home, files } = seedFixtureHome(ports);
	step(`fixture home ${home}, ports ${assigned.join("/")}`);

	initDatabase(binaries.prod, home, PROD_ID);
	initDatabase(binaries.dev, home, DEV_ID);

	startXvfb();
	const busAddress = startSessionBus();
	await sleep(1000);

	step("starting the production build");
	const production = launch(binaries.prod, home, ports.prod, busAddress, "production");
	await waitForHealth(production, ports.prod.main);
	const prodSecret = readSecret(home, PROD_ID);
	await waitForContext(production, ports.prod.context, prodSecret);

	const before = snapshotProduction(home);
	assert.ok(
		Object.keys(before[".config/quill"]).length > 0,
		"the fixture must have production state to protect",
	);

	step("starting the dev build beside it");
	const development = launch(binaries.dev, home, ports.dev, busAddress, "development");
	await waitForHealth(development, ports.dev.main);
	const devSecret = readSecret(home, PROD_ID);
	await waitForContext(development, ports.dev.context, devSecret);

	// Both alive, each on the endpoints its explicit overrides named.
	assert.equal(production.proc.exitCode, null, "production must survive the dev launch");
	assert.equal(development.proc.exitCode, null, "the dev build must survive the shared bus");
	assert.ok((await fetch(`http://127.0.0.1:${ports.prod.main}/api/v1/health`)).ok);
	assert.ok((await fetch(`http://127.0.0.1:${ports.dev.main}/api/v1/health`)).ok);

	// One shared credential: a provider holds a single contract, so both
	// builds must accept the secret that contract publishes. Only the store
	// behind each listener is private.
	assert.equal(prodSecret, devSecret, "the contract's credential is machine-global");
	assert.ok(
		!existsSync(join(fixtureRoots(home).data, DEV_ID, "auth_secret")),
		"a dev run must not mint a second credential",
	);
	assert.equal(await contextStatus(ports.prod.context, prodSecret), 200);
	assert.equal(await contextStatus(ports.dev.context, devSecret), 200);

	// Distinct context and cache roots.
	const devConfigRoot = join(fixtureRoots(home).config, "quill-dev");
	const devContextDb = join(devConfigRoot, "context/context.db");
	assert.ok(existsSync(files.contextDb), "production owns a valid context store");
	assert.ok(existsSync(devContextDb), "dev owns its context store");
	assert.notEqual(files.contextDb, devContextDb);
	const prodCacheRoot = join(fixtureRoots(home).data, PROD_ID, "WebKitCache");
	const devCacheRoot = join(fixtureRoots(home).data, DEV_ID, "WebKitCache");
	assert.ok(existsSync(prodCacheRoot), "production owns its app cache root");
	assert.ok(existsSync(devCacheRoot), "dev owns its app cache root");
	assert.notEqual(prodCacheRoot, devCacheRoot);

	// Nothing production owns moved.
	const after = snapshotProduction(home);
	assert.deepEqual(after, before, "dev startup must not touch production-owned state");
	const contract = JSON.parse(readFileSync(files.config, "utf8"));
	assert.equal(contract.url, `http://localhost:${ports.prod.main}`);
	assert.equal(contract.context_url, `http://localhost:${ports.prod.context}`);
	assert.equal(contract.secret, "production-secret-placeholder");

	step("all assertions passed");
}

main()
	.then(() => {
		for (const cleanup of cleanups.reverse()) cleanup();
		process.exit(0);
	})
	.catch((error) => {
		console.error(`[dev-isolation] FAILED: ${error.message}`);
		for (const cleanup of cleanups.reverse()) cleanup();
		process.exit(1);
	});
