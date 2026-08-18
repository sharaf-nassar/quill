import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";
import test from "node:test";
import { EXTENSION_VERSION } from "../src-tauri/pi-integration/quill.ts";

const packageDir = new URL("../src-tauri/pi-integration/", import.meta.url);
const manifest = JSON.parse(readFileSync(new URL("package.json", packageDir), "utf8"));

// @lat: [[pi-package-tests#Pi Package Test Specs#Registry artifact]]
test("Pi package has one dependency-free extension export", () => {
  assert.equal(manifest.name, "@sharaf-nassar/quill-pi");
  assert.equal(manifest.version, "0.2.0");
  assert.equal(manifest.version, EXTENSION_VERSION);
  assert.equal(manifest.exports["."], "./quill.ts");
  assert.deepEqual(manifest.pi.extensions, ["./quill.ts"]);
  assert.equal(manifest.engines.node, ">=22.19.0");
  assert.equal(manifest.peerDependencies["@earendil-works/pi-coding-agent"], ">=0.84.0 <1");
  assert.deepEqual(manifest.dependencies, undefined);

  const [packed] = JSON.parse(
    execFileSync("npm", ["pack", "--dry-run", "--json", packageDir.pathname], {
      encoding: "utf8",
    }),
  );
  assert.equal(packed.name, manifest.name);
  assert.equal(packed.version, manifest.version);
  assert.deepEqual(
    packed.files.map(({ path }) => path).sort(),
    ["LICENSE", "README.md", "package.json", "quill.ts"],
  );
});

// @lat: [[pi-package-tests#Pi Package Test Specs#Desktop-first publication]]
test("Pi package publication waits for the matching desktop release", () => {
  const workflow = readFileSync(
    new URL("../.github/workflows/publish-pi-extension.yml", import.meta.url),
    "utf8",
  );
  const desktopGate = workflow.indexOf(
    "Verify matching desktop release is available",
  );
  const buildStage = workflow.indexOf(
    "Stage exact desktop build in reporter source",
  );
  const dryRun = workflow.indexOf("npm publish --dry-run --provenance");
  const publish = workflow.indexOf("npm publish --provenance --access public");

  assert.ok(desktopGate > 0);
  assert.ok(buildStage > desktopGate);
  assert.ok(dryRun > buildStage);
  assert.ok(publish > dryRun);
  assert.match(workflow, /gh release view "v\$\{PI_VERSION\}"/);
  assert.match(workflow, /"0\.0\.0-injected-by-ci", process\.env\.PI_VERSION/);
});
