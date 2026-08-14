import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const docs = Object.fromEntries(
	await Promise.all(
		["PRODUCT.md", "README.md", "marketing-site/index.html", "release_notes.md"].map(
			async (path) => [path, await readFile(new URL(`../${path}`, import.meta.url), "utf8")],
		),
	),
);

test("public docs describe Pi without claiming Limits support", () => {
	for (const path of ["PRODUCT.md", "README.md", "marketing-site/index.html"])
		assert.match(docs[path], /Claude Code, Codex(?:,| &amp;) Pi/);

	assert.match(docs["README.md"], /Pi does not use MCP or external hook commands/);
	assert.match(docs["README.md"], /Pi does not use MCP[\s\S]*no LIMITS row/);
	assert.match(
		docs["marketing-site/index.html"],
		/Across Claude Code, Codex &amp; Pi[\s\S]{0,300}Pi contributes session and model analytics plus local tools, but has no LIMITS row\./,
	);
});

test("README distinguishes hook HTTP telemetry from Pi transcript ingestion", () => {
	const readme = docs["README.md"];
	const tokenTracking = readme.match(/### Token tracking\n\n([\s\S]*?)\n\n### Code stats/)?.[1] ?? "";
	assert.match(
		tokenTracking,
		/Claude Code and Codex hook telemetry uses the authenticated local HTTP server/,
	);
	assert.match(tokenTracking, /Pi usage is ingested watcher-side from `AssistantMessage` transcript entries/);
	assert.doesNotMatch(tokenTracking, /Pi[^\n]*(?:HTTP|hook)|(?:HTTP|hook)[^\n]*Pi/i);
});

test("public docs distinguish MCP tools from the Pi extension subset", () => {
	const readme = docs["README.md"];
	const contextTools = readme.match(/### Working context preservation\n\n([\s\S]*?)\n\n### MCP server/)?.[1] ?? "";
	assert.match(
		contextTools,
		/Claude Code and Codex receive `quill_execute_file` and `quill_batch_execute` through MCP/,
	);
	assert.match(
		contextTools,
		/Pi registers `quill_index_context`, `quill_fetch_and_index`, `quill_execute`, `quill_search_context`, `quill_get_context_source`, `quill_context_stats`, and `quill_purge_context`, plus `quill_search_history`/,
	);
	assert.doesNotMatch(
		contextTools,
		/Pi (?:registers|receives|installs)[^\n]*(?:same|full|all)[^\n]*tools/i,
	);
	assert.doesNotMatch(
		contextTools,
		/Pi (?:registers|receives|installs)[^\n]*(?:quill_execute_file|quill_batch_execute)/,
	);
	assert.match(
		docs["marketing-site/index.html"],
		/Claude Code and Codex receive the full tool set through MCP;\s+Pi registers the listed core tools through its managed extension/,
	);
});

test("release notes disclose the managed executable and downgrade", () => {
	const notes = docs["release_notes.md"];
	assert.match(notes, /`quill\.ts` at\s+`~\/\.pi\/agent\/extensions\/quill\.ts`/);
	assert.match(notes, /`\$PI_CODING_AGENT_DIR\/extensions\/quill\.ts` when configured/);
	assert.match(notes, /repairs and\s+self-updates that file/);
	assert.match(notes, /Disabling Pi removes it while preserving every other Pi\s+file/);
	assert.match(notes, /older Quill builds that do not understand provider `pi` drop\s+its saved enablement entry/);
	assert.match(notes, /If you downgrade and later return to a current\s+build, re-enable Pi/);
});
