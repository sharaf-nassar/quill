// ViewRegion — everything below LIMITS.
//
// The region owns the two pieces of state every view shares: which view is
// showing and which time range is selected. Both live here rather than inside
// a view so that switching views keeps the operator's range and the band's
// shared control strip stays single.
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
import ContextView from "./views/ContextView";
import ModelsView from "./views/ModelsView";
import UsageView from "./views/UsageView";

interface ViewDefinition extends ViewOption {
  render: (range: RangeType) => ReactNode;
}

/** Only registered views reach the dropdown. */
const VIEWS: readonly ViewDefinition[] = [
  {
    id: "usage",
    label: "Usage",
    render: (range) => <UsageView range={range} />,
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
        <span />
      </div>
      {active.render(range)}
    </section>
  );
}

export default ViewRegion;
