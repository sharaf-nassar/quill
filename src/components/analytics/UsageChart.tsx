import {
  AreaChart,
  Area,
  Line,
  XAxis,
  YAxis,
  CartesianGrid,
  Tooltip,
  ReferenceLine,
  ResponsiveContainer,
  ComposedChart,
} from "recharts";
import { formatTokenCount } from "../../utils/tokens";
import type {
  DataPoint,
  TokenDataPoint,
  RangeType,
  MergedDataPoint,
} from "../../types";

interface TimestampedRecord {
  timestamp: string;
}

function dedupeTickLabels(
  data: TimestampedRecord[],
  formatter: (v: string) => string,
): Set<number> {
  const seen = new Set<string>();
  const allowed = new Set<number>();
  for (let i = 0; i < data.length; i++) {
    const label = formatter(data[i].timestamp);
    if (!seen.has(label)) {
      seen.add(label);
      allowed.add(i);
    }
  }
  return allowed;
}

function formatTime(timestamp: string, range: RangeType): string {
  const d = new Date(timestamp);
  if (range === "1h" || range === "24h") {
    return d.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
  }
  if (range === "7d") {
    return d.toLocaleDateString([], { weekday: "short", hour: "2-digit" });
  }
  return d.toLocaleDateString([], { month: "short", day: "numeric" });
}

function getAreaColor(data: DataPoint[]): string {
  if (!data || data.length === 0) return "#34d399";
  const latest = data[data.length - 1].utilization;
  if (latest >= 80) return "#f87171";
  if (latest >= 50) return "#fbbf24";
  return "#34d399";
}

interface TooltipPayloadEntry {
  dataKey: string;
  value: number;
}

interface CustomTooltipProps {
  active?: boolean;
  payload?: TooltipPayloadEntry[];
  label?: string;
}

function CustomTooltip({ active, payload, label }: CustomTooltipProps) {
  if (!active || !payload || payload.length === 0) return null;

  const time = new Date(label!);
  const utilEntry = payload.find((p) => p.dataKey === "utilization");
  const tokenEntry = payload.find((p) => p.dataKey === "total_tokens");

  return (
    <div className="chart-tooltip">
      <div className="chart-tooltip-time">
        {time.toLocaleString([], {
          month: "short",
          day: "numeric",
          hour: "2-digit",
          minute: "2-digit",
        })}
      </div>
      {utilEntry && (
        <div
          className="chart-tooltip-value"
          style={{
            color:
              utilEntry.value >= 80
                ? "#f87171"
                : utilEntry.value >= 50
                  ? "#fbbf24"
                  : "#34d399",
          }}
        >
          {utilEntry.value.toFixed(1)}%
        </div>
      )}
      {tokenEntry && tokenEntry.value > 0 && (
        <div className="chart-tooltip-value" style={{ color: "#60a5fa" }}>
          {formatTokenCount(tokenEntry.value)} tokens
        </div>
      )}
    </div>
  );
}

interface UsageChartProps {
  data: DataPoint[];
  range: RangeType;
  bucket: string;
  tokenData: TokenDataPoint[];
}

/** Minimum gap (ms) before we append a "now" anchor to extend the X-axis */
const NOW_ANCHOR_THRESHOLD_MS = 2 * 60 * 1000; // 2 minutes

/**
 * If the last data point is older than NOW_ANCHOR_THRESHOLD_MS, append a
 * sentinel point at "now" so the chart X-axis extends to the current time
 * and idle gaps are visible rather than compressed to the right edge.
 */
function anchorToNow<T extends { timestamp: string }>(
  points: T[],
  defaults: Omit<T, "timestamp">,
): T[] {
  if (points.length === 0) return points;

  const lastTs = new Date(points[points.length - 1].timestamp).getTime();
  const now = Date.now();

  if (now - lastTs > NOW_ANCHOR_THRESHOLD_MS) {
    return [
      ...points,
      { ...defaults, timestamp: new Date(now).toISOString() } as T,
    ];
  }
  return points;
}

function UsageChart({ data, range, bucket, tokenData }: UsageChartProps) {
  if (!data || data.length === 0) {
    return (
      <div className="chart-empty">No data for {bucket} in this range</div>
    );
  }

  const color = getAreaColor(data);
  const gradientId = `gradient-${bucket.replace(/\s/g, "")}`;
  const hasTokenData = tokenData && tokenData.length > 0;

  // Anchor data to "now" so idle gaps are visible in the chart
  const anchoredData = anchorToNow(data, { utilization: 0 } as Omit<DataPoint, "timestamp">);
  const anchoredTokenData = hasTokenData
    ? anchorToNow(tokenData, {
        input_tokens: 0,
        output_tokens: 0,
        cache_creation_input_tokens: 0,
        cache_read_input_tokens: 0,
        total_tokens: 0,
      } as Omit<TokenDataPoint, "timestamp">)
    : tokenData;

  // Merge usage and token data by timestamp for the composed chart
  const mergedData: MergedDataPoint[] = hasTokenData
    ? mergeDataSeries(anchoredData, anchoredTokenData)
    : anchoredData.map((d) => ({ ...d, total_tokens: null, total_lines_changed: null }));

  const formatter = (v: string) => formatTime(v, range);
  const allowedTicks = dedupeTickLabels(mergedData, formatter);

  // Compute max token value for right Y-axis
  const maxTokens = hasTokenData
    ? Math.max(...tokenData.map((d) => d.total_tokens), 0)
    : 0;

  if (!hasTokenData) {
    return (
      <div className="chart-container">
        <ResponsiveContainer width="100%" height="100%">
          <AreaChart
            data={anchoredData}
            margin={{ top: 8, right: 8, left: -20, bottom: 0 }}
          >
            <defs>
              <linearGradient id={gradientId} x1="0" y1="0" x2="0" y2="1">
                <stop offset="5%" stopColor={color} stopOpacity={0.15} />
                <stop offset="95%" stopColor={color} stopOpacity={0.01} />
              </linearGradient>
            </defs>
            <CartesianGrid
              strokeDasharray="3 3"
              stroke="rgba(255,255,255,0.06)"
              vertical={false}
            />
            <XAxis
              dataKey="timestamp"
              tickFormatter={formatter}
              ticks={anchoredData
                .filter((_, i) => allowedTicks.has(i))
                .map((d) => d.timestamp)}
              stroke="rgba(255,255,255,0.2)"
              fontSize={9}
              tickLine={false}
              axisLine={false}
              minTickGap={50}
            />
            <YAxis
              domain={[0, 100]}
              ticks={[0, 25, 50, 75, 100]}
              stroke="rgba(255,255,255,0.2)"
              fontSize={9}
              tickLine={false}
              axisLine={false}
              tickFormatter={(v) => `${v}%`}
            />
            <Tooltip content={<CustomTooltip />} />
            <ReferenceLine
              y={80}
              stroke="rgba(248,113,113,0.3)"
              strokeDasharray="4 4"
            />
            <ReferenceLine
              y={50}
              stroke="rgba(251,191,36,0.2)"
              strokeDasharray="4 4"
            />
            <Area
              type="monotone"
              dataKey="utilization"
              stroke={color}
              strokeWidth={1.5}
              fill={`url(#${gradientId})`}
              dot={false}
              animationDuration={300}
            />
          </AreaChart>
        </ResponsiveContainer>
      </div>
    );
  }

  // Dual-axis composed chart
  return (
    <div className="chart-container">
      <ResponsiveContainer width="100%" height="100%">
        <ComposedChart
          data={mergedData}
          margin={{ top: 8, right: 8, left: -20, bottom: 0 }}
        >
          <defs>
            <linearGradient id={gradientId} x1="0" y1="0" x2="0" y2="1">
              <stop offset="5%" stopColor={color} stopOpacity={0.15} />
              <stop offset="95%" stopColor={color} stopOpacity={0.01} />
            </linearGradient>
          </defs>
          <CartesianGrid
            strokeDasharray="3 3"
            stroke="rgba(255,255,255,0.06)"
            vertical={false}
          />
          <XAxis
            dataKey="timestamp"
            tickFormatter={formatter}
            ticks={mergedData
              .filter((_, i) => allowedTicks.has(i))
              .map((d) => d.timestamp)}
            stroke="rgba(255,255,255,0.2)"
            fontSize={9}
            tickLine={false}
            axisLine={false}
            minTickGap={50}
          />
          <YAxis
            yAxisId="left"
            domain={[0, 100]}
            ticks={[0, 25, 50, 75, 100]}
            stroke="rgba(255,255,255,0.2)"
            fontSize={9}
            tickLine={false}
            axisLine={false}
            tickFormatter={(v) => `${v}%`}
          />
          <YAxis
            yAxisId="right"
            orientation="right"
            domain={[0, Math.ceil(maxTokens * 1.1)]}
            stroke="rgba(96,165,250,0.3)"
            fontSize={9}
            tickLine={false}
            axisLine={false}
            tickFormatter={formatTokenCount}
          />
          <Tooltip content={<CustomTooltip />} />
          <ReferenceLine
            yAxisId="left"
            y={80}
            stroke="rgba(248,113,113,0.3)"
            strokeDasharray="4 4"
          />
          <ReferenceLine
            yAxisId="left"
            y={50}
            stroke="rgba(251,191,36,0.2)"
            strokeDasharray="4 4"
          />
          <Area
            yAxisId="left"
            type="monotone"
            dataKey="utilization"
            stroke={color}
            strokeWidth={1.5}
            fill={`url(#${gradientId})`}
            dot={false}
            animationDuration={300}
          />
          <Line
            yAxisId="right"
            type="monotone"
            dataKey="total_tokens"
            stroke="#60a5fa"
            strokeWidth={1.5}
            dot={false}
            animationDuration={300}
            connectNulls
          />
        </ComposedChart>
      </ResponsiveContainer>
    </div>
  );
}

function findClosestIndex(sorted: number[], target: number): number {
  let lo = 0;
  let hi = sorted.length - 1;
  while (lo < hi) {
    const mid = (lo + hi) >> 1;
    if (sorted[mid] < target) lo = mid + 1;
    else hi = mid;
  }
  if (
    lo > 0 &&
    Math.abs(sorted[lo - 1] - target) < Math.abs(sorted[lo] - target)
  ) {
    return lo - 1;
  }
  return lo;
}

const MATCH_WINDOW_MS = 30 * 60 * 1000;

function mergeDataSeries(
  usageData: DataPoint[],
  tokenData: TokenDataPoint[],
): MergedDataPoint[] {
  const tokenTimestamps = tokenData.map((t) => new Date(t.timestamp).getTime());
  const usageTimestamps = usageData.map((u) => new Date(u.timestamp).getTime());

  // Build merged array from usage data, matching closest token via binary search
  const merged: MergedDataPoint[] = usageData.map((u, i) => {
    if (tokenTimestamps.length === 0) {
      return { ...u, total_tokens: null, total_lines_changed: null };
    }
    const closest = findClosestIndex(tokenTimestamps, usageTimestamps[i]);
    const dist = Math.abs(tokenTimestamps[closest] - usageTimestamps[i]);
    if (dist < MATCH_WINDOW_MS) {
      return { ...u, total_tokens: tokenData[closest].total_tokens, total_lines_changed: null };
    }
    return { ...u, total_tokens: null, total_lines_changed: null };
  });

  // Add token data points without nearby usage points
  for (let i = 0; i < tokenData.length; i++) {
    if (usageTimestamps.length === 0) {
      merged.push({
        timestamp: tokenData[i].timestamp,
        utilization: null,
        total_tokens: tokenData[i].total_tokens,
        total_lines_changed: null,
      });
      continue;
    }
    const closest = findClosestIndex(usageTimestamps, tokenTimestamps[i]);
    const dist = Math.abs(usageTimestamps[closest] - tokenTimestamps[i]);
    if (dist >= MATCH_WINDOW_MS) {
      merged.push({
        timestamp: tokenData[i].timestamp,
        utilization: null,
        total_tokens: tokenData[i].total_tokens,
        total_lines_changed: null,
      });
    }
  }

  const tsCache = new Map<string, number>();
  const getTs = (ts: string) => {
    let v = tsCache.get(ts);
    if (v === undefined) {
      v = new Date(ts).getTime();
      tsCache.set(ts, v);
    }
    return v;
  };
  merged.sort((a, b) => getTs(a.timestamp) - getTs(b.timestamp));

  return merged;
}

export default UsageChart;
