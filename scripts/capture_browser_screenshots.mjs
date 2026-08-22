#!/usr/bin/env node
import { spawn } from "node:child_process";
import { copyFileSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { resolve } from "node:path";

const REPO = resolve(import.meta.dirname, "..");
const OUTDIR = resolve(process.argv[2] ?? "marketing-site/assets/screenshots");
const APP_URL = "http://127.0.0.1:8181";
const DEBUG_URL = "http://127.0.0.1:9222";
const CHROME = process.env.CHROME_BIN ?? "chromium";
const children = [];

const sleep = (ms) => new Promise((done) => setTimeout(done, ms));

function start(command, args, options = {}) {
  const child = spawn(command, args, {
    detached: true,
    stdio: ["ignore", "pipe", "pipe"],
    ...options,
  });
  children.push(child);
  child.stdout?.on("data", (chunk) => process.stdout.write(chunk));
  child.stderr?.on("data", (chunk) => process.stderr.write(chunk));
  return child;
}

function stop(child) {
  if (child.exitCode !== null || !child.pid) return;
  try {
    process.kill(-child.pid, "SIGKILL");
  } catch {
    child.kill("SIGKILL");
  }
}

async function waitForUrl(url, timeoutMs = 60_000) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    try {
      const response = await fetch(url);
      if (response.ok) return response;
    } catch {
      // Service is still starting.
    }
    await sleep(100);
  }
  throw new Error(`Timed out waiting for ${url}`);
}

class CdpClient {
  constructor(url) {
    this.nextId = 1;
    this.pending = new Map();
    this.listeners = new Map();
    this.socket = new WebSocket(url);
  }

  async open() {
    await new Promise((resolveOpen, rejectOpen) => {
      this.socket.addEventListener("open", resolveOpen, { once: true });
      this.socket.addEventListener("error", rejectOpen, { once: true });
    });
    this.socket.addEventListener("message", (event) => {
      const message = JSON.parse(event.data);
      if (message.id) {
        const pending = this.pending.get(message.id);
        if (!pending) return;
        this.pending.delete(message.id);
        if (message.error) pending.reject(new Error(message.error.message));
        else pending.resolve(message.result);
        return;
      }
      const listeners = this.listeners.get(message.method) ?? [];
      this.listeners.delete(message.method);
      for (const resolveEvent of listeners) resolveEvent(message.params);
    });
  }

  send(method, params = {}) {
    const id = this.nextId++;
    return new Promise((resolveSend, rejectSend) => {
      this.pending.set(id, { resolve: resolveSend, reject: rejectSend });
      this.socket.send(JSON.stringify({ id, method, params }));
    });
  }

  once(method) {
    return new Promise((resolveEvent) => {
      const listeners = this.listeners.get(method) ?? [];
      listeners.push(resolveEvent);
      this.listeners.set(method, listeners);
    });
  }

  close() {
    this.socket.close();
  }
}

async function evaluate(client, expression, awaitPromise = false) {
  const result = await client.send("Runtime.evaluate", {
    expression,
    awaitPromise,
    returnByValue: true,
  });
  if (result.exceptionDetails) {
    throw new Error(result.exceptionDetails.text ?? `Evaluation failed: ${expression}`);
  }
  return result.result.value;
}

async function waitFor(client, expression, timeoutMs = 15_000) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (await evaluate(client, expression)) return;
    await sleep(100);
  }
  throw new Error(`Timed out waiting for browser state: ${expression}`);
}

async function setViewport(client, width, height) {
  await client.send("Emulation.setDeviceMetricsOverride", {
    width,
    height,
    deviceScaleFactor: 2,
    mobile: false,
    screenWidth: width,
    screenHeight: height,
  });
}

async function navigate(client, path, readyExpression) {
  const loaded = client.once("Page.loadEventFired");
  await client.send("Page.navigate", { url: `${APP_URL}${path}` });
  await loaded;
  await waitFor(client, readyExpression);
  await sleep(500);
}

async function clickText(client, selector, text) {
  const clicked = await evaluate(client, `(() => {
    const element = [...document.querySelectorAll(${JSON.stringify(selector)})]
      .find((candidate) => candidate.textContent?.trim().startsWith(${JSON.stringify(text)}));
    if (!element) return false;
    element.click();
    return true;
  })()`);
  if (!clicked) throw new Error(`Could not click ${text} in ${selector}`);
  await sleep(350);
}

async function setInput(client, selector, value) {
  const changed = await evaluate(client, `(() => {
    const input = document.querySelector(${JSON.stringify(selector)});
    if (!(input instanceof HTMLInputElement)) return false;
    const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")?.set;
    setter?.call(input, ${JSON.stringify(value)});
    input.dispatchEvent(new Event("input", { bubbles: true }));
    return true;
  })()`);
  if (!changed) throw new Error(`Could not set input ${selector}`);
  await sleep(700);
}

async function screenshot(client, filename) {
  const { data } = await client.send("Page.captureScreenshot", {
    format: "png",
    fromSurface: true,
    captureBeyondViewport: false,
  });
  writeFileSync(resolve(OUTDIR, filename), Buffer.from(data, "base64"));
  console.log(`Saved: ${filename}`);
}

async function captureWidget(client) {
  await setViewport(client, 480, 800);
  await navigate(client, "/?screenshot=marketing", "Boolean(document.querySelector('.wg-shell'))");
  await clickText(client, ".wg-toggle", "6H");
  await clickText(client, '[aria-label="Breakdown mode"] button', "Sessions");
  await screenshot(client, "hero.png");
  copyFileSync(resolve(OUTDIR, "hero.png"), resolve(OUTDIR, "live.png"));

  await clickText(client, ".wg-viewdd-btn", "Usage");
  await clickText(client, ".wg-viewdd-option", "Models");
  await clickText(client, ".wg-toggle", "7D");
  await screenshot(client, "models.png");

  await clickText(client, ".wg-viewdd-btn", "Models");
  await clickText(client, ".wg-viewdd-option", "Context");
  await clickText(client, ".wg-toggle", "6H");
  await screenshot(client, "analytics-context.png");
}

function pngDimensions(path) {
  const bytes = readFileSync(path);
  if (bytes.toString("ascii", 1, 4) !== "PNG") throw new Error(`Not a PNG: ${path}`);
  return [bytes.readUInt32BE(16), bytes.readUInt32BE(20)];
}

function validateOutput() {
  for (const file of ["hero.png", "live.png", "models.png", "analytics-context.png"]) {
    const dimensions = pngDimensions(resolve(OUTDIR, file));
    if (dimensions[0] !== 960 || dimensions[1] !== 1600) {
      throw new Error(`Unexpected widget dimensions for ${file}: ${dimensions.join("x")}`);
    }
  }
  for (const file of ["sessions.png", "learning.png", "memory.png", "settings.png", "brevity.png"]) {
    const dimensions = pngDimensions(resolve(OUTDIR, file));
    if (dimensions[0] !== 1920 || dimensions[1] !== 1360) {
      throw new Error(`Unexpected Tools dimensions for ${file}: ${dimensions.join("x")}`);
    }
  }
  if (!readFileSync(resolve(OUTDIR, "hero.png")).equals(readFileSync(resolve(OUTDIR, "live.png")))) {
    throw new Error("live.png must be an exact copy of hero.png");
  }
}

async function captureTools(client) {
  await setViewport(client, 960, 680);
  await navigate(
    client,
    "/?view=manage&section=sessions&screenshot=marketing",
    "Boolean(document.querySelector('.sessions-search-input'))",
  );
  await setInput(client, ".sessions-search-input", "parser");
  await waitFor(client, "document.querySelectorAll('.sessions-result-card').length > 0");
  await evaluate(client, "document.querySelector('.sessions-result-card')?.click()");
  await sleep(500);
  await screenshot(client, "sessions.png");

  await clickText(client, ".manage-rail-item", "Learning");
  await waitFor(client, "Boolean(document.querySelector('.learning-window'))");
  await screenshot(client, "learning.png");

  await clickText(client, ".learning-cog-btn", "Memories");
  await waitFor(client, "document.body.textContent.includes('All Projects (4)')");
  await screenshot(client, "memory.png");

  await clickText(client, ".manage-rail-item", "Settings");
  await waitFor(client, "Boolean(document.querySelector('.settings-window'))");
  await clickText(client, ".settings-tab", "Integrations");
  await screenshot(client, "settings.png");

  await clickText(client, ".settings-tab", "Context");
  await evaluate(client, `(() => {
    const content = document.querySelector('.settings-content');
    if (content) content.scrollTop = content.scrollHeight;
  })()`);
  await sleep(350);
  await screenshot(client, "brevity.png");
}

async function main() {
  mkdirSync(OUTDIR, { recursive: true });
  const vite = start("npm", ["run", "dev", "--", "--host", "127.0.0.1"], {
    cwd: REPO,
    env: { ...process.env, BROWSER: "none" },
  });
  const chrome = start(CHROME, [
    "--headless=new",
    "--no-sandbox",
    "--disable-gpu",
    "--disable-background-networking",
    "--disable-component-update",
    "--disable-dev-shm-usage",
    "--hide-scrollbars",
    "--remote-debugging-address=127.0.0.1",
    "--remote-debugging-port=9222",
    "--user-data-dir=/tmp/quill-screenshot-chrome",
    "about:blank",
  ], { stdio: "ignore" });

  try {
    await waitForUrl(APP_URL);
    const targets = await (await waitForUrl(`${DEBUG_URL}/json`)).json();
    const page = targets.find((target) => target.type === "page");
    if (!page?.webSocketDebuggerUrl) throw new Error("Chrome page target not found");
    const client = new CdpClient(page.webSocketDebuggerUrl);
    await client.open();
    await client.send("Page.enable");
    await client.send("Runtime.enable");
    await client.send("Page.addScriptToEvaluateOnNewDocument", {
      source: `(() => {
        const fixed = Date.parse("2026-08-22T12:00:00.000Z");
        const NativeDate = Date;
        class FixedDate extends NativeDate {
          constructor(...args) { super(...(args.length ? args : [fixed])); }
          static now() { return fixed; }
        }
        window.Date = FixedDate;
      })();`,
    });
    await captureWidget(client);
    await captureTools(client);
    validateOutput();
    client.close();
  } finally {
    stop(chrome);
    stop(vite);
  }
}

main().then(
  () => process.exit(0),
  (error) => {
    console.error(error);
    for (const child of children) stop(child);
    process.exit(1);
  },
);
