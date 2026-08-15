import assert from "node:assert/strict";
import { existsSync, mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { spawnSync } from "node:child_process";
import test from "node:test";

const chrome = [
  process.env.CHROME_BIN,
  "/usr/bin/google-chrome",
  "/usr/bin/google-chrome-stable",
  "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
].find((candidate) => candidate && existsSync(candidate));

// @lat: [[confirm-dialog-tests#Confirmation Dialog Tests#Manage Window Centering]]
test(
  "confirmation dialog restores native centering after the global reset",
  { skip: chrome ? false : "Chrome is not installed" },
  () => {
    const css = readFileSync("src/styles/index.css", "utf8");
    const html = `<!doctype html>
      <style>${css}</style>
      <dialog class="confirm-dialog" aria-label="Enable Claude?">Confirm</dialog>
      <script>
        const dialog = document.querySelector("dialog");
        dialog.showModal();
        const rule = [...document.styleSheets[0].cssRules].find(
          (candidate) => candidate.selectorText === ".confirm-dialog",
        );
        document.body.dataset.dialogMargin = rule?.style.margin ?? "";
      <\/script>`;
    const profile = mkdtempSync(join(tmpdir(), "quill-dialog-test."));

    try {
      const result = spawnSync(
        chrome,
        [
          "--headless=new",
          "--disable-gpu",
          "--no-first-run",
          `--user-data-dir=${profile}`,
          "--window-size=975,686",
          "--dump-dom",
          `data:text/html;base64,${Buffer.from(html).toString("base64")}`,
        ],
        { encoding: "utf8", maxBuffer: 4 * 1024 * 1024 },
      );
      assert.equal(result.status, 0, result.stderr);

      const margin = result.stdout.match(/data-dialog-margin="([^"]*)"/)?.[1];
      assert.equal(margin, "auto");
    } finally {
      rmSync(profile, { recursive: true, force: true });
    }
  },
);
