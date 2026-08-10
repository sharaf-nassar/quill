// ViewRegion — everything below LIMITS.
//
// The region owns the two pieces of state every view shares: which view is
// showing and which time range is selected. Both live here rather than inside
// a view so that switching views keeps the operator's range — the mockup's
// band header is one control strip, not one per view — and so a new compact
// view is registered by adding a single entry to `VIEWS` instead of touching
// the shell.
//
// See specs/018-widget-ui-redesign/plan.md#Affected Components.

import { useEffect, useState, type ReactNode } from "react";
import type { RangeType } from "../../types";
import { useCachedInvokeEvents } from "../../hooks/useCachedInvokeEvents";
import {
  readStoredWidgetRange,
  storeWidgetRange,
  WIDGET_RANGES,
} from "./rangePreference";
import ViewSwitcher, { type ViewOption, type WidgetView } from "./ViewSwitcher";
import ChartsView from "./views/ChartsView";
import ContextView from "./views/ContextView";
import ModelsView from "./views/ModelsView";
import TrendsView from "./views/TrendsView";
import UsageView from "./views/UsageView";

interface ViewDefinition extends ViewOption {
  render: (range: RangeType) => ReactNode;
  /**
   * Whether the shared range strip applies. A view that fixes its own windows
   * (Trends compares whole weeks) sets this false, and the strip is hidden
   * rather than left standing as a control that would change nothing.
   */
  ranged?: boolean;
}

/**
 * The view registry. Compact views (Trends, Charts, Models, Context) append
 * their entry here as they land; only registered views reach the dropdown, so
 * the list never offers a view that would render nothing.
 */
const VIEWS: readonly ViewDefinition[] = [
  {
    id: "usage",
    label: "Usage",
    render: (range) => <UsageView range={range} />,
  },
  {
    id: "trends",
    label: "Trends",
    ranged: false,
    render: () => <TrendsView />,
  },
  {
    id: "charts",
    label: "Charts",
    render: (range) => <ChartsView range={range} />,
  },
  {
    id: "models",
    label: "Models",
    render: (range) => <ModelsView range={range} />,
  },
  {
    id: "context",
    label: "Context",
    render: (range) => <ContextView range={range} />,
  },
];

function ViewRegion() {
  useCachedInvokeEvents();
  const [view, setView] = useState<WidgetView>("usage");
  const [range, setRange] = useState<RangeType>(() => readStoredWidgetRange());

  useEffect(() => storeWidgetRange(range), [range]);

  const active = VIEWS.find((entry) => entry.id === view) ?? VIEWS[0];

  return (
    <section className="wg-view" aria-label={`${active.label} view`}>
      {/* Three tracks so the range strip is centred on the widget, not on the
          space the view name happens to leave over. */}
      <div className="wg-view-head">
        <ViewSwitcher options={VIEWS} view={active.id} onSelect={setView} />
        {active.ranged === false ? (
          <span />
        ) : (
          <div className="wg-toggles wg-num" role="group" aria-label="Time range">
            {WIDGET_RANGES.map((entry) => (
              <button
                key={entry.id}
                type="button"
                className="wg-toggle"
                aria-pressed={entry.id === range}
                onClick={() => setRange(entry.id)}
              >
                {entry.label}
              </button>
            ))}
          </div>
        )}
        <span />
      </div>
      {active.render(range)}
    </section>
  );
}

export default ViewRegion;
