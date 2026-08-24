import assert from "node:assert/strict";
import { mkdtempSync, readdirSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, relative, sep } from "node:path";
import test from "node:test";
import { build } from "vite";

const DOCUMENT = "web.html";
// The two desktop-only surfaces `src/main.tsx` imports and the web entry must
// not: their chunks are named after these modules when Rollup emits them.
const DESKTOP_ONLY = /manage|release-?notes/i;

function bundledFiles(root, directory = root) {
	return readdirSync(directory, { withFileTypes: true }).flatMap((entry) => {
		const path = join(directory, entry.name);
		return entry.isDirectory()
			? bundledFiles(root, path)
			: [relative(root, path).split(sep).join("/")];
	});
}

// Walk the entry's imports, dynamic imports, styles, and referenced assets.
function transitiveGraph(manifest) {
	const chunks = new Set();
	const files = new Set([DOCUMENT]);
	const pending = [DOCUMENT];

	while (pending.length > 0) {
		const key = pending.pop();
		if (chunks.has(key)) continue;
		chunks.add(key);
		const chunk = manifest[key];
		assert.ok(chunk, `${key} is reachable from the web entry but absent from the manifest`);
		for (const file of [chunk.file, ...(chunk.css ?? []), ...(chunk.assets ?? [])]) {
			files.add(file);
		}
		pending.push(...(chunk.imports ?? []), ...(chunk.dynamicImports ?? []));
	}
	return { chunks, files };
}

// @lat: [[web-ui-server-tests#Web UI Server Test Specs#Only the isolated monitor bundle is servable]]
test("the web bundle's served asset set is exactly the monitor entry's graph", async () => {
	const outDir = mkdtempSync(join(tmpdir(), "quill-web-bundle-"));
	try {
		await build({ mode: "web", logLevel: "silent", build: { outDir, emptyOutDir: true } });
		const manifest = JSON.parse(readFileSync(join(outDir, ".vite/manifest.json"), "utf8"));

		assert.deepEqual(
			Object.values(manifest)
				.filter((chunk) => chunk.isEntry)
				.map((chunk) => chunk.src),
			[DOCUMENT],
		);

		const { chunks, files } = transitiveGraph(manifest);
		for (const key of chunks) {
			const chunk = manifest[key];
			for (const name of [key, chunk.src ?? "", chunk.name ?? "", chunk.file]) {
				assert.doesNotMatch(name, DESKTOP_ONLY, `${key} reaches a desktop-only module`);
			}
		}

		// The listener embeds this directory and routes `/` plus `/assets/*`, so
		// any file here outside the entry's graph would be a servable chunk the
		// monitor surface never asked for. `.vite/` holds this manifest and is
		// not routed.
		const served = bundledFiles(outDir).filter((file) => !file.startsWith(".vite/"));
		assert.deepEqual(served.sort(), [...files].sort());
	} finally {
		rmSync(outDir, { recursive: true, force: true });
	}
});
