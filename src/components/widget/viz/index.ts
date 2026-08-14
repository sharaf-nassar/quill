// Widget viz kit — the internal SVG primitives every widget band draws with.
// Deliberately not a charting library: two shapes, no runtime dependency, and
// full control over the Flat Polish treatments (surface-faded overlay,
// hover-only legend chip, endpoint markers) that a generic library fights.
//
// The barrel re-exports only what widget views consume. Symbols used
// solely between the primitives themselves (`seriesTotal`, the prop types) stay
// module-local so the kit's public surface cannot drift back into dead code.

export { default as AreaChart } from "./AreaChart";
export type { VizSeries } from "./AreaChart";
export { default as Sparkline } from "./Sparkline";
export { bucketTotals } from "./geometry";
