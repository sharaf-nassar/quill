import { readdirSync, readFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const SENTRY_DEBUG_ID_INJECTION =
	/_sentryDebugIdIdentifier\s*=\s*["']sentry-dbid-[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}["']/i;

export function verifySentryOutput(
	directory = "dist",
	expectDebugIds = Boolean(process.env.SENTRY_AUTH_TOKEN),
) {
	const files = readdirSync(directory, { recursive: true }).map((entry) =>
		join(directory, entry),
	);
	const maps = files.filter((file) => file.endsWith(".map"));
	if (maps.length) throw new Error(`Source map remained in build: ${maps[0]}`);
	if (!expectDebugIds) return;

	const scripts = files.filter((file) => /\.(?:c|m)?js$/.test(file));
	if (!scripts.length) throw new Error("Sentry build emitted no JavaScript");
	const missing = scripts.find(
		(file) => !SENTRY_DEBUG_ID_INJECTION.test(readFileSync(file, "utf8")),
	);
	if (missing) throw new Error(`Sentry debug ID missing from build: ${missing}`);
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
	verifySentryOutput(process.argv[2]);
}
