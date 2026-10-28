// Native production discovery and graceful quit. Never signal an arbitrary Quill PID.
import { execFileSync, spawn } from "node:child_process";
import { accessSync, constants, existsSync, readFileSync, readlinkSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";

const PROD_ID = "com.quilltoolkit.app";

function output(command, args) {
	return execFileSync(command, args, { encoding: "utf8", timeout: 10_000, stdio: ["ignore", "pipe", "pipe"] }).trim();
}

export function processIdentity(pid, platform = process.platform) {
	try {
		if (platform === "linux") {
			const stat = readFileSync(`/proc/${pid}/stat`, "utf8");
			const fields = stat.slice(stat.lastIndexOf(")") + 2).split(" ");
			if (fields[0] === "Z") return null;
			return { pid, start: fields[19], parent: Number(fields[1]) };
		}
		const fields = output("ps", ["-p", String(pid), "-o", "lstart=", "-o", "stat=", "-o", "ppid="]).split(/\s+/);
		const start = fields.slice(0, -2).join(" ");
		return !start || fields.at(-2).startsWith("Z") ? null : { pid, start, parent: Number(fields.at(-1)) };
	} catch (error) {
		if ((error.code === "ENOENT" && platform === "linux") || error.code === "ESRCH" || (error.status === 1 && platform === "darwin")) return null;
		throw error;
	}
}

export function sameProcess(identity) {
	return identity && processIdentity(identity.pid)?.start === identity.start;
}

export function interruptOwnerTree(owner) {
	if (!sameProcess(owner)) return;
	const rows = output("ps", ["-e", "-o", "pid=,ppid="]).split("\n").map((line) => line.trim().split(/\s+/).map(Number));
	const owned = new Set([owner.pid]);
	for (let previous = 0; previous !== owned.size;) {
		previous = owned.size;
		for (const [pid, parent] of rows) if (owned.has(parent)) owned.add(pid);
	}
	const identities = [...owned].map((pid) => processIdentity(pid))
		.filter((identity) => identity && (identity.pid === owner.pid || owned.has(identity.parent)));
	if (!sameProcess(owner)) return;
	for (const identity of identities.reverse()) {
		if (!sameProcess(identity)) continue;
		try { process.kill(identity.pid, "SIGINT"); }
		catch (error) { if (error.code !== "ESRCH") throw error; }
	}
}

export function findTauriOwner() {
	let pid = process.ppid;
	for (let depth = 0; pid > 1 && depth < 12; depth++) {
		const identity = processIdentity(pid);
		if (!identity) break;
		const command = process.platform === "linux"
			? readFileSync(`/proc/${pid}/cmdline`, "utf8").replaceAll("\0", " ")
			: output("ps", ["-p", String(pid), "-o", "command="]);
		if (/(?:^|[/ ])tauri(?:\.js)?\s+dev(?:\s|$)/.test(command)) return identity;
		pid = identity.parent;
	}
	throw new Error("Dev handoff must be launched by tauri dev through beforeDevCommand.");
}

function bus(...args) {
	const result = output("busctl", ["--user", "--json=short", "--auto-start=no", "--timeout=5", ...args]);
	return result ? JSON.parse(result).data : [];
}

function busCall(service, path, iface, method, ...args) {
	return bus("call", service, path, iface, method, ...args);
}

function busPid(service) {
	return busCall("org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus", "GetConnectionUnixProcessID", "s", service)[0];
}

export function findQuitItem(node) {
	const [id, properties, children] = node;
	if (properties.label?.data?.replaceAll("_", "") === "Quit" && properties.enabled?.data !== false) return id;
	for (const child of children) {
		const found = findQuitItem(child.data);
		if (found !== undefined) return found;
	}
}

function linuxProduction() {
	const names = busCall("org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus", "ListNames")[0];
	const name = `${PROD_ID}.SingleInstance`;
	if (!names.includes(name)) return null;
	const pid = busPid(name);
	const identity = processIdentity(pid);
	if (!identity) return null;
	const environment = readFileSync(`/proc/${pid}/environ`, "utf8").split("\0");
	const appImage = environment.find((value) => value.startsWith("APPIMAGE="))?.slice(9);
	const binary = appImage || readlinkSync(`/proc/${pid}/exe`);
	accessSync(binary, constants.X_OK);
	const items = bus("get-property", "org.kde.StatusNotifierWatcher", "/StatusNotifierWatcher", "org.kde.StatusNotifierWatcher", "RegisteredStatusNotifierItems");
	for (const item of items) {
		const slash = item.indexOf("/");
		if (slash < 0) continue;
		const service = item.slice(0, slash).replace(/@$/, "");
		if (busPid(service) !== pid) continue;
		const path = bus("get-property", service, item.slice(slash), "org.kde.StatusNotifierItem", "Menu");
		const layout = busCall(service, path, "com.canonical.dbusmenu", "GetLayout", "iias", "0", "10", "0")[1];
		const quitId = findQuitItem(layout);
		if (quitId !== undefined) return { identity, launch: { command: binary, args: [] }, menu: { service, path, quitId } };
	}
	throw new Error("Installed Quill has no accessible Quit tray action. Quit it manually before starting dev.");
}

function bundleIdentifier(bundle, run = output) {
	return run("/usr/libexec/PlistBuddy", ["-c", "Print :CFBundleIdentifier", join(bundle, "Contents/Info.plist")]);
}

export function macProduction(run = output, identify = processIdentity) {
	for (const line of run("ps", ["-x", "-U", String(process.getuid()), "-o", "pid=,comm="]).split("\n")) {
		const match = line.match(/^\s*(\d+)\s+(.+\.app)\/Contents\/MacOS\/quill$/i);
		if (!match || bundleIdentifier(match[2], run) !== PROD_ID) continue;
		const identity = identify(Number(match[1]));
		if (identity) return { identity, launch: { command: "open", args: ["-g", match[2]] } };
	}
	return null;
}

export function productionPlan() {
	const running = process.platform === "linux" ? linuxProduction() : macProduction();
	if (running) return running;
	if (process.platform === "darwin") {
		const bundle = [join(homedir(), "Applications/Quill.app"), "/Applications/Quill.app"]
			.find((path) => existsSync(path) && bundleIdentifier(path) === PROD_ID);
		return bundle ? { launch: { command: "open", args: ["-g", bundle] } } : null;
	}
	const desktop = [
		join(process.env.XDG_DATA_HOME || join(homedir(), ".local/share"), "applications/quill.desktop"),
		...String(process.env.XDG_DATA_DIRS || "/usr/local/share:/usr/share").split(":").map((dir) => join(dir, "applications/quill.desktop")),
	].find(existsSync);
	if (!desktop) return null;
	output("gio", ["version"]);
	return { launch: { command: "gio", args: ["launch", desktop] } };
}

export function quitProduction(plan) {
	if (!plan?.identity || !sameProcess(plan.identity)) return;
	try {
		if (process.platform === "darwin") {
			output("osascript", ["-e", `tell application id "${PROD_ID}" to quit`]);
		} else {
			if (busPid(plan.menu.service) !== plan.identity.pid) throw new Error("Production Quill changed before handoff; refusing to quit a different process.");
			busCall(plan.menu.service, plan.menu.path, "com.canonical.dbusmenu", "Event", "isvu", String(plan.menu.quitId), "clicked", "i", "0", "0");
		}
	} catch (error) {
		// A successful quit can close the native IPC connection before its reply.
		if ((typeof error.status === "number" || error.code === "ETIMEDOUT") && !sameProcess(plan.identity)) return;
		throw error;
	}
}

export function productionRunning() {
	if (process.platform === "darwin") return macProduction() !== null;
	return busCall("org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus", "ListNames")[0].includes(`${PROD_ID}.SingleInstance`);
}

export function productionEnvironment(environment = process.env) {
	return Object.fromEntries(Object.entries(environment).filter(([key]) =>
		!key.startsWith("TAURI_") && !key.startsWith("QUILL_") && !key.startsWith("CARGO_") && !key.startsWith("DYLD_")
		&& !["APPIMAGE", "APPDIR", "LD_LIBRARY_PATH", "LD_PRELOAD", "NODE_OPTIONS", "NODE_CHANNEL_FD", "NODE_CHANNEL_SERIALIZATION_MODE"].includes(key)));
}

export function launchProduction(plan) {
	const child = spawn(plan.launch.command, plan.launch.args, {
		cwd: homedir(), env: productionEnvironment(), detached: true, stdio: "ignore",
	});
	return new Promise((resolve, reject) => {
		child.once("error", reject);
		child.once("spawn", () => { child.unref(); resolve(); });
	});
}
