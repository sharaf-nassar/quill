// The Usage Explorer's shared plane: every visible series rescaled to 0..1
// and overlaid on one daily grid, so shapes can be compared across units
// (tokens vs hours vs line counts). Interactions: hover crosshair with a
// value tooltip, click-to-pin a day, arrow-key day walking, and emphasis
// (nearest line under the cursor, or a set supplied by the parent when a
// panel row / correlation chip is hovered).
//
// Colors arrive as literal hex (never CSS vars) because they also feed SVG
// gradient stops and endpoint labels.

import {
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from "react";

export interface ExploreChartSeries {
  readonly id: string;
  readonly label: string;
  readonly color: string;
  /** Daily values, already smoothed by the parent when smoothing is on. */
  readonly values: readonly number[];
  readonly format: (value: number) => string;
}

export type ExploreScale = "norm" | "index";

interface ExploreChartProps {
  /** ISO bucket starts, oldest first. */
  grid: readonly string[];
  series: readonly ExploreChartSeries[];
  scale: ExploreScale;
  /** Series ids to emphasize from outside (panel row / correlation hover). */
  emphasized: readonly string[] | null;
}

const PAD = { l: 36, r: 76, t: 12, b: 24 } as const;
/** Cursor must be this close (px) to a line before it becomes the hot one. */
const HOT_DISTANCE_PX = 28;

/** Rescale a series to the shared 0..1 plane. */
function scaled(values: readonly number[], mode: ExploreScale): number[] {
  if (mode === "norm") {
    const min = Math.min(...values);
    const max = Math.max(...values);
    const span = max - min || 1;
    return values.map((value) => (value - min) / span);
  }
  // Indexed: 0.5 = the series mean; swings are relative to it.
  const mean = values.reduce((sum, value) => sum + value, 0) / values.length || 1;
  const relative = values.map((value) => value / Math.abs(mean));
  const max = Math.max(...relative.map(Math.abs), 2);
  return relative.map((value) => 0.5 + (value - 1) / (2 * max));
}

/** Catmull-Rom → cubic bezier path for smooth lines. */
function smoothPath(points: ReadonlyArray<readonly [number, number]>): string {
  if (points.length < 3) {
    return `M${points.map((point) => point.join(",")).join("L")}`;
  }
  let path = `M${points[0][0]},${points[0][1]}`;
  for (let index = 0; index < points.length - 1; index += 1) {
    const p0 = points[Math.max(0, index - 1)];
    const p1 = points[index];
    const p2 = points[index + 1];
    const p3 = points[Math.min(points.length - 1, index + 2)];
    const c1x = p1[0] + (p2[0] - p0[0]) / 6;
    const c1y = p1[1] + (p2[1] - p0[1]) / 6;
    const c2x = p2[0] - (p3[0] - p1[0]) / 6;
    const c2y = p2[1] - (p3[1] - p1[1]) / 6;
    path += `C${c1x.toFixed(1)},${c1y.toFixed(1)} ${c2x.toFixed(1)},${c2y.toFixed(1)} ${p2[0].toFixed(1)},${p2[1].toFixed(1)}`;
  }
  return path;
}

interface Plot {
  series: ExploreChartSeries;
  points: Array<[number, number]>;
  lineD: string;
  areaD: string;
  labelY: number;
}

function ExploreChart({ grid, series, scale, emphasized }: ExploreChartProps) {
  const wrapRef = useRef<HTMLDivElement | null>(null);
  const svgRef = useRef<SVGSVGElement | null>(null);
  const [size, setSize] = useState({ width: 0, height: 0 });
  const [hover, setHover] = useState<{ index: number; hot: string | null } | null>(
    null,
  );
  const [pinned, setPinned] = useState<number | null>(null);
  const [tipPos, setTipPos] = useState<{ x: number; y: number } | null>(null);

  const count = grid.length;

  useLayoutEffect(() => {
    const node = wrapRef.current;
    if (!node) return;
    // Measure synchronously so the first paint is never a zero-width frame;
    // the observer then owns every later resize.
    const rect = node.getBoundingClientRect();
    setSize({ width: rect.width, height: rect.height });
    const observer = new ResizeObserver((entries) => {
      const entry = entries[0]?.contentRect;
      if (entry) setSize({ width: entry.width, height: entry.height });
    });
    observer.observe(node);
    return () => observer.disconnect();
  }, []);

  // A range switch changes the grid length; a stale pin would name the wrong
  // day, so it is clamped into the new grid rather than silently kept.
  useEffect(() => {
    setPinned((current) =>
      current === null ? null : Math.min(current, Math.max(0, count - 1)),
    );
    setHover(null);
  }, [count]);

  const { width, height } = size;
  const innerWidth = Math.max(1, width - PAD.l - PAD.r);
  const innerHeight = Math.max(1, height - PAD.t - PAD.b);
  const x = (index: number) =>
    PAD.l + (count > 1 ? (index / (count - 1)) * innerWidth : innerWidth / 2);
  const y = (value: number) => PAD.t + (1 - value) * innerHeight;

  const plots = useMemo<Plot[]>(() => {
    if (width === 0 || count === 0) return [];
    const built = series.map((entry) => {
      const norm = scaled(entry.values, scale);
      const points = norm.map(
        (value, index) =>
          [x(index), y(Math.max(0, Math.min(1, value)))] as [number, number],
      );
      const lineD = smoothPath(points);
      const areaD = `${lineD}L${points[points.length - 1][0]},${y(0)}L${points[0][0]},${y(0)}Z`;
      return {
        series: entry,
        points,
        lineD,
        areaD,
        labelY: points[points.length - 1][1],
      };
    });
    // Endpoint labels nudge apart on collision, top to bottom.
    const ordered = [...built].sort((a, b) => a.labelY - b.labelY);
    for (let index = 1; index < ordered.length; index += 1) {
      if (ordered[index].labelY - ordered[index - 1].labelY < 12) {
        ordered[index].labelY = ordered[index - 1].labelY + 12;
      }
    }
    return built;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [series, scale, width, height, count]);

  // Fills fade as more series stack so the midfield stays readable.
  const fillAlpha = Math.min(0.14, 0.36 / Math.max(1, plots.length));

  const activeIndex = pinned ?? hover?.index ?? null;
  const emphasisSet = useMemo<ReadonlySet<string> | null>(() => {
    if (hover?.hot) return new Set([hover.hot]);
    if (emphasized && emphasized.length > 0) return new Set(emphasized);
    return null;
  }, [hover, emphasized]);

  const indexFromClientX = (clientX: number): number => {
    const rect = svgRef.current?.getBoundingClientRect();
    if (!rect || count === 0) return 0;
    const px = clientX - rect.left;
    return Math.max(
      0,
      Math.min(count - 1, Math.round(((px - PAD.l) / innerWidth) * (count - 1))),
    );
  };

  const handleMouseMove = (event: React.MouseEvent<SVGSVGElement>) => {
    if (pinned !== null || plots.length === 0) return;
    const rect = svgRef.current?.getBoundingClientRect();
    if (!rect) return;
    const index = indexFromClientX(event.clientX);
    const py = event.clientY - rect.top;
    let hot: string | null = null;
    let best = HOT_DISTANCE_PX;
    for (const plot of plots) {
      const distance = Math.abs(plot.points[index][1] - py);
      if (distance < best) {
        best = distance;
        hot = plot.series.id;
      }
    }
    setHover({ index, hot });
    setTipPos({
      x: event.clientX - rect.left + 16,
      y: event.clientY - rect.top + 16,
    });
  };

  const handleClick = (event: React.MouseEvent<SVGSVGElement>) => {
    if (plots.length === 0) return;
    const index = indexFromClientX(event.clientX);
    if (pinned === index) {
      setPinned(null);
      setHover(null);
    } else {
      setPinned(index);
      setHover({ index, hot: null });
      setTipPos({ x: x(index) + 16, y: PAD.t + 8 });
    }
  };

  const handleKeyDown = (event: React.KeyboardEvent<SVGSVGElement>) => {
    if (plots.length === 0 || count === 0) return;
    const current = activeIndex ?? count - 1;
    let next: number | null = null;
    if (event.key === "ArrowLeft") next = Math.max(0, current - 1);
    else if (event.key === "ArrowRight") next = Math.min(count - 1, current + 1);
    else if (event.key === "Home") next = 0;
    else if (event.key === "End") next = count - 1;
    else if (event.key === "Enter" || event.key === " ") {
      setPinned((value) => (value === current ? null : current));
      event.preventDefault();
      return;
    } else if (event.key === "Escape") {
      setPinned(null);
      setHover(null);
      return;
    } else {
      return;
    }
    event.preventDefault();
    if (pinned !== null) setPinned(next);
    setHover({ index: next, hot: null });
    setTipPos({ x: x(next) + 16, y: PAD.t + 8 });
  };

  const tickEvery = count > 30 ? 14 : count > 7 ? 7 : 1;
  const dates = useMemo(() => grid.map((iso) => new Date(iso)), [grid]);

  const tooltipRows =
    activeIndex === null
      ? []
      : [...plots].sort(
          (a, b) => a.points[activeIndex][1] - b.points[activeIndex][1],
        );

  return (
    <div className="explore-chart" ref={wrapRef}>
      <svg
        ref={svgRef}
        width="100%"
        height="100%"
        viewBox={`0 0 ${Math.max(1, width)} ${Math.max(1, height)}`}
        role="application"
        tabIndex={0}
        aria-label="Usage overlay chart. Use arrow keys to inspect days, Enter to pin, Escape to release."
        onMouseMove={handleMouseMove}
        onMouseLeave={() => {
          if (pinned === null) setHover(null);
        }}
        onClick={handleClick}
        onKeyDown={handleKeyDown}
        onBlur={() => {
          if (pinned === null) setHover(null);
        }}
      >
        <defs>
          {plots.map((plot) => (
            <linearGradient
              key={plot.series.id}
              id={`explore-grad-${plot.series.id.replace(/[^a-zA-Z0-9_-]/g, "_")}`}
              x1="0"
              y1="0"
              x2="0"
              y2="1"
            >
              <stop offset="0%" stopColor={plot.series.color} stopOpacity={fillAlpha} />
              <stop offset="100%" stopColor={plot.series.color} stopOpacity={0} />
            </linearGradient>
          ))}
        </defs>

        {/* Weekend bands: the weekly rhythm reads at a glance. */}
        {width > 0 &&
          dates.map((date, index) => {
            if (date.getDay() !== 6) return null;
            const dayWidth = count > 1 ? innerWidth / (count - 1) : innerWidth;
            const from = Math.max(PAD.l, x(index) - dayWidth / 2);
            const to = Math.min(
              width - PAD.r,
              x(Math.min(count - 1, index + 1)) + dayWidth / 2,
            );
            return (
              <rect
                key={`weekend-${grid[index]}`}
                x={from}
                y={PAD.t}
                width={Math.max(0, to - from)}
                height={innerHeight}
                fill="rgba(255,255,255,0.018)"
              />
            );
          })}

        {/* Horizontal quarters; % labels for min-max, an avg line for indexed. */}
        {[0, 0.25, 0.5, 0.75, 1].map((fraction) => {
          const mid = scale === "index" && fraction === 0.5;
          return (
            <g key={fraction}>
              <line
                x1={PAD.l}
                x2={Math.max(PAD.l, width - PAD.r)}
                y1={y(fraction)}
                y2={y(fraction)}
                stroke={mid ? "rgba(255,255,255,0.14)" : "rgba(255,255,255,0.05)"}
                strokeDasharray={mid ? "2 4" : undefined}
              />
              {scale === "norm" ? (
                <text
                  x={PAD.l - 7}
                  y={y(fraction) + 3}
                  className="explore-axis-label"
                  textAnchor="end"
                >
                  {Math.round(fraction * 100)}%
                </text>
              ) : mid ? (
                <text
                  x={PAD.l - 7}
                  y={y(fraction) + 3}
                  className="explore-axis-label"
                  textAnchor="end"
                >
                  avg
                </text>
              ) : null}
            </g>
          );
        })}

        {/* X ticks */}
        {width > 0 &&
          dates.map((date, index) => {
            if (index % tickEvery !== 0) return null;
            const label =
              count <= 7
                ? date.toLocaleDateString(undefined, { weekday: "short" })
                : date.toLocaleDateString(undefined, {
                    month: "short",
                    day: "numeric",
                  });
            return (
              <g key={`tick-${grid[index]}`}>
                <line
                  x1={x(index)}
                  x2={x(index)}
                  y1={PAD.t}
                  y2={height - PAD.b}
                  stroke="rgba(255,255,255,0.03)"
                />
                <text
                  x={x(index)}
                  y={height - 8}
                  className="explore-tick-label"
                  textAnchor="middle"
                >
                  {label}
                </text>
              </g>
            );
          })}

        {/* Series */}
        {plots.map((plot) => {
          const dim = emphasisSet !== null && !emphasisSet.has(plot.series.id);
          const hot = emphasisSet !== null && emphasisSet.has(plot.series.id);
          const gradId = `explore-grad-${plot.series.id.replace(/[^a-zA-Z0-9_-]/g, "_")}`;
          const last = plot.points[plot.points.length - 1];
          return (
            <g
              key={plot.series.id}
              className="explore-series-group"
              style={{ opacity: dim ? 0.18 : 1 }}
            >
              <path d={plot.areaD} fill={`url(#${gradId})`} stroke="none" />
              <path
                d={plot.lineD}
                fill="none"
                stroke={plot.series.color}
                strokeWidth={hot ? 2.4 : 1.7}
                strokeLinejoin="round"
                strokeLinecap="round"
              />
              <circle cx={last[0]} cy={last[1]} r={2.6} fill={plot.series.color} />
              <text
                x={width - PAD.r + 8}
                y={plot.labelY + 3}
                className="explore-endpoint-label"
                fill={plot.series.color}
              >
                {plot.series.format(
                  plot.series.values[plot.series.values.length - 1] ?? 0,
                )}
              </text>
              {activeIndex !== null &&
                (emphasisSet === null || emphasisSet.has(plot.series.id)) && (
                  <circle
                    cx={plot.points[activeIndex][0]}
                    cy={plot.points[activeIndex][1]}
                    r={3}
                    fill={plot.series.color}
                    stroke="#0d1117"
                    strokeWidth={1.5}
                  />
                )}
            </g>
          );
        })}

        {/* Crosshair */}
        {activeIndex !== null && (
          <line
            x1={x(activeIndex)}
            x2={x(activeIndex)}
            y1={PAD.t}
            y2={height - PAD.b}
            stroke={
              pinned !== null ? "rgba(96,165,250,0.55)" : "rgba(255,255,255,0.22)"
            }
            strokeDasharray={pinned !== null ? undefined : "3 3"}
          />
        )}
      </svg>

      {activeIndex !== null && tipPos !== null && tooltipRows.length > 0 && (
        <div
          className="explore-tip"
          style={{
            left: Math.min(Math.max(0, width - 200), tipPos.x),
            top: Math.min(Math.max(0, height - 24 * tooltipRows.length - 48), tipPos.y),
          }}
        >
          <div className="explore-tip-date">
            {dates[activeIndex]?.toLocaleDateString(undefined, {
              weekday: "short",
              month: "short",
              day: "numeric",
            })}
            {pinned !== null && <span className="explore-tip-pin">PINNED</span>}
          </div>
          {tooltipRows.map((plot) => (
            <div
              className="explore-tip-row"
              data-hot={hover?.hot === plot.series.id ? "true" : undefined}
              key={plot.series.id}
            >
              <i style={{ background: plot.series.color }} />
              {plot.series.label}
              <b>{plot.series.format(plot.series.values[activeIndex] ?? 0)}</b>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

export default ExploreChart;
