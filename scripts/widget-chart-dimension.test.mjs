import assert from "node:assert/strict";
import test from "node:test";
import {
  readStoredWidgetChartDimension,
  resolveStoredWidgetChartDimension,
  storeWidgetChartDimension,
} from "../src/components/widget/rangePreference.ts";
import { chartSeriesFor } from "../src/components/widget/chartDimensions.ts";

const providerLabel = (provider) => provider.toUpperCase();
const providerHue = (provider) => `var(--provider-${provider})`;

class MemoryStorage {
  values = new Map();

  getItem(key) {
    return this.values.get(key) ?? null;
  }

  setItem(key, value) {
    this.values.set(key, value);
  }
}

// @lat: [[widget-usage-tests#Widget Usage Tests#Chart dimension preference]]
test("widget chart dimension defaults to Models and persists a valid choice", () => {
  const storage = new MemoryStorage();
  assert.equal(resolveStoredWidgetChartDimension(null), "models");
  assert.equal(resolveStoredWidgetChartDimension("invalid"), "models");
  assert.equal(readStoredWidgetChartDimension(storage), "models");

  storeWidgetChartDimension("llm", storage);
  assert.equal(readStoredWidgetChartDimension(storage), "llm");
});

// @lat: [[widget-usage-tests#Widget Usage Tests#Chart group preservation]]
test("chart dimensions preserve every model-evidence token", () => {
  const activity = {
    bucketSeconds: 300,
    bucketStarts: ["2026-08-01T12:00:00Z", "2026-08-01T12:05:00Z"],
    series: [
      {
        identity: { provider: "pi", modelId: "cliproxyapi/claude-opus-5" },
        sessionsPerBucket: [1, 0],
        tokensPerBucket: [100, 0],
      },
      {
        identity: { provider: "pi", modelId: "cliproxyapi/gpt-5.6-sol" },
        sessionsPerBucket: [0, 1],
        tokensPerBucket: [0, 200],
      },
    ],
    unattributedSeries: [{ provider: "pi", tokensPerBucket: [7, 0] }],
  };
  for (const dimension of ["cli", "llm", "models"]) {
    const { series } = chartSeriesFor(activity, dimension, providerLabel, providerHue);
    assert.equal(
      series.flatMap(({ values }) => values).reduce((sum, value) => sum + value, 0),
      307,
    );
  }
  assert.deepEqual(
    chartSeriesFor(activity, "llm", providerLabel, providerHue).series.map(({ label }) => label),
    ["cliproxyapi", "PI / unattributed"],
  );
});

// @lat: [[widget-usage-tests#Widget Usage Tests#Chart group preservation]]
test("Model dimension merges one model across CLIs by normalized id", () => {
  const activity = {
    bucketSeconds: 300,
    bucketStarts: ["2026-08-01T12:00:00Z", "2026-08-01T12:05:00Z"],
    series: [
      {
        identity: { provider: "pi", modelId: "cliproxyapi/gpt-5.6-sol" },
        sessionsPerBucket: [0, 1],
        tokensPerBucket: [0, 20],
      },
      {
        identity: { provider: "codex", modelId: "gpt-5.6-sol" },
        sessionsPerBucket: [1, 0],
        tokensPerBucket: [130, 0],
      },
      {
        identity: { provider: "claude", modelId: "gpt-5.6-sol" },
        sessionsPerBucket: [0, 1],
        tokensPerBucket: [0, 50],
      },
    ],
    unattributedSeries: [],
  };

  for (const dimension of ["cli", "llm", "models"]) {
    const { series } = chartSeriesFor(activity, dimension, providerLabel, providerHue);
    assert.equal(
      series.flatMap(({ values }) => values).reduce((sum, value) => sum + value, 0),
      200,
    );
  }

  const models = chartSeriesFor(activity, "models", providerLabel, providerHue).series;
  assert.equal(models.length, 1);
  assert.equal(models[0].id, "model:gpt-5.6-sol");
  assert.equal(models[0].label, "gpt-5.6-sol");

  const cli = chartSeriesFor(activity, "cli", providerLabel, providerHue).series;
  assert.equal(cli.length, 3);
});
