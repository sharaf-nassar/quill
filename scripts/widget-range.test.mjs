import assert from "node:assert/strict";
import test from "node:test";
import {
  DEFAULT_WIDGET_RANGE,
  readStoredWidgetRange,
  resolveStoredWidgetRange,
  storeWidgetRange,
  WIDGET_RANGES,
} from "../src/components/widget/rangePreference.ts";

// @lat: [[widget-view-tests#Widget View Tests#Stored Range Preference]]
test("stored widget range accepts only widget scopes and degrades safely", () => {
  assert.deepEqual(
    WIDGET_RANGES.map(({ id }) => resolveStoredWidgetRange(id)),
    ["1h", "6h", "24h", "7d"],
  );
  for (const value of [null, "", "30d", "invalid"]) {
    assert.equal(resolveStoredWidgetRange(value), DEFAULT_WIDGET_RANGE);
  }
  assert.equal(
    readStoredWidgetRange({
      getItem() {
        throw new Error("storage unavailable");
      },
    }),
    DEFAULT_WIDGET_RANGE,
  );
  assert.doesNotThrow(() =>
    storeWidgetRange("6h", {
      setItem() {
        throw new Error("storage unavailable");
      },
    }),
  );
});
