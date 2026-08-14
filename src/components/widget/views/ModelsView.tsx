// ModelsView — provider-qualified model evidence at 360px.
//
// Two bands and nothing else: what each provider is running *right now*, and
// which models actually did the work over the selected range. Selecting a row
// is deliberately inert — session paging and chain history stay out of the
// widget (plan: "no inspect panel in v1").
//
// Three rules this file obeys, all of them constitution #1:
//
//   - **Raw ids only.** A model renders exactly as it was observed, qualified
//     by its provider chip. No catalog, no family parsing, no friendly name.
//     A long id is visually truncated with the full string in `title` — never
//     rewritten, because a shortened id is still the id and an invented one is
//     not.
//   - **A shade is never a name.** Identity is always swatch *plus* id. The
//     swatch is a rank-assigned shade of the provider's family ramp
//     (DESIGN.md, The Model-Shade Rule), not a hue derived from the id.
//   - **Empty is a claim, so it has to be earned.** "No models" is stated only
//     when the backend calls the scope final *and* the history inventory is
//     complete; otherwise the band says the evidence is still being processed
//     and the retained-history line says why.
//   - **Zero is never a stand-in for absent.** A model that ran sessions
//     necessarily burned tokens, so a zero attributed-token figure is the
//     absence of a measurement, not a measurement of nothing. It renders as an
//     em dash — the same way every other widget head states a figure it does
//     not have — with the reason on hover.
//
// See specs/018-widget-ui-redesign/plan.md#Affected Components.

import { useEffect, useMemo, useState } from "react";
import { useModelAnalytics } from "../../../hooks/useModelAnalytics";
import { useRollupBackfill } from "../../../hooks/useRollupBackfill";
import { formatNumber, formatRecency } from "../../../utils/format";
import { providerTag } from "../../../utils/providers";
import { formatTokenCount } from "../../../utils/tokens";
import { modelIdentityKey } from "../../../types";
import type {
  ModelBackfillStatus,
  ModelIdentity,
  ModelIdentityKey,
  ModelUsageOverviewResponse,
  ModelUsageOverviewRow,
  RangeType,
} from "../../../types";

/** Ranked rows shown before the list would start dominating the widget. */
const MODEL_LIMIT = 5;

/**
 * Per-provider shade ramps, rank 1..6 within the provider by delivered order
 * (the backend delivers models session-ranked). Values mirror DESIGN.md's
 * Model-Shade Rule; Claude's orange is deliberately redder than caution amber
 * so a provider hue can never read as severity.
 */
const CLAUDE_SHADES = [
  "#fb923c",
  "#cf4a0c",
  "#fed7aa",
  "#9a3412",
  "#ffedd5",
  "#7c2d12",
] as const;

const CODEX_SHADES = [
  "#60a5fa",
  "#2563eb",
  "#93c5fd",
  "#1d4ed8",
  "#a7cdfd",
  "#16308f",
] as const;

/** MiniMax and every additional provider family draw from violet. */
const VIOLET_SHADES = [
  "#a78bfa",
  "#7c3aed",
  "#ddd6fe",
  "#5b21b6",
  "#ede9fe",
  "#4c1d95",
] as const;

/** Rank seven and beyond renders neutral rather than a generated hue. */
const NEUTRAL_SHADE = "#8b949e";

// 24-hour, matching the Usage readouts. The `AM`/`PM` suffix is not a tabular
// figure and would make the takeover caption change width across noon.
const CLOCK_FORMAT = new Intl.DateTimeFormat(undefined, {
  hour: "2-digit",
  minute: "2-digit",
  hour12: false,
});

const DATE_CLOCK_FORMAT = new Intl.DateTimeFormat(undefined, {
  month: "short",
  day: "numeric",
  hour: "2-digit",
  minute: "2-digit",
  hour12: false,
});

const BACKFILL_LABELS: Record<ModelBackfillStatus["status"], string> = {
  pending: "pending",
  running: "running",
  complete: "complete",
  partial: "partial",
  failed: "failed",
};

const EMPTY_MODELS: readonly ModelUsageOverviewRow[] = [];

function shadeRamp(provider: string): readonly string[] {
  if (provider === "claude") return CLAUDE_SHADES;
  if (provider === "codex") return CODEX_SHADES;
  return VIOLET_SHADES;
}

/**
 * Scope-stable shade assignment, computed once per overview response. Both
 * bands read the same map, so a model keeps one shade across the whole view.
 */
function buildShadeMap(
  models: readonly ModelUsageOverviewRow[],
): ReadonlyMap<ModelIdentityKey, string> {
  const shades = new Map<ModelIdentityKey, string>();
  const ranks = new Map<string, number>();
  for (const row of models) {
    const key = modelIdentityKey(row.identity);
    if (shades.has(key)) continue;
    const rank = ranks.get(row.identity.provider) ?? 0;
    ranks.set(row.identity.provider, rank + 1);
    const ramp = shadeRamp(row.identity.provider);
    shades.set(key, rank < ramp.length ? ramp[rank] : NEUTRAL_SHADE);
  }
  return shades;
}

function shadeFor(
  shades: ReadonlyMap<ModelIdentityKey, string>,
  identity: ModelIdentity,
): string {
  return shades.get(modelIdentityKey(identity)) ?? NEUTRAL_SHADE;
}

/** Chip tone; unknown providers stay neutral rather than borrowing a hue. */
function chipTone(provider: string): string {
  if (provider === "claude" || provider === "codex" || provider === "mini_max") {
    return provider;
  }
  return "other";
}

/** `16:49 today` while the takeover is same-day, else `Jul 18, 22:14`. */
function formatSince(timestamp: string, nowMs: number): string {
  const then = new Date(timestamp);
  if (!Number.isFinite(then.getTime())) return timestamp;
  const now = new Date(nowMs);
  const sameDay =
    then.getFullYear() === now.getFullYear() &&
    then.getMonth() === now.getMonth() &&
    then.getDate() === now.getDate();
  return sameDay
    ? `${CLOCK_FORMAT.format(then)} today`
    : DATE_CLOCK_FORMAT.format(then);
}

/**
 * Whether the backend's scope facts can be trusted to prove a negative. Same
 * bar the full Models page uses: a complete inventory with nothing failed and
 * nothing left to process.
 */
function backfillProvesFinalScope(status: ModelBackfillStatus | null): boolean {
  return (
    status !== null &&
    status.status === "complete" &&
    status.inventoryComplete &&
    status.failedRoots === 0 &&
    status.failedSources === 0 &&
    status.remainingSources === 0
  );
}

/**
 * The one sentence an empty band is allowed to say. Scope facts already
 * exclude suppressed sources, so they — not the row count — decide which
 * negative is being claimed.
 */
function emptyClaim(
  overview: ModelUsageOverviewResponse,
  status: ModelBackfillStatus | null,
): string {
  if (!backfillProvesFinalScope(status) || !overview.scope.scopeFinal) {
    return "Model evidence is still being processed";
  }
  if (overview.scope.globalSessionCount === 0) {
    return "No retained sessions to attribute yet";
  }
  if (overview.scope.scopedSessionCount === 0) {
    return "No sessions were active in this range";
  }
  if (overview.scope.scopedEvidenceCount === 0) {
    return "Sessions ran, but none carry a model identifier";
  }
  return "No models observed in this range";
}

interface TokenReading {
  readonly text: string;
  /** Why the figure is absent, or null when a real figure is being shown. */
  readonly absence: string | null;
}

/**
 * Attributed tokens for a ranked row, or an em dash when there are none to
 * attribute.
 *
 * A model only earns a row by having run sessions, and a session that ran
 * necessarily burned tokens — so a zero here cannot be a measurement, only the
 * absence of one. It is what a provider whose observations carry no token
 * columns looks like (Codex reports cumulative deltas rather than
 * per-observation counts). Printing `0` beside the range's busiest model would
 * state a figure nobody recorded; the em dash says the number is missing and
 * the hover says why (constitution #1 — gaps stay explicit).
 */
function tokenReading(row: ModelUsageOverviewRow): TokenReading {
  if (row.attributedTokens > 0) {
    return { text: formatTokenCount(row.attributedTokens), absence: null };
  }
  return {
    text: "—",
    absence:
      "No token counts are attributed to this model — its provider reports usage without per-observation token figures",
  };
}

interface ModelSwatchProps {
  color: string;
}

/** 8px identity swatch — never rendered without the id beside it. */
function ModelSwatch({ color }: ModelSwatchProps) {
  return (
    <i
      className="wg-mv-swatch"
      style={{ background: color }}
      aria-hidden="true"
    />
  );
}

interface BandHeadProps {
  title: string;
  meta: string;
}

function BandHead({ title, meta }: BandHeadProps) {
  return (
    <div className="wg-mv-head">
      <span className="wg-mv-title">{title}</span>
      <span className="wg-mv-meta wg-num">{meta}</span>
    </div>
  );
}

export interface ModelsViewProps {
  range: RangeType;
}

function ModelsView({ range }: ModelsViewProps) {
  const [nowMs, setNowMs] = useState(() => Date.now());
  // The widget renders exactly one view, so a mounted Models view is by
  // definition the observable one — the hook's own visibility guard handles
  // the hidden-window case.
  const { overview, backfill } = useModelAnalytics(range, null, true);
  const modelRollup = useRollupBackfill("model");
  const data = overview.data;
  const retryOverview = overview.retry;

  const modelRollupCompleted = modelRollup.state.kind === "completed";
  useEffect(() => {
    if (modelRollupCompleted) retryOverview();
  }, [modelRollupCompleted, retryOverview]);

  // Recency and "since" labels are relative, so they age on their own clock
  // rather than waiting for the next refresh.
  useEffect(() => {
    const interval = setInterval(() => setNowMs(Date.now()), 30_000);
    return () => clearInterval(interval);
  }, []);

  const shades = useMemo(
    () => buildShadeMap(data?.models ?? EMPTY_MODELS),
    [data],
  );

  const ranked = useMemo(
    () => (data?.models ?? EMPTY_MODELS).slice(0, MODEL_LIMIT),
    [data],
  );

  // One scale for every bar: rank is only readable when the tracks share a
  // denominator, and that denominator is the top model in scope.
  const sessionsCeiling = ranked.reduce(
    (peak, row) => Math.max(peak, row.sessions),
    0,
  );

  const totalModels = data?.models.length ?? 0;
  const rankedMeta =
    totalModels > ranked.length
      ? `top ${ranked.length} of ${formatNumber(totalModels)}`
      : `${formatNumber(totalModels)} ${totalModels === 1 ? "model" : "models"}`;

  const coverage =
    data?.totals.coveragePercent === null ||
    data?.totals.coveragePercent === undefined
      ? null
      : Math.round(data.totals.coveragePercent);

  const status = backfill.status;
  const backfillNeedsAttention =
    status !== null &&
    (status.status !== "complete" ||
      status.lastError !== null ||
      backfill.retryError !== null ||
      backfill.isRetrying);
  const backfillRetryable =
    status !== null && (status.status === "partial" || status.status === "failed");

  const loading = overview.initialLoading && data === null;
  const failed = overview.error !== null && data === null;
  const buildingIndex = data?.buildingIndex === true;
  const modelIndexNote = (() => {
    const state = modelRollup.state;
    if (state.kind === "running") {
      return `Building model index · ${formatNumber(
        state.progress.rowsDone,
      )}/${formatNumber(
        state.progress.rowsTotal,
      )} observations · using raw evidence meanwhile`;
    }
    if (state.kind === "error") {
      return `Model index build stopped · ${state.detail} Models is using raw evidence; rebuild in Settings › Performance.`;
    }
    if (state.kind === "completed" && buildingIndex) {
      return "Model index complete · refreshing indexed analytics";
    }
    if (buildingIndex) {
      return "Building model index · using raw evidence while retained observations are indexed";
    }
    return null;
  })();

  if (failed) {
    return (
      <div className="wg-mv">
        <div className="wg-state wg-state-error">
          <span className="wg-state-lamp" aria-hidden="true" />
          <span>Model evidence unavailable</span>
          <button type="button" className="wg-mv-retry" onClick={overview.retry}>
            Retry
          </button>
        </div>
      </div>
    );
  }

  return (
    <div className="wg-mv">
      <section className="wg-mv-band" aria-label="Running now">
        <BandHead title="Running now" meta="per provider" />
        {loading ? (
          <div className="wg-mv-pending" aria-hidden="true">
            <div className="wg-skeleton wg-skeleton-line" />
            <div className="wg-skeleton wg-skeleton-line" />
          </div>
        ) : data === null || data.runningNow.length === 0 ? (
          <div className="wg-state">
            <span className="wg-state-lamp" aria-hidden="true" />
            <span>No model has taken over in this range</span>
          </div>
        ) : (
          <ul className="wg-mv-now-list">
            {data.runningNow.map((entry) => {
              const identity = {
                provider: entry.provider,
                modelId: entry.modelId,
              } satisfies ModelIdentity;
              const since = formatSince(entry.runningSinceAt, nowMs);
              const replaced =
                entry.previousModelId === null
                  ? null
                  : `replaced ${entry.previousModelId}`;
              return (
                <li
                  className="wg-mv-now"
                  key={modelIdentityKey(identity)}
                  title={`${entry.modelId} · running since ${since}${
                    replaced === null ? "" : ` · ${replaced}`
                  }`}
                >
                  <div className="wg-mv-now-line">
                    <span
                      className="wg-row-chip"
                      data-tone={chipTone(entry.provider)}
                    >
                      {providerTag(entry.provider)}
                    </span>
                    <ModelSwatch color={shadeFor(shades, identity)} />
                    <bdi className="wg-mv-id" dir="ltr" translate="no">
                      {entry.modelId}
                    </bdi>
                    <span className="wg-mv-ago wg-num">
                      {formatRecency(entry.lastSeenAt, nowMs)}
                    </span>
                  </div>
                  <p className="wg-mv-since wg-num">
                    since {since}
                    {replaced !== null && ` · ${replaced}`}
                  </p>
                </li>
              );
            })}
          </ul>
        )}
      </section>

      <div className="wg-rule" />

      <section className="wg-mv-band" aria-label="Models by sessions">
        <BandHead title="By sessions" meta={rankedMeta} />
        {loading ? (
          <div className="wg-mv-pending" aria-hidden="true">
            <div className="wg-skeleton wg-skeleton-line" />
            <div className="wg-skeleton wg-skeleton-line" />
            <div className="wg-skeleton wg-skeleton-line" />
          </div>
        ) : ranked.length === 0 ? (
          <div className="wg-state">
            <span className="wg-state-lamp" aria-hidden="true" />
            <span>{data === null ? "No model evidence" : emptyClaim(data, status)}</span>
          </div>
        ) : (
          <ul className="wg-mv-rank">
            {ranked.map((row) => {
              const shade = shadeFor(shades, row.identity);
              const percent =
                sessionsCeiling > 0 ? (row.sessions / sessionsCeiling) * 100 : 0;
              const tokens = tokenReading(row);
              return (
                <li
                  className="wg-mv-row"
                  key={modelIdentityKey(row.identity)}
                  title={`${row.identity.modelId} · ${formatNumber(
                    row.sessions,
                  )} sessions · ${formatNumber(
                    row.projects,
                  )} projects · ${formatNumber(row.turns)} turns`}
                >
                  <div className="wg-mv-row-top">
                    <ModelSwatch color={shade} />
                    <bdi className="wg-mv-id" dir="ltr" translate="no">
                      {row.identity.modelId}
                    </bdi>
                    <span className="wg-mv-tokens wg-num" title={tokens.absence ?? undefined}>
                      {tokens.text}
                    </span>
                  </div>
                  <div className="wg-mv-row-bar">
                    <span
                      className="wg-row-chip"
                      data-tone={chipTone(row.identity.provider)}
                    >
                      {providerTag(row.identity.provider)}
                    </span>
                    <span
                      className="wg-mv-track"
                      role="progressbar"
                      aria-label={`${row.identity.modelId} sessions`}
                      aria-valuenow={row.sessions}
                      aria-valuemin={0}
                      aria-valuemax={sessionsCeiling}
                      aria-valuetext={`${formatNumber(row.sessions)} of ${formatNumber(
                        sessionsCeiling,
                      )} sessions`}
                    >
                      <i
                        className="wg-mv-fill"
                        style={{ width: `${percent}%`, background: shade }}
                      />
                    </span>
                    <span className="wg-mv-count wg-num">
                      {formatNumber(row.sessions)}
                    </span>
                  </div>
                </li>
              );
            })}
          </ul>
        )}

        {/* Coverage is attributed tokens over all token-bearing observations.
            Anything short of 100% means activity ran before any model evidence
            existed, and that gap is stated rather than rounded away. */}
        {coverage !== null && coverage < 100 && (
          <p
            className="wg-mv-note wg-num"
            role="note"
            title="Token activity recorded before a chain's first model observation stays unattributed instead of being assigned a model."
          >
            Coverage · {coverage}% of token activity carries model evidence
          </p>
        )}

        {modelIndexNote !== null && (
          <p className="wg-mv-note wg-num" role="status" aria-live="polite">
            {modelIndexNote}
          </p>
        )}

        {backfillNeedsAttention && (
          <p className="wg-mv-note wg-num" role="note">
            Retained history · {BACKFILL_LABELS[status.status]} · sources{" "}
            {formatNumber(status.processedSources)}/
            {formatNumber(status.totalSources)}
            {backfillRetryable && (
              <button
                type="button"
                className="wg-mv-retry"
                onClick={backfill.retry}
                disabled={backfill.isRetrying}
              >
                {backfill.isRetrying ? "Retrying…" : "Retry"}
              </button>
            )}
          </p>
        )}

        {/* A refused retry states its reason here rather than disappearing —
            the operator asked for the work, so they get its outcome. */}
        {backfill.retryError !== null && (
          <p className="wg-mv-note wg-mv-note-alert" role="alert">
            {backfill.retryError.message}
          </p>
        )}
      </section>
    </div>
  );
}

export default ModelsView;
