// Widget viz kit — the internal SVG primitives every widget band draws with.
// Deliberately not a charting library: four shapes, no runtime dependency, and
// full control over the Flat Polish treatments (surface-faded overlay,
// hover-only legend chip, endpoint markers) that a generic library fights.

export { default as AreaChart } from "./AreaChart";
export type { AreaChartProps, VizSeries } from "./AreaChart";
export { default as Bars } from "./Bars";
export type { BarsProps, VizBar } from "./Bars";
export { default as Heat } from "./Heat";
export type { HeatProps, VizHeatCell } from "./Heat";
export { default as Sparkline } from "./Sparkline";
export type { SparklineProps } from "./Sparkline";
export { areaPath, scalePoints, seriesMax, seriesTotal, smoothPath } from "./geometry";
export type { ScaleOptions, VizPoint } from "./geometry";
