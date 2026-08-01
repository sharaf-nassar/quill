import { useEffect, useMemo, useState } from "react";
import type {
  IntegrationProvider,
  ProviderStatus,
  UsageBucket,
  UsageData,
} from "../../types";
import { providerLabel } from "../../utils/providers";

/**
 * LIMITS — the widget's compact subscription readout.
 *
 * One row per *enabled* provider: identity swatch + name, one fixed-width cell
 * per rate-limit window (percent, short window label, 4px bar), and a
 * right-aligned countdown to that row's nearest reset. The whole section is
 * absent when no provider is enabled, so the widget simply starts at the view
 * region with no reserved gap.
 *
 * Severity rules carried over verbatim from the old live pane
 * ([[lat.md/features#Features#Live Usage View]]): amber at 50%, red at 80%,
 * and a bucket whose `resets_at` has already elapsed renders neutral — a
 * utilization measured against a bygone window must never read as a live
 * severity.
 */

/** Bar/percent severity. `stale` is deliberately outside the 50/80 ramp. */
type Severity = "nominal" | "caution" | "critical" | "stale";

/** How a provider's row renders when it has no live buckets to show. */
type RowState = "ready" | "pending" | "setup" | "unavailable";

/** Countdown re-render cadence; matches the old usage row's 10s tick. */
const TICK_MS = 10_000;

/**
 * MiniMax reports one bucket per model. Only the plan-level buckets belong in
 * the widget row — the long tail stays out of a 360px surface (same filter the
 * live pane applied).
 */
const PRIMARY_MINIMAX_MODELS = ["M*", "coding-plan-search", "coding-plan-vlm"];

function isPrimaryMinimaxBucket(bucket: UsageBucket): boolean {
  return PRIMARY_MINIMAX_MODELS.some(
    (model) => bucket.label.startsWith(`${model} `) || bucket.label === model,
  );
}

function severityFor(utilization: number): Severity {
  if (utilization < 50) return "nominal";
  if (utilization < 80) return "caution";
  return "critical";
}

/** Milliseconds until `resetsAt`, or null when absent/unparseable. */
function msUntil(resetsAt: string | null, nowMs: number): number | null {
  if (!resetsAt) return null;
  const parsed = Date.parse(resetsAt);
  if (Number.isNaN(parsed)) return null;
  return parsed - nowMs;
}

/** Compact countdown: `6d 22h`, `3h 54m`, `44m`, or `now` once elapsed. */
function formatCountdown(remainingMs: number): string {
  const totalSeconds = Math.floor(remainingMs / 1000);
  if (totalSeconds <= 0) return "now";
  const days = Math.floor(totalSeconds / 86_400);
  const hours = Math.floor((totalSeconds % 86_400) / 3600);
  const minutes = Math.floor((totalSeconds % 3600) / 60);
  if (days > 0) return `${days}d ${String(hours).padStart(2, "0")}h`;
  if (hours > 0) return `${hours}h ${String(minutes).padStart(2, "0")}m`;
  return `${minutes}m`;
}

/**
 * Compress a bucket label to a column heading. The row already names the
 * provider, so the provider token is dropped and only the window survives:
 * `Codex · Weekly` → `WEEKLY`, `Sonnet · 5h` → `5H`, `Codex Spark` → `SPARK`.
 * Nothing is invented — the untouched label stays in the cell's title and in
 * the bar's accessible name.
 */
function shortBucketLabel(
  label: string,
  provider: IntegrationProvider,
): string {
  const tail = label.split("·").pop()?.trim() ?? "";
  const base = tail.length > 0 ? tail : label.trim();
  const withoutProvider = base
    .replace(new RegExp(`^${providerLabel(provider)}\\s+`, "i"), "")
    .trim();
  return (withoutProvider.length > 0 ? withoutProvider : base).toUpperCase();
}

interface RowBucket {
  key: string;
  fullLabel: string;
  shortLabel: string;
  percent: number;
  fraction: number;
  severity: Severity;
  remainingMs: number | null;
}

interface LimitsRow {
  provider: IntegrationProvider;
  state: RowState;
  buckets: RowBucket[];
  resetText: string | null;
  resetSeverity: Severity;
  detail: string | null;
}

function buildBuckets(
  provider: IntegrationProvider,
  buckets: UsageBucket[],
  nowMs: number,
): RowBucket[] {
  const scoped = buckets.filter((bucket) => bucket.provider === provider);
  const primary =
    provider === "mini_max" ? scoped.filter(isPrimaryMinimaxBucket) : scoped;
  const ordered = [...primary].sort(
    (left, right) => (left.sort_order ?? 0) - (right.sort_order ?? 0),
  );
  return ordered.map((bucket) => {
    const remainingMs = msUntil(bucket.resets_at, nowMs);
    const stale = remainingMs !== null && remainingMs <= 0;
    return {
      key: `${bucket.provider}:${bucket.key}`,
      fullLabel: bucket.label,
      shortLabel: shortBucketLabel(bucket.label, provider),
      percent: Math.round(bucket.utilization),
      fraction: Math.max(0, Math.min(bucket.utilization / 100, 1)),
      severity: stale ? "stale" : severityFor(bucket.utilization),
      remainingMs,
    };
  });
}

/**
 * A provider with no live buckets is either mid-setup or simply not answering.
 * `config`/`auth` failures (and an unfinished install) are actionable — the
 * user supplies a key or finishes setup — so they read SETUP; everything else
 * reads UNAVAILABLE rather than pretending a number exists.
 */
function emptyRowState(
  status: ProviderStatus,
  errorKind: string | null,
  loaded: boolean,
): RowState {
  if (errorKind === "config" || errorKind === "auth") return "setup";
  if (
    status.setupState === "missing" ||
    status.setupState === "error" ||
    status.setupState === "not_installed" ||
    status.setupState === "installing"
  ) {
    return "setup";
  }
  if (!loaded && errorKind === null) return "pending";
  return "unavailable";
}

function buildRows(
  statuses: ProviderStatus[],
  data: UsageData | null,
  nowMs: number,
): LimitsRow[] {
  const loaded = data !== null;
  return statuses
    .filter((status) => status.enabled)
    .map((status) => {
      const buckets = buildBuckets(status.provider, data?.buckets ?? [], nowMs);
      const providerError =
        data?.provider_errors.find(
          (error) => error.provider === status.provider,
        ) ?? null;

      // Nearest *upcoming* reset, and the severity of the bucket that owns it
      // — the countdown belongs to one bucket, not to the row's worst cell. A
      // window that already rolled over is not an upcoming reset; it only
      // reads "now" when the whole row is waiting on refreshed numbers.
      const nearest = buckets.reduce<{ bucket: RowBucket; ms: number } | null>(
        (best, bucket) => {
          const ms = bucket.remainingMs;
          if (ms === null || ms <= 0) return best;
          return best === null || ms < best.ms ? { bucket, ms } : best;
        },
        null,
      );
      const anyDated = buckets.some((bucket) => bucket.remainingMs !== null);

      return {
        provider: status.provider,
        state:
          buckets.length > 0
            ? "ready"
            : emptyRowState(status, providerError?.kind ?? null, loaded),
        buckets,
        resetText:
          nearest !== null
            ? formatCountdown(nearest.ms)
            : anyDated
              ? "now"
              : null,
        resetSeverity: nearest?.bucket.severity ?? "nominal",
        detail: providerError?.message ?? status.lastError ?? null,
      };
    });
}

interface LimitsSectionProps {
  /** Latest usage poll, or null before the first response lands. */
  data: UsageData | null;
  /** Provider statuses; only enabled providers get a row. */
  statuses: ProviderStatus[];
}

function LimitsSection({ data, statuses }: LimitsSectionProps) {
  const [nowMs, setNowMs] = useState(() => Date.now());

  useEffect(() => {
    const interval = setInterval(() => setNowMs(Date.now()), TICK_MS);
    return () => clearInterval(interval);
  }, []);

  const rows = useMemo(
    () => buildRows(statuses, data, nowMs),
    [statuses, data, nowMs],
  );

  if (rows.length === 0) return null;

  return (
    <section className="wg-limits wg-num" aria-label="Subscription limits">
      <span className="wg-limits-title">Limits</span>
      {rows.map((row) => {
        const name = providerLabel(row.provider);
        return (
          <div className="wg-limits-row" key={row.provider}>
            <span
              className="wg-limits-swatch"
              data-provider={row.provider}
              aria-hidden="true"
            />
            <span className="wg-limits-name">{name.toUpperCase()}</span>

            {row.state === "ready" &&
              row.buckets.map((bucket) => (
                <div className="wg-limits-bucket" key={bucket.key}>
                  <div className="wg-limits-bucket-top">
                    <span
                      className="wg-limits-pct"
                      data-severity={bucket.severity}
                    >
                      {bucket.percent}%
                    </span>
                    <span
                      className="wg-limits-window"
                      title={bucket.fullLabel}
                    >
                      {bucket.shortLabel}
                    </span>
                  </div>
                  <div
                    className="wg-bar"
                    data-severity={bucket.severity}
                    role="progressbar"
                    aria-valuenow={bucket.percent}
                    aria-valuemin={0}
                    aria-valuemax={100}
                    aria-label={`${name} ${bucket.fullLabel} utilization`}
                  >
                    <div
                      className="wg-bar-fill"
                      style={{ width: `${bucket.fraction * 100}%` }}
                    />
                  </div>
                </div>
              ))}

            {row.state === "pending" && (
              <div className="wg-limits-pending" aria-hidden="true">
                <div className="wg-skeleton wg-skeleton-line" />
                <div className="wg-skeleton wg-skeleton-line" />
              </div>
            )}

            {(row.state === "setup" || row.state === "unavailable") && (
              <span
                className="wg-limits-status"
                data-tone={row.state}
                title={row.detail ?? undefined}
              >
                <span className="wg-limits-lamp" aria-hidden="true" />
                {row.state === "setup" ? "SETUP" : "UNAVAILABLE"}
              </span>
            )}

            <span className="wg-limits-reset" data-severity={row.resetSeverity}>
              {row.resetText}
            </span>
          </div>
        );
      })}
    </section>
  );
}

export default LimitsSection;
