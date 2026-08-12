import assert from "node:assert/strict";
import test from "node:test";
import {
  bucketTotals,
  bucketIndexAtPosition,
  legendPositionAtPosition,
} from "../src/components/widget/viz/geometry.ts";

// @lat: [[widget-viz-tests#Widget Viz Tests#Bucket Series Totals]]
test("bucket totals align uneven provider series", () => {
  assert.deepEqual(
    bucketTotals([{ values: [1, 2, 3] }, { values: [4, 5] }]),
    [5, 7, 3],
  );
  assert.deepEqual(bucketTotals([]), []);
});

// @lat: [[widget-viz-tests#Widget Viz Tests#Pointer Scrub Bucket Mapping]]
test("pointer positions select buckets and place the floating legend", () => {
  assert.deepEqual(
    [-1, 0, 41.49, 41.5, 166, 331.99, 332, 400].map((x) =>
      bucketIndexAtPosition(x, 332, 8),
    ),
    [0, 0, 0, 1, 4, 7, 7, 7],
  );
  assert.equal(bucketIndexAtPosition(1, 0, 8), null);
  assert.equal(bucketIndexAtPosition(1, 332, 0), null);
  assert.deepEqual(legendPositionAtPosition(-20, -20, 332, 118, 120, 32), {
    left: 8,
    top: 0,
    side: "after",
  });
  assert.deepEqual(legendPositionAtPosition(166, 50, 332, 118, 120, 32), {
    left: 174,
    top: 58,
    side: "after",
  });
  assert.deepEqual(legendPositionAtPosition(400, 110, 332, 118, 120, 32), {
    left: 204,
    top: 86,
    side: "before",
  });
  assert.equal(legendPositionAtPosition(1, 1, 0, 118, 120, 32), null);
});
