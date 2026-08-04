// ContextView — what Quill's working-context store did with the selected
// range, in the order an operator asks for it: how much text was kept out of
// the transcript and how much of it came back (the headline pair), what that
// was worth (the shared cache-savings line), and what it cost (routing).
//
// Two rules this file obeys:
//
//   - **No chart.** `summary` and `timeSeries` are computed from different
//     token columns — the category-scoped totals the savings taxonomy
//     introduced versus the legacy per-bucket estimates that also counted
//     telemetry. Plotting the series beneath these headlines would put two
//     numbers that disagree in one band, and a headline the graphic
//     contradicts is worse than no graphic (constitution #1). The split bar
//     is the only visualization, and it is drawn from the exact three figures
//     printed around it.
//   - **Category totals only, never the legacy columns.** A backend that does
//     not report categories reads as zero here and says so, rather than
//     falling back to the pre-taxonomy figure that counted telemetry as
//     savings. Silence beats a confident wrong number.
//
// See specs/018-widget-ui-redesign/plan.md#Affected Components.

import { contextSavingsInsight } from "./insightLine";
import { useContextSavingsStats } from "../../../hooks/useContextSavingsStats";
import { formatBytes, formatNumber } from "../../../utils/format";
import { formatTokenCount } from "../../../utils/tokens";
import type { ContextSavingsSummary, RangeType } from "../../../types";

/**
 * Tokens written to local storage instead of staying in the live transcript.
 *
 * Deliberately no fallback to `tokensPreservedEst`: that column counted
 * telemetry events too, and quoting it here would silently re-inflate the very
 * headline the category taxonomy exists to correct.
 */
function preservedTokens(summary: ContextSavingsSummary): number {
  return summary.tokensPreserved ?? 0;
}

/** Tokens read back out of the store on demand. */
function retrievedTokens(summary: ContextSavingsSummary): number {
  return summary.tokensRetrieved ?? 0;
}

/** Transcript tokens spent on the nudges and snippets that do the routing. */
function routingTokens(summary: ContextSavingsSummary): number {
  return summary.tokensRouting ?? 0;
}

/**
 * The count that matches `routingTokens` — router decisions plus capture
 * guidance, search snippets and bounded results.
 *
 * The hook normalizes an absent category count to 0, so "not reported" and
 * "genuinely none" arrive here indistinguishable. A zero alongside non-zero
 * routing tokens can only be the former, so it falls back to the
 * event-type-scoped `routerEventCount` — the closest honest stand-in — and
 * returns null when neither count exists. The caller drops the clause rather
 * than printing a count that contradicts the value beside it.
 */
function routingEventCount(summary: ContextSavingsSummary): number | null {
  const scoped = summary.routingEventCount ?? 0;
  if (scoped > 0) return scoped;
  const fallback = summary.routerEventCount ?? 0;
  return fallback > 0 ? fallback : null;
}

/** Share of preserved sources later read back; null when none were preserved. */
function reusePercent(summary: ContextSavingsSummary): number | null {
  if ((summary.sourcesPreserved ?? 0) <= 0) return null;
  return Math.round((summary.retentionRatio ?? 0) * 100);
}

interface Category {
  readonly id: string;
  readonly label: string;
  readonly hue: string;
  readonly tokens: number;
}

/** One half of the headline pair: value, hue-swatched key, byte sub-line. */
function Headline({ category, sub }: { category: Category; sub: string }) {
  return (
    <div className="wg-ctx-head">
      <span className="wg-ctx-value">
        {formatTokenCount(category.tokens)}
        <span className="wg-ctx-unit">tokens</span>
      </span>
      <span className="wg-cell-key">
        <i
          className="wg-cell-swatch"
          style={{ background: category.hue }}
          aria-hidden="true"
        />
        {category.label}
      </span>
      <span className="wg-ctx-sub">{sub}</span>
    </div>
  );
}

/** Band-scoped states so the view's padding applies to every branch. */
function ContextState({ children, error }: { children: string; error?: boolean }) {
  return (
    <div className="wg-ctx-band">
      <div className={error ? "wg-state wg-state-error" : "wg-state"}>
        <span className="wg-state-lamp" aria-hidden="true" />
        {children}
      </div>
    </div>
  );
}

export interface ContextViewProps {
  range: RangeType;
}

function ContextView({ range }: ContextViewProps) {
  // Default event limit deliberately: it matches the Usage view's insight-line
  // read, so the two views share one backend analytics cache entry per range
  // and switching between them costs no query.
  const { data, loading, error } = useContextSavingsStats(range);
  const summary = data?.summary ?? null;

  if (!summary) {
    if (error) return <ContextState error>Context analytics unavailable</ContextState>;
    if (loading) {
      return (
        <div className="wg-ctx-band" aria-hidden="true">
          <div className="wg-ctx-pending">
            <div className="wg-skeleton wg-ctx-pending-head" />
            <div className="wg-skeleton wg-ctx-pending-head" />
          </div>
          <div className="wg-skeleton wg-ctx-pending-bar" />
        </div>
      );
    }
    return <ContextState>No context events in this range</ContextState>;
  }

  const preserved: Category = {
    id: "preserved",
    label: "Preserved",
    hue: "var(--context-preserved)",
    tokens: preservedTokens(summary),
  };
  const retrieved: Category = {
    id: "retrieved",
    label: "Retrieved",
    hue: "var(--context-retrieved)",
    tokens: retrievedTokens(summary),
  };
  const routing: Category = {
    id: "routing",
    label: "Routing cost",
    hue: "var(--context-routing)",
    tokens: routingTokens(summary),
  };
  const categories: readonly Category[] = [preserved, retrieved, routing];
  const accounted = categories.reduce((sum, entry) => sum + entry.tokens, 0);

  // Two different nothings, kept apart because they mean different things:
  // no context work happened in this range, or work happened and this backend
  // does not categorize its tokens (constitution #1 — gaps stay explicit).
  if (accounted <= 0) {
    return (
      <ContextState>
        {summary.eventCount > 0
          ? "Context events recorded, none carrying token categories"
          : "No context events in this range"}
      </ContextState>
    );
  }

  const splitLabel = `Context tokens by category — ${categories
    .map((entry) => `${entry.label} ${formatTokenCount(entry.tokens)}`)
    .join(", ")}`;

  // The savings sentence is built by the same function that feeds the Usage
  // view's insight line, so the two surfaces cannot state it differently. Here
  // it is unconditional rather than one candidate among several: on this view
  // the context store *is* the subject.
  const guidanceEvents = routingEventCount(summary);
  const savingsLine = contextSavingsInsight({
    tokensSaved: summary.tokensSavedEst,
    reusePercent: reusePercent(summary),
    loading,
  });

  return (
    <>
      <div className="wg-ctx-band">
        <div className="wg-ctx-heads wg-num">
          <Headline
            category={preserved}
            sub={`${formatBytes(summary.indexedBytes)} indexed`}
          />
          <Headline
            category={retrieved}
            sub={`${formatBytes(summary.returnedBytes)} returned`}
          />
        </div>

        {/* The one graphic, made of the same three numbers printed around it:
            how the range's accounted context tokens split by category. */}
        <div className="wg-ctx-split" role="img" aria-label={splitLabel}>
          {categories
            .filter((entry) => entry.tokens > 0)
            .map((entry) => (
              <i
                key={entry.id}
                className="wg-ctx-split-seg"
                style={{ flexGrow: entry.tokens, background: entry.hue }}
                title={`${entry.label} — ${formatTokenCount(entry.tokens)} tokens, ${Math.round(
                  (entry.tokens / accounted) * 100,
                )}% of accounted context tokens`}
              />
            ))}
        </div>

        {savingsLine && (
          <p className="wg-insight wg-num">
            <span className="wg-insight-glyph" aria-hidden="true">
              ◆
            </span>
            <span>
              <b>{savingsLine.headline}</b>
              {savingsLine.detail !== null && ` — ${savingsLine.detail}`}
            </span>
          </p>
        )}
      </div>

      <div className="wg-rule" />

      <section className="wg-ctx-routing" aria-label="Routing cost">
        <div className="wg-ctx-routing-top">
          <span className="wg-cell-value wg-num">
            {formatTokenCount(routing.tokens)}
            <span className="wg-ctx-unit">tokens</span>
          </span>
          {guidanceEvents !== null && (
            <span className="wg-ctx-routing-meta wg-num">
              {formatNumber(guidanceEvents)} guidance events
            </span>
          )}
        </div>
        <span className="wg-cell-key">
          <i
            className="wg-cell-swatch"
            style={{ background: routing.hue }}
            aria-hidden="true"
          />
          {routing.label}
        </span>
        <p className="wg-ctx-note">
          Transcript tokens spent on router nudges, capture guidance and bounded
          results — the overhead Quill adds to keep larger payloads out of the
          prompt.
        </p>
      </section>
    </>
  );
}

export default ContextView;
