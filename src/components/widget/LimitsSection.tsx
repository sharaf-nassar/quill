import { Fragment, useEffect, useId, useMemo, useState } from "react";
import type {
  CpaAccountHealth,
  CpaPoolAggregate,
  IntegrationProvider,
  ProviderStatus,
  UsageBucket,
  UsageData,
} from "../../types";
import { providerLabel } from "../../utils/providers";

/**
 * LIMITS — direct-provider rows plus source-keyed CPA pool rows.
 *
 * Direct rows retain their compact fixed-window treatment. CPA accounts are
 * summarized first by provider and source, then disclosed on demand so the
 * 360px widget remains a glanceable instrument rather than an account table.
 *
 * Severity rules follow [[lat.md/features#Features#Live Usage View]]: amber at
 * 50%, red at 80%, and any bucket whose reset elapsed renders neutral.
 */

type Severity = "nominal" | "caution" | "critical" | "stale";
type RowState = "ready" | "pending" | "setup" | "unavailable";
type CpaProvider = Extract<IntegrationProvider, "claude" | "codex">;
type AccountState = "ready" | "disabled" | "unavailable" | "cooling";

const TICK_MS = 10_000;
const MAX_VISIBLE_ACCOUNTS = 6;
const CPA_PROVIDERS: readonly CpaProvider[] = ["claude", "codex"];
const PRIMARY_MINIMAX_MODELS = ["M*", "coding-plan-search", "coding-plan-vlm"];

interface WindowDefinition {
  key: string;
  fullLabel: string;
  shortLabel: string;
  sortOrder: number;
}

interface LimitCell extends WindowDefinition {
  percent: number | null;
  fraction: number;
  severity: Severity;
  remainingMs: number | null;
}

interface LimitsRow {
  provider: IntegrationProvider;
  state: RowState;
  cells: LimitCell[];
  resetText: string | null;
  resetSeverity: Severity;
  detail: string | null;
}

interface CpaAccountRow {
  id: string;
  label: string;
  statusMessage: string | null;
  state: AccountState;
  cells: LimitCell[];
}

interface CpaLimitsRow extends LimitsRow {
  provider: CpaProvider;
  healthy: number | null;
  total: number | null;
  accounts: CpaAccountRow[];
}

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

function msUntil(resetsAt: string | null, nowMs: number): number | null {
  if (!resetsAt) return null;
  const parsed = Date.parse(resetsAt);
  if (Number.isNaN(parsed)) return null;
  return parsed - nowMs;
}

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

function cpaWindowKey(bucket: UsageBucket): string {
  const parts = bucket.key.split("/");
  return parts[0] === "cpa" && parts.length >= 3
    ? parts.slice(2).join("/")
    : bucket.key;
}

function numericCell(
  provider: IntegrationProvider,
  bucket: UsageBucket,
  nowMs: number,
  key = bucket.key,
): LimitCell {
  const remainingMs = msUntil(bucket.resets_at, nowMs);
  const stale = remainingMs !== null && remainingMs <= 0;
  return {
    key,
    fullLabel: bucket.label,
    shortLabel: shortBucketLabel(bucket.label, provider),
    sortOrder: bucket.sort_order ?? 0,
    percent: Math.round(bucket.utilization),
    fraction: Math.max(0, Math.min(bucket.utilization / 100, 1)),
    severity: stale ? "stale" : severityFor(bucket.utilization),
    remainingMs,
  };
}

function directCells(
  provider: IntegrationProvider,
  buckets: UsageBucket[],
  nowMs: number,
): LimitCell[] {
  const direct = buckets.filter(
    (bucket) =>
      bucket.provider === provider && (bucket.source ?? "direct") === "direct",
  );
  const primary =
    provider === "mini_max" ? direct.filter(isPrimaryMinimaxBucket) : direct;
  return [...primary]
    .sort((left, right) => (left.sort_order ?? 0) - (right.sort_order ?? 0))
    .map((bucket) => numericCell(provider, bucket, nowMs));
}

function fallbackWindows(provider: CpaProvider): WindowDefinition[] {
  if (provider === "claude") {
    return [
      { key: "five_hour", fullLabel: "5 hours", shortLabel: "5 HOURS", sortOrder: 0 },
      { key: "seven_day", fullLabel: "7 days", shortLabel: "7 DAYS", sortOrder: 0 },
    ];
  }
  return [
    {
      key: "primary_300m",
      fullLabel: "5 hours",
      shortLabel: "5 HOURS",
      sortOrder: 0,
    },
    {
      key: "secondary_10080m",
      fullLabel: "7 days",
      shortLabel: "7 DAYS",
      sortOrder: 0,
    },
  ];
}

function cpaWindowDefinitions(
  provider: CpaProvider,
  aggregateBuckets: UsageBucket[],
  accountBuckets: UsageBucket[],
): WindowDefinition[] {
  const definitions = new Map<string, WindowDefinition>();
  for (const bucket of [...aggregateBuckets, ...accountBuckets]) {
    const key = cpaWindowKey(bucket);
    if (definitions.has(key)) continue;
    definitions.set(key, {
      key,
      fullLabel: bucket.label,
      shortLabel: shortBucketLabel(bucket.label, provider),
      sortOrder: bucket.sort_order ?? 0,
    });
  }
  const windows = [...definitions.values()].sort(
    (left, right) =>
      left.sortOrder - right.sortOrder ||
      left.fullLabel.localeCompare(right.fullLabel) ||
      left.key.localeCompare(right.key),
  );
  return windows.length > 0 ? windows : fallbackWindows(provider);
}

function cpaCells(
  provider: CpaProvider,
  definitions: WindowDefinition[],
  buckets: UsageBucket[],
  nowMs: number,
): LimitCell[] {
  const byWindow = new Map(
    buckets.map((bucket) => [cpaWindowKey(bucket), bucket] as const),
  );
  return definitions.map((definition) => {
    const bucket = byWindow.get(definition.key);
    if (bucket) return numericCell(provider, bucket, nowMs, definition.key);
    return {
      ...definition,
      percent: null,
      fraction: 0,
      severity: "stale",
      remainingMs: null,
    };
  });
}

function rowTiming(cells: LimitCell[]): Pick<
  LimitsRow,
  "resetText" | "resetSeverity"
> {
  const nearest = cells.reduce<{ cell: LimitCell; ms: number } | null>(
    (best, cell) => {
      const ms = cell.remainingMs;
      if (ms === null || ms <= 0) return best;
      return best === null || ms < best.ms ? { cell, ms } : best;
    },
    null,
  );
  const anyDated = cells.some((cell) => cell.remainingMs !== null);
  return {
    resetText:
      nearest !== null
        ? formatCountdown(nearest.ms)
        : anyDated
          ? "now"
          : null,
    resetSeverity: nearest?.cell.severity ?? "nominal",
  };
}

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

function directRows(
  statuses: ProviderStatus[],
  data: UsageData | null,
  nowMs: number,
): LimitsRow[] {
  const loaded = data !== null;
  return statuses
    .filter((status) => status.enabled)
    .map((status) => {
      const cells = directCells(status.provider, data?.buckets ?? [], nowMs);
      const providerError =
        data?.provider_errors.find(
          (error) =>
            error.provider === status.provider &&
            (error.source ?? "direct") === "direct",
        ) ?? null;
      return {
        provider: status.provider,
        state:
          cells.length > 0
            ? "ready"
            : emptyRowState(status, providerError?.kind ?? null, loaded),
        cells,
        ...rowTiming(cells),
        detail: providerError?.message ?? status.lastError ?? null,
      };
    });
}

function asCpaProvider(provider: string): CpaProvider | null {
  const normalized = provider.trim().toLowerCase();
  return normalized === "claude" || normalized === "codex"
    ? normalized
    : null;
}

function accountState(account: CpaAccountHealth): AccountState {
  if (account.disabled) return "disabled";
  if (account.unavailable) return "unavailable";
  if (account.status !== "ready") return "cooling";
  return "ready";
}

function cpaRows(data: UsageData | null, nowMs: number): CpaLimitsRow[] {
  if (!data) return [];

  const pools = new Map<CpaProvider, CpaPoolAggregate>();
  for (const pool of data.cpa_pools ?? []) {
    if (pool.provider === "claude" || pool.provider === "codex") {
      pools.set(pool.provider, pool);
    }
  }

  const accounts = new Map<CpaProvider, CpaAccountHealth[]>();
  for (const account of data.cpa_accounts ?? []) {
    const provider = asCpaProvider(account.provider);
    if (!provider) continue;
    const group = accounts.get(provider) ?? [];
    group.push(account);
    accounts.set(provider, group);
  }

  const allCpaBuckets = (data.buckets ?? []).filter(
    (bucket) => bucket.source === "cpa" && bucket.account_id,
  );

  return CPA_PROVIDERS.flatMap((provider) => {
    const pool = pools.get(provider);
    const providerAccounts = [...(accounts.get(provider) ?? [])].sort(
      (left, right) =>
        left.label.localeCompare(right.label) ||
        left.auth_index.localeCompare(right.auth_index),
    );
    const providerError =
      data.provider_errors.find(
        (error) => error.provider === provider && error.source === "cpa",
      ) ?? null;
    if (!pool && providerAccounts.length === 0 && !providerError) return [];

    const providerBuckets = allCpaBuckets.filter(
      (bucket) => bucket.provider === provider,
    );
    const definitions = cpaWindowDefinitions(
      provider,
      pool?.buckets ?? [],
      providerBuckets,
    );
    const cells = cpaCells(provider, definitions, pool?.buckets ?? [], nowMs);
    const hasInventory = pool !== undefined || providerAccounts.length > 0;
    const health = providerAccounts.filter(
      (account) => accountState(account) === "ready",
    ).length;

    return [
      {
        provider,
        state: hasInventory
          ? "ready"
          : providerError?.kind === "config" || providerError?.kind === "auth"
            ? "setup"
            : "unavailable",
        cells: hasInventory ? cells : [],
        ...rowTiming(hasInventory ? cells : []),
        detail: providerError?.message ?? null,
        healthy: pool?.healthy ?? (hasInventory ? health : null),
        total: pool?.total ?? (hasInventory ? providerAccounts.length : null),
        accounts: providerAccounts.map((account) => {
          const accountBuckets = providerBuckets.filter(
            (bucket) => bucket.account_id === account.auth_index,
          );
          return {
            id: account.auth_index,
            label:
              account.label.trim() ||
              accountBuckets[0]?.account_label?.trim() ||
              account.auth_index,
            statusMessage: account.status_message?.trim() || null,
            state: accountState(account),
            cells: cpaCells(provider, definitions, accountBuckets, nowMs),
          };
        }),
      },
    ];
  });
}

function CpaChevron() {
  return (
    <svg
      className="wg-cpa-chevron"
      width="8"
      height="8"
      viewBox="0 0 8 8"
      fill="none"
      stroke="currentColor"
      strokeWidth={1.3}
      strokeLinecap="round"
      aria-hidden="true"
      focusable={false}
    >
      <path d="M1.5 3l2.5 2.5L6.5 3" />
    </svg>
  );
}

function WindowCells({
  cells,
  ownerLabel,
}: {
  cells: LimitCell[];
  ownerLabel: string;
}) {
  return (
    <div className="wg-limits-cells">
      {cells.map((cell) => (
        <div
          className="wg-limits-bucket"
          data-placeholder={cell.percent === null ? "true" : undefined}
          key={cell.key}
        >
          <div className="wg-limits-bucket-top">
            <span className="wg-limits-pct" data-severity={cell.severity}>
              {cell.percent === null ? "—" : `${cell.percent}%`}
            </span>
            <span className="wg-limits-window" title={cell.fullLabel}>
              {cell.shortLabel}
            </span>
          </div>
          {cell.percent === null ? (
            <div
              className="wg-bar"
              data-severity="stale"
              aria-label={`${ownerLabel} ${cell.fullLabel} utilization unavailable`}
            />
          ) : (
            <div
              className="wg-bar"
              data-severity={cell.severity}
              role="progressbar"
              aria-valuenow={cell.percent}
              aria-valuemin={0}
              aria-valuemax={100}
              aria-label={`${ownerLabel} ${cell.fullLabel} utilization`}
            >
              <div
                className="wg-bar-fill"
                style={{ width: `${cell.fraction * 100}%` }}
              />
            </div>
          )}
        </div>
      ))}
    </div>
  );
}

function DirectRow({ row }: { row: LimitsRow }) {
  const name = providerLabel(row.provider);
  return (
    <div className="wg-limits-row" data-source="direct">
      <span
        className="wg-limits-swatch"
        data-provider={row.provider}
        aria-hidden="true"
      />
      <span className="wg-limits-name">{name.toUpperCase()}</span>

      {row.state === "ready" && (
        <WindowCells cells={row.cells} ownerLabel={name} />
      )}
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
}

function AccountHealth({ state }: { state: AccountState }) {
  if (state === "ready") return null;
  return (
    <span className="wg-cpa-account-state" data-state={state}>
      {state !== "disabled" && (
        <span className="wg-limits-lamp" aria-hidden="true" />
      )}
      {state === "disabled"
        ? "DISABLED"
        : state === "unavailable"
          ? "UNAVAILABLE"
          : "COOLING"}
    </span>
  );
}

function CpaRow({
  row,
  expanded,
  controlsId,
  onToggle,
}: {
  row: CpaLimitsRow;
  expanded: boolean;
  controlsId: string;
  onToggle: () => void;
}) {
  const name = providerLabel(row.provider);
  const visibleAccounts = row.accounts.slice(0, MAX_VISIBLE_ACCOUNTS);
  const hiddenCount = Math.max(0, row.accounts.length - visibleAccounts.length);
  const healthText =
    row.healthy === null || row.total === null
      ? "—/—"
      : `${row.healthy}/${row.total}`;
  const healthLabel =
    row.healthy === null || row.total === null
      ? `${name} CPA health count unavailable`
      : `${row.healthy} healthy of ${row.total} ${name} CPA accounts`;

  return (
    <div
      className="wg-cpa-group"
      data-open={expanded ? "true" : undefined}
      role="group"
      aria-label={`${name} CPA pool`}
    >
      <div className="wg-limits-row" data-source="cpa">
        {row.accounts.length > 0 ? (
          <button
            type="button"
            className="wg-cpa-toggle"
            aria-label={`${expanded ? "Collapse" : "Expand"} ${name} CPA accounts`}
            aria-expanded={expanded}
            aria-controls={controlsId}
            onClick={onToggle}
          >
            <CpaChevron />
          </button>
        ) : (
          <span className="wg-cpa-toggle-space" aria-hidden="true" />
        )}
        <span
          className="wg-limits-swatch"
          data-provider={row.provider}
          aria-hidden="true"
        />
        <span className="wg-cpa-identity">
          <span className="wg-limits-name">{name.toUpperCase()}</span>
          <span className="wg-cpa-meta">
            <span className="wg-cpa-source">CPA</span>
            <span className="wg-cpa-health" aria-label={healthLabel}>
              {healthText}
            </span>
          </span>
        </span>

        {row.state === "ready" && (
          <WindowCells cells={row.cells} ownerLabel={`${name} CPA pool`} />
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

      {row.accounts.length > 0 && (
        <div
          className="wg-cpa-accounts"
          id={controlsId}
          hidden={!expanded}
        >
          {visibleAccounts.map((account) => (
            <div
              className="wg-cpa-account-row"
              data-state={account.state}
              title={account.statusMessage ?? undefined}
              key={account.id}
            >
              <span className="wg-cpa-account-identity">
                <span className="wg-cpa-account-name" title={account.label}>
                  {account.label}
                </span>
                <AccountHealth state={account.state} />
              </span>
              <WindowCells
                cells={account.cells}
                ownerLabel={`${name} account ${account.label}`}
              />
            </div>
          ))}
          {hiddenCount > 0 && (
            <div className="wg-cpa-more">…and {hiddenCount} more</div>
          )}
        </div>
      )}
    </div>
  );
}

interface LimitsSectionProps {
  data: UsageData | null;
  statuses: ProviderStatus[];
}

// @lat: [[frontend#Frontend#Components#Widget Limits Band]]
function LimitsSection({ data, statuses }: LimitsSectionProps) {
  const [nowMs, setNowMs] = useState(() => Date.now());
  const [expanded, setExpanded] = useState<Set<CpaProvider>>(() => new Set());
  const disclosurePrefix = useId().replace(/\W/g, "");

  useEffect(() => {
    const interval = setInterval(() => setNowMs(Date.now()), TICK_MS);
    return () => clearInterval(interval);
  }, []);

  const direct = useMemo(
    () => directRows(statuses, data, nowMs),
    [statuses, data, nowMs],
  );
  const cpa = useMemo(() => cpaRows(data, nowMs), [data, nowMs]);
  const directByProvider = new Map(direct.map((row) => [row.provider, row]));
  const cpaByProvider = new Map(cpa.map((row) => [row.provider, row]));
  const providerOrder = [
    ...new Set<IntegrationProvider>([
      ...statuses.map((status) => status.provider),
      ...cpa.map((row) => row.provider),
    ]),
  ];
  const otherAccountCount = (data?.cpa_accounts ?? []).filter(
    (account) => asCpaProvider(account.provider) === null,
  ).length;

  if (direct.length === 0 && cpa.length === 0 && otherAccountCount === 0) {
    return null;
  }

  return (
    <section className="wg-limits wg-num" aria-label="Subscription limits">
      <span className="wg-limits-title">Limits</span>
      {providerOrder.map((provider) => {
        const directRow = directByProvider.get(provider);
        const cpaRow =
          provider === "mini_max" ? undefined : cpaByProvider.get(provider);
        return (
          <Fragment key={provider}>
            {directRow && <DirectRow row={directRow} />}
            {cpaRow && (
              <CpaRow
                row={cpaRow}
                expanded={expanded.has(cpaRow.provider)}
                controlsId={`${disclosurePrefix}-${cpaRow.provider}-accounts`}
                onToggle={() =>
                  setExpanded((current) => {
                    const next = new Set(current);
                    if (next.has(cpaRow.provider)) next.delete(cpaRow.provider);
                    else next.add(cpaRow.provider);
                    return next;
                  })
                }
              />
            )}
          </Fragment>
        );
      })}
      {otherAccountCount > 0 && (
        <div className="wg-cpa-other">+{otherAccountCount} other accounts</div>
      )}
    </section>
  );
}

export default LimitsSection;
