// Insight line selection — which single computed insight the Usage view's hero
// band states for the window the user has selected.
//
// v1 pinned the line to context savings. A pinned line goes silent on every
// window where Quill's own context store happened to do nothing, so the set has
// to rotate — and a rotating set needs a stated rule, because an insight picked
// by taste is an insight the reader cannot audit (constitution #1).
//
// The rule, in four clauses:
//
//   1. **Restatement only.** Every candidate restates figures the widget
//      already read from local storage for this same window. Nothing here
//      derives a metric that has no source behind it.
//   2. **Eligible or absent.** A candidate offers a sentence only when its
//      figure exists and is non-zero for the window. Nothing is zeroed,
//      padded, or extrapolated to keep the line occupied; with no eligible
//      candidate the line is simply not drawn.
//   3. **First eligible in a fixed order wins.** The order is by how much of
//      the story the rest of the widget does not already tell: context savings
//      appears nowhere else on the surface; the cached-token volume is only
//      implied by the footer's percentage; the provider split is already drawn
//      by the chart directly above, so it speaks last.
//   4. **A pending source holds the line.** While a higher-priority candidate's
//      source has not answered for this window, the line stays empty rather
//      than showing a lower-priority insight that would be swapped out a moment
//      later. A source that failed counts as answered-with-nothing, so one
//      broken read cannot mute the whole line.
//
// Selection is therefore a pure function of the window and its resolved data:
// the same window with the same data always yields the same line, no clock or
// counter ever rotates it under the reader, and a screenshot of a given window
// is reproducible.

import { formatNumber } from "../../../utils/format";
import { formatTokenCount } from "../../../utils/tokens";

/** Stable identity of each candidate, in priority order. */
export type InsightId = "context-savings" | "cache-reuse" | "provider-mix";

/** The chosen sentence: a bolded figure and an optional supporting clause. */
export interface InsightLine {
  readonly id: InsightId;
  readonly headline: string;
  readonly detail: string | null;
}

/** One provider's token total for the window, already labelled by the caller. */
export interface ProviderTotal {
  readonly label: string;
  readonly tokens: number;
}

/**
 * The window's resolved figures, as the Usage view already holds them.
 *
 * Each group carries `loading` alongside its figures because the hooks retain
 * the previous window's answer across a refresh: a source is pending only when
 * it is loading *and* has nothing to state yet.
 */
export interface InsightInputs {
  /** Tokens Quill's own context store kept out of the prompt. */
  readonly savings: {
    readonly tokensSaved: number | null;
    /** Share of preserved sources later read back, when any were preserved. */
    readonly reusePercent: number | null;
    readonly loading: boolean;
  };
  /** Prompt-cache reuse, the same figures the footer's `Cache` cell reads. */
  readonly cache: {
    readonly tokensFromCache: number | null;
    readonly percentOfInput: number | null;
    readonly loading: boolean;
  };
  /** Per-provider totals behind the hero chart, one entry per plotted series. */
  readonly providers: {
    readonly totals: readonly ProviderTotal[] | null;
    readonly loading: boolean;
  };
}

interface Candidate {
  readonly id: InsightId;
  /** True while this candidate's source has yet to answer for the window. */
  readonly pending: (inputs: InsightInputs) => boolean;
  /** The sentence, or null when this candidate has nothing true to say. */
  readonly build: (inputs: InsightInputs) => InsightLine | null;
}

/**
 * Candidates in priority order. Adding one is a deliberate act: it must
 * restate a figure the widget already reads for the selected window, and it
 * must go silent rather than round to zero.
 */
const CANDIDATES: readonly Candidate[] = [
  {
    id: "context-savings",
    pending: ({ savings }) => savings.loading && savings.tokensSaved === null,
    build: ({ savings }) => {
      const saved = savings.tokensSaved;
      if (saved === null || saved <= 0) return null;
      return {
        id: "context-savings",
        headline: `${formatTokenCount(saved)} tokens saved`,
        detail:
          savings.reusePercent === null
            ? null
            : `${savings.reusePercent}% of preserved sources reused`,
      };
    },
  },
  {
    id: "cache-reuse",
    pending: ({ cache }) => cache.loading && cache.tokensFromCache === null,
    build: ({ cache }) => {
      const fromCache = cache.tokensFromCache;
      if (fromCache === null || fromCache <= 0) return null;
      return {
        id: "cache-reuse",
        headline: `${formatTokenCount(fromCache)} tokens served from cache`,
        detail:
          cache.percentOfInput === null ? null : `${cache.percentOfInput}% of input`,
      };
    },
  },
  {
    id: "provider-mix",
    pending: ({ providers }) => providers.loading && providers.totals === null,
    build: ({ providers }) => {
      const totals = providers.totals;
      if (!totals) return null;
      const active = totals.filter((entry) => entry.tokens > 0);
      // A lone provider's share is always 100% — a sentence that states the
      // obvious is worse than no sentence at all.
      if (active.length < 2) return null;
      const sum = active.reduce((running, entry) => running + entry.tokens, 0);
      if (sum <= 0) return null;
      const leader = active.reduce((best, entry) =>
        entry.tokens > best.tokens ? entry : best,
      );
      return {
        id: "provider-mix",
        headline: `${leader.label} drove ${Math.round((leader.tokens / sum) * 100)}% of tokens`,
        detail: `${formatNumber(active.length)} providers active`,
      };
    },
  },
];

/**
 * The insight line for this window, or null when no candidate has anything
 * true to say (or the first one that might is still loading).
 */
export function selectInsightLine(inputs: InsightInputs): InsightLine | null {
  for (const candidate of CANDIDATES) {
    if (candidate.pending(inputs)) return null;
    const line = candidate.build(inputs);
    if (line) return line;
  }
  return null;
}
