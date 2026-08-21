import type { ModelActivity } from "../../types";
import type { WidgetChartDimension } from "./rangePreference";

export interface ChartSeries {
  readonly id: string;
  readonly label: string;
  readonly provider: string;
  readonly values: readonly number[];
  readonly color: string;
}

type ProviderLabel = (provider: string) => string;
type ProviderHue = (provider: string) => string;

const CHART_PALETTE = [
  "#60a5fa",
  "#fb923c",
  "#a78bfa",
  "#2dd4bf",
  "#f472b6",
  "#a3e635",
  "#8b949e",
] as const;

function upstreamLabel(provider: string, modelId: string): string {
  const separator = modelId.indexOf("/");
  return provider === "pi" && separator > 0 ? modelId.slice(0, separator) : provider;
}

/**
 * Merges a model's evidence across CLIs: Pi ids embed the upstream gateway
 * prefix (`cliproxyapi/gpt-5.6-sol`) while other CLIs report the bare model
 * id, so stripping that prefix is a pure string derivation with no catalog
 * or vendor inference. See docs/solutions/conventions/model-id-normalization-boundary.md.
 */
export function normalizeModelId(provider: string, modelId: string): string {
  const separator = modelId.indexOf("/");
  return provider === "pi" && separator > 0 ? modelId.slice(separator + 1) : modelId;
}

export function chartSeriesFor(
  activity: ModelActivity | undefined,
  dimension: WidgetChartDimension,
  providerLabel: ProviderLabel,
  providerHue: ProviderHue,
): { series: ChartSeries[]; labels: string[] } {
  if (!activity) return { series: [], labels: [] };
  const groups = new Map<string, { label: string; provider: string; values: number[] }>();
  const add = (key: string, label: string, provider: string, values: readonly number[]) => {
    const group = groups.get(key) ?? { label, provider, values: new Array(values.length).fill(0) };
    for (let index = 0; index < values.length; index += 1) group.values[index] += values[index] ?? 0;
    groups.set(key, group);
  };
  for (const entry of activity.series) {
    const { provider, modelId } = entry.identity;
    if (dimension === "cli") add(provider, providerLabel(provider), provider, entry.tokensPerBucket);
    else if (dimension === "llm") {
      const label = upstreamLabel(provider, modelId);
      add(`llm:${label}`, label, provider, entry.tokensPerBucket);
    } else {
      const normalized = normalizeModelId(provider, modelId);
      add(`model:${normalized}`, normalized, provider, entry.tokensPerBucket);
    }
  }
  for (const entry of activity.unattributedSeries) {
    const label = `${providerLabel(entry.provider)} / unattributed`;
    if (dimension === "cli") add(entry.provider, providerLabel(entry.provider), entry.provider, entry.tokensPerBucket);
    else add(`unattributed:${entry.provider}`, label, entry.provider, entry.tokensPerBucket);
  }
  const series = Array.from(groups.entries())
    .map(([id, group]) => ({
      id,
      label: group.label,
      provider: group.provider,
      values: group.values,
    }))
    .sort((left, right) => right.values.reduce((sum, value) => sum + value, 0) - left.values.reduce((sum, value) => sum + value, 0) || left.label.localeCompare(right.label))
    .map((entry, index) => ({
      ...entry,
      color: dimension === "cli" ? providerHue(entry.provider) : CHART_PALETTE[index % CHART_PALETTE.length],
    }));
  return { series, labels: activity.bucketStarts };
}
