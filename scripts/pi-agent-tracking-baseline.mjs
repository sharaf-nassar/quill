#!/usr/bin/env node
// @lat: [[pi-extension-tests#Pi Extension Test Specs#Privacy-Safe Tracking Baseline]]
import assert from "node:assert/strict";
import { once } from "node:events";
import {
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  readdirSync,
  rmSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { createServer } from "node:http";
import { arch, availableParallelism, platform, tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { performance } from "node:perf_hooks";
import { DatabaseSync } from "node:sqlite";
import { fileURLToPath } from "node:url";
import quill from "../src-tauri/pi-integration/quill.ts";

const REPO = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const CHILDREN = 64;
const REQUESTS_PER_CHILD = 16;
const HANDLER_WARMUP = 512;
const HANDLER_SAMPLES = 4096;
const INVENTORY_SAMPLES = 25;
const SESSIONS_SAMPLES = 100;
const BASELINE_START = "<!-- pi-agent-tracking-baseline-report:start -->";
const BASELINE_END = "<!-- pi-agent-tracking-baseline-report:end -->";

function round(value, digits = 3) {
  const scale = 10 ** digits;
  return Math.round(value * scale) / scale;
}

function summarize(samples) {
  assert.ok(samples.length > 0, "metric has samples");
  const sorted = [...samples].sort((a, b) => a - b);
  const middle = Math.floor(sorted.length / 2);
  const median =
    sorted.length % 2 === 0
      ? (sorted[middle - 1] + sorted[middle]) / 2
      : sorted[middle];
  return {
    samples: sorted.length,
    median: round(median, 6),
    p95: round(sorted[Math.ceil(sorted.length * 0.95) - 1], 6),
    max: round(sorted.at(-1), 6),
  };
}

function startEventLoopSampler() {
  const samples = [];
  let previous = performance.now();
  const timer = setInterval(() => {
    const now = performance.now();
    samples.push(Math.max(0, now - previous - 1));
    previous = now;
  }, 1);
  return {
    samples,
    stop() {
      clearInterval(timer);
    },
  };
}

function fakePi() {
  const handlers = new Map();
  return {
    api: {
      registerTool() {},
      on(event, handler) {
        const registered = handlers.get(event) || [];
        registered.push(handler);
        handlers.set(event, registered);
      },
    },
    handlers,
  };
}

function fixtureContext(sessionFile) {
  return {
    cwd: "/fixture",
    mode: "tui",
    ui: { notify() {} },
    sessionManager: {
      getSessionId: () => "fixture",
      getSessionFile: () => sessionFile,
      getHeader: () => ({
        type: "session",
        id: "fixture",
        timestamp: "2026-08-18T00:00:00.000Z",
        cwd: "/fixture",
      }),
    },
  };
}

async function settle() {
  await new Promise((resolvePromise) => setImmediate(resolvePromise));
  await new Promise((resolvePromise) => setImmediate(resolvePromise));
}

async function measureHandler(root) {
  const home = join(root, "extension-home");
  const configPath = join(home, ".config", "quill", "config.json");
  const sessionFile = join(root, "handler-fixture.jsonl");
  mkdirSync(dirname(configPath), { recursive: true });
  writeFileSync(
    configPath,
    JSON.stringify({
      url: "http://127.0.0.1:1",
      context_url: "http://127.0.0.1:1",
      secret: "fixture",
      hostname: "fixture",
    }),
  );
  writeFileSync(sessionFile, "{}\n");

  const oldHome = process.env.HOME;
  const oldFetch = globalThis.fetch;
  let requests = 0;
  process.env.HOME = home;
  globalThis.fetch = async () => {
    requests += 1;
    return { ok: true, status: 202, body: { cancel: async () => {} } };
  };

  try {
    const pi = fakePi();
    quill(pi.api);
    const handler = pi.handlers.get("model_select")?.[0];
    assert.equal(typeof handler, "function", "model_select handler registered");
    const context = fixtureContext(sessionFile);
    for (let index = 0; index < HANDLER_WARMUP; index += 1) {
      handler(
        {
          type: "model_select",
          model: { provider: "fixture", id: `warmup-${index % 2}` },
          source: "set",
        },
        context,
      );
    }
    await settle();
    requests = 0;

    const samples = [];
    for (let index = 0; index < HANDLER_SAMPLES; index += 1) {
      const started = performance.now();
      handler(
        {
          type: "model_select",
          model: { provider: "fixture", id: `model-${index % 2}` },
          source: "set",
        },
        context,
      );
      samples.push(performance.now() - started);
    }
    await settle();
    assert.equal(requests, HANDLER_SAMPLES, "every handler request completed");
    await pi.handlers.get("session_shutdown")[0](
      { type: "session_shutdown", reason: "quit", targetSessionFile: sessionFile },
      context,
    );
    return summarize(samples);
  } finally {
    globalThis.fetch = oldFetch;
    if (oldHome === undefined) delete process.env.HOME;
    else process.env.HOME = oldHome;
  }
}

async function startFixtureServer() {
  let requests = 0;
  const server = createServer(async (request, response) => {
    for await (const _chunk of request) {
      // Drain only. The harness never retains request bodies.
    }
    requests += 1;
    response.writeHead(202, { "Content-Type": "application/json" });
    response.end('{"status":"accepted"}');
  });
  server.listen(0, "127.0.0.1");
  await once(server, "listening");
  const address = server.address();
  assert.ok(address && typeof address === "object");
  return {
    url: `http://127.0.0.1:${address.port}/synthetic`,
    requests: () => requests,
    async close() {
      server.close();
      await once(server, "close");
    },
  };
}

function startRssSampler() {
  const start = process.memoryUsage().rss;
  const samples = [0];
  const sample = () => samples.push(Math.max(0, process.memoryUsage().rss - start));
  const timer = setInterval(sample, 5);
  return {
    start,
    samples,
    sample,
    stop() {
      clearInterval(timer);
      sample();
    },
  };
}

async function runFleet(root) {
  const sourceRoot = join(root, "sources");
  mkdirSync(sourceRoot, { recursive: true });
  const server = await startFixtureServer();
  const rss = startRssSampler();
  const started = performance.now();
  let failures = 0;
  const fixtureLine = `${JSON.stringify({ type: "custom", customType: "quill-tracking", data: { schema: 1, event: "activity" } })}\n`;

  try {
    await Promise.all(
      Array.from({ length: CHILDREN }, async (_, childIndex) => {
        const source = join(sourceRoot, `${String(childIndex).padStart(2, "0")}.jsonl`);
        writeFileSync(source, fixtureLine.repeat(16), { mode: 0o600 });
        for (let requestIndex = 0; requestIndex < REQUESTS_PER_CHILD; requestIndex += 1) {
          try {
            const response = await fetch(server.url, {
              method: "POST",
              headers: { "Content-Type": "application/json" },
              body: '{"event":"activity"}',
            });
            if (!response.ok) failures += 1;
          } catch {
            failures += 1;
          }
        }
      }),
    );
  } finally {
    rss.stop();
    await server.close();
  }

  const durationMs = performance.now() - started;
  const files = readdirSync(sourceRoot, { withFileTypes: true }).filter((entry) => entry.isFile());
  const sessionFileGrowthBytes = files.reduce(
    (total, entry) => total + statSync(join(sourceRoot, entry.name)).size,
    0,
  );
  return {
    sourceRoot,
    files: files.map((entry) => entry.name),
    metrics: {
      children: CHILDREN,
      requests: server.requests(),
      failures,
      duration_ms: round(durationMs),
      requests_per_second: round(server.requests() / (durationMs / 1000)),
      rss_start_bytes: rss.start,
      rss_growth_bytes: summarize(rss.samples),
      session_files: files.length,
      session_file_growth_bytes: sessionFileGrowthBytes,
    },
  };
}

function measureInventory(sourceRoot) {
  const samples = [];
  let changedSources = 0;
  for (let sample = 0; sample < INVENTORY_SAMPLES; sample += 1) {
    const started = performance.now();
    changedSources = readdirSync(sourceRoot, { withFileTypes: true }).reduce(
      (count, entry) =>
        entry.isFile() && statSync(join(sourceRoot, entry.name)).size > 0 ? count + 1 : count,
      0,
    );
    samples.push(performance.now() - started);
  }
  return { changed_sources: changedSources, latency_ms: summarize(samples) };
}

function fileSize(path) {
  return existsSync(path) ? statSync(path).size : 0;
}

function databaseSizes(path) {
  return { db: fileSize(path), wal: fileSize(`${path}-wal`) };
}

function reconcileAndMeasureSessions(root, sourceRoot, files) {
  const databasePath = join(root, "baseline.db");
  const database = new DatabaseSync(databasePath);
  database.exec(`
    PRAGMA journal_mode = WAL;
    PRAGMA synchronous = NORMAL;
    PRAGMA wal_autocheckpoint = 0;
    CREATE TABLE sources (
      source_index INTEGER PRIMARY KEY,
      byte_count INTEGER NOT NULL,
      event_count INTEGER NOT NULL,
      activity_rank INTEGER NOT NULL
    );
  `);
  database.exec("PRAGMA wal_checkpoint(TRUNCATE)");
  const before = databaseSizes(databasePath);
  const sourceLatency = [];
  const backlogAge = [];
  const reconcileStarted = performance.now();
  let failures = 0;
  let remaining = files.length;
  const insert = database.prepare(
    "INSERT INTO sources (source_index, byte_count, event_count, activity_rank) VALUES (?, ?, ?, ?)",
  );

  for (const [sourceIndex, name] of files.entries()) {
    const started = performance.now();
    try {
      const payload = readFileSync(join(sourceRoot, name), "utf8");
      const eventCount = payload.split("\n").filter(Boolean).length;
      database.exec("BEGIN IMMEDIATE");
      insert.run(sourceIndex, Buffer.byteLength(payload), eventCount, files.length - sourceIndex);
      database.exec("COMMIT");
    } catch {
      failures += 1;
      try {
        database.exec("ROLLBACK");
      } catch {
        // No active transaction.
      }
    }
    remaining -= 1;
    sourceLatency.push(performance.now() - started);
    backlogAge.push(performance.now() - reconcileStarted);
  }
  const reconcileDurationMs = performance.now() - reconcileStarted;

  const readLatency = [];
  const overlayLatency = [];
  const combinedLatency = [];
  const overlay = new Map(
    Array.from({ length: CHILDREN }, (_, index) => [index, { active_children: index % 4 }]),
  );
  const read = database.prepare(
    "SELECT source_index, byte_count, event_count FROM sources ORDER BY activity_rank DESC LIMIT 64",
  );
  let rowCount = 0;
  for (let sample = 0; sample < SESSIONS_SAMPLES; sample += 1) {
    const combinedStarted = performance.now();
    const readStarted = performance.now();
    const rows = read.all();
    readLatency.push(performance.now() - readStarted);
    const overlayStarted = performance.now();
    const projected = rows.map((row) => ({ ...row, ...overlay.get(Number(row.source_index)) }));
    overlayLatency.push(performance.now() - overlayStarted);
    combinedLatency.push(performance.now() - combinedStarted);
    rowCount = projected.length;
  }
  const after = databaseSizes(databasePath);
  database.close();

  return {
    reconciliation: {
      initial_backlog: files.length,
      peak_backlog: files.length,
      remaining_backlog: remaining,
      failures,
      duration_ms: round(reconcileDurationMs),
      sources_per_second: round(files.length / (reconcileDurationMs / 1000)),
      source_latency_ms: summarize(sourceLatency),
      backlog_age_ms: summarize(backlogAge),
    },
    sessions: {
      rows: rowCount,
      read_ms: summarize(readLatency),
      overlay_ms: summarize(overlayLatency),
      read_overlay_ms: summarize(combinedLatency),
    },
    database_growth: {
      db_bytes: after.db - before.db,
      wal_bytes: after.wal - before.wal,
      total_bytes: after.db + after.wal - before.db - before.wal,
    },
  };
}

function comparisonBaseline() {
  const spec = readFileSync(join(REPO, "specs", "028-pi-agent-tracking-hardening.md"), "utf8");
  const start = spec.indexOf(BASELINE_START);
  const end = spec.indexOf(BASELINE_END);
  assert.ok(start !== -1 && end > start, "recorded baseline evidence exists");
  const fenced = spec.slice(start + BASELINE_START.length, end);
  const match = fenced.match(/```json\s*([\s\S]*?)\s*```/);
  assert.ok(match, "recorded baseline evidence is JSON");
  return JSON.parse(match[1]);
}

function addCheck(checks, name, passed, evidence) {
  checks.push({ name, status: passed ? "pass" : "fail", evidence });
}

function applyChecks(report, compare) {
  const checks = [];
  addCheck(
    checks,
    "handler maximum <=10 ms",
    report.handler_ms.max <= 10,
    `${report.handler_ms.max} ms`,
  );
  addCheck(
    checks,
    "Sessions read/overlay maximum <=300 ms",
    report.sessions.read_overlay_ms.max <= 300,
    `${report.sessions.read_overlay_ms.max} ms`,
  );
  addCheck(
    checks,
    "64 synthetic children completed without failures",
    report.fleet.children === CHILDREN &&
      report.fleet.failures === 0 &&
      report.fleet.requests === CHILDREN * REQUESTS_PER_CHILD,
    `${report.fleet.requests} requests; ${report.fleet.failures} failures`,
  );
  addCheck(
    checks,
    "inventory and reconciliation converged",
    report.inventory.changed_sources === CHILDREN &&
      report.reconciliation.remaining_backlog === 0 &&
      report.reconciliation.failures === 0,
    `${report.inventory.changed_sources} sources; ${report.reconciliation.remaining_backlog} remaining`,
  );

  if (compare) {
    const baseline = comparisonBaseline();
    for (const [name, current, previous] of [
      ["handler p95 regression <=10%", report.handler_ms.p95, baseline.handler_ms.p95],
      [
        "event-loop p95 regression <=10%",
        report.event_loop_delay_ms.p95,
        baseline.event_loop_delay_ms.p95,
      ],
      [
        "RSS p95 regression <=10%",
        report.fleet.rss_growth_bytes.p95,
        baseline.fleet.rss_growth_bytes.p95,
      ],
      [
        "reconciliation p95 regression <=10%",
        report.reconciliation.source_latency_ms.p95,
        baseline.reconciliation.source_latency_ms.p95,
      ],
      [
        "Sessions overlay p95 regression <=10%",
        report.sessions.read_overlay_ms.p95,
        baseline.sessions.read_overlay_ms.p95,
      ],
    ]) {
      const limit = previous * 1.1;
      addCheck(checks, name, current <= limit, `${current} <= ${round(limit)}`);
    }
  }
  report.checks = checks;
  report.verdict = checks.every((check) => check.status === "pass") ? "pass" : "fail";
}

function assertPrivacy(report, root) {
  const output = JSON.stringify(report);
  for (const forbidden of [
    root,
    "session_id",
    "hostname",
    "jsonl_path",
    "prompt",
    "message_body",
    "fixture-secret",
  ]) {
    assert.ok(!output.includes(forbidden), `report excludes ${forbidden}`);
  }
}

async function main() {
  const compare = process.argv.slice(2).includes("--compare");
  const root = mkdtempSync(join(tmpdir(), "quill-pi-baseline-"));
  const eventLoop = startEventLoopSampler();
  try {
    const handler = await measureHandler(root);
    const fleet = await runFleet(root);
    const inventory = measureInventory(fleet.sourceRoot);
    const measured = reconcileAndMeasureSessions(root, fleet.sourceRoot, fleet.files);
    await new Promise((resolvePromise) => setTimeout(resolvePromise, 20));
    eventLoop.stop();

    const report = {
      schema: 1,
      command: `node scripts/pi-agent-tracking-baseline.mjs${compare ? " --compare" : ""}`,
      profile: {
        kind: "isolated synthetic current-runtime shape",
        node: process.version,
        platform: platform(),
        arch: arch(),
        logical_cpus: availableParallelism(),
        children: CHILDREN,
        requests_per_child: REQUESTS_PER_CHILD,
        handler_event: "model_select",
      },
      handler_ms: handler,
      event_loop_delay_ms: summarize(eventLoop.samples),
      fleet: fleet.metrics,
      inventory,
      ...measured,
      privacy: {
        retained_request_bodies: 0,
        identifying_fields: 0,
        fixture_cleanup: "automatic",
        live_quill_window_touched: false,
        runtime_source_touched: false,
      },
    };
    applyChecks(report, compare);
    assertPrivacy(report, root);
    console.log(JSON.stringify(report, null, 2));
    if (report.verdict !== "pass") process.exitCode = 1;
  } finally {
    eventLoop.stop();
    rmSync(root, { recursive: true, force: true });
  }
}

await main();
