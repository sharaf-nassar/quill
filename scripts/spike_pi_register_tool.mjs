#!/usr/bin/env node

import { mkdtempSync, readFileSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { spawnSync } from "node:child_process";

const pi = process.env.PI_BIN || "pi";
const versionRun = spawnSync(pi, ["--version"], { encoding: "utf8" });
if (versionRun.status !== 0) throw new Error(versionRun.stderr || `${pi} not found`);

const version = versionRun.stdout.trim();
const [major, minor] = version.split(".").map(Number);
if (!Number.isInteger(major) || !Number.isInteger(minor) || (major === 0 && minor < 84)) {
  throw new Error(`pi >=0.84.0 required, found ${version}`);
}

const piBin = spawnSync("which", [pi], { encoding: "utf8" }).stdout.trim();
const piRoot = dirname(dirname(realpathSync(piBin)));
const root = mkdtempSync(join(tmpdir(), "quill-pi-register-tool-"));

const candidates = {
  "plain-object": `const parameters = {
  type: "object",
  properties: { query: { type: "string" } },
  required: ["query"],
  additionalProperties: false,
};`,
  "bare-typebox": `import { Type } from "typebox";
const parameters = Type.Object({ query: Type.String() });`,
  "create-require": `import { createRequire } from "node:module";
const { Type } = createRequire(${JSON.stringify(join(piRoot, "package.json"))})("typebox");
const parameters = Type.Object({ query: Type.String() });`,
};

try {
  console.log(`pi ${version}`);
  for (const [name, schema] of Object.entries(candidates)) {
    const extension = join(root, `${name}.ts`);
    const marker = join(root, `${name}.passed`);
    writeFileSync(extension, `import { writeFileSync } from "node:fs";
${schema}
export default function (pi) {
  pi.registerTool({
    name: "quill_schema_spike",
    label: "Quill schema spike",
    description: "Verifies the registerTool parameter shape.",
    parameters,
    async execute() { return { content: [{ type: "text", text: "ok" }] }; },
  });
  writeFileSync(process.env.QUILL_SPIKE_MARKER, "registered");
}
`);

    const run = spawnSync(
      pi,
      ["--offline", "--mode", "rpc", "--no-session", "--no-context-files", "--no-skills", "--no-extensions", "--extension", extension],
      {
        encoding: "utf8",
        input: '{"type":"get_state","id":"probe"}\n',
        env: {
          ...process.env,
          PI_CODING_AGENT_DIR: join(root, "config"),
          PI_CODING_AGENT_SESSION_DIR: join(root, "sessions"),
          QUILL_SPIKE_MARKER: marker,
        },
        timeout: 15000,
      },
    );
    if (run.status !== 0 || readFileSync(marker, "utf8") !== "registered" || !run.stdout.includes('"success":true')) {
      throw new Error(`${name} failed: ${run.stderr || run.stdout}`);
    }
    console.log(`PASS ${name}`);
  }
  console.log("CHOSEN plain-object");
  console.log("EXTRA_FILES none");
} finally {
  rmSync(root, { recursive: true, force: true });
}
