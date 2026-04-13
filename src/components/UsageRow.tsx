import type { ReactNode } from "react";
import { useState, useEffect } from "react";
import type { TimeMode } from "../types";

function colorClass(utilization: number): string {
  if (utilization < 50) return "green";
  if (utilization < 80) return "yellow";
  return "red";
}

function statusText(utilization: number): { short: string; full: string } {
  if (utilization < 50) return { short: "", full: "" };
  if (utilization < 80) return { short: "High", full: "High usage" };
  return { short: "Crit", full: "Critical usage" };
}

export function gradientColor(utilization: number): string {
  const t = Math.max(0, Math.min(utilization / 100, 1));
  let r: number, g: number, b: number;
  if (t < 0.5) {
    const f = t / 0.5;
    r = Math.round(52 + (251 - 52) * f);
    g = Math.round(211 + (191 - 211) * f);
    b = Math.round(36 + (153 - 36) * f);
  } else {
    const f = (t - 0.5) / 0.5;
    r = Math.round(251 + (248 - 251) * f);
    g = Math.round(191 + (113 - 191) * f);
    b = Math.round(36 + (113 - 36) * f);
  }
  return `rgb(${r}, ${g}, ${b})`;
}

export function formatCountdown(resetsAt: string | null): ReactNode[] | null {
  if (!resetsAt) return null;
  try {
    const resetDate = new Date(resetsAt);
    const now = new Date();
    const totalSeconds = Math.floor(
      (resetDate.getTime() - now.getTime()) / 1000,
    );
    if (totalSeconds <= 0) return formatNumUnit("now");

    const days = Math.floor(totalSeconds / 86400);
    const hours = Math.floor((totalSeconds % 86400) / 3600);
    const minutes = Math.floor((totalSeconds % 3600) / 60);

    let raw: string;
    if (days > 0) {
      raw = `${days}d ${String(hours).padStart(2, "0")}h`;
    } else if (hours > 0) {
      raw = `${hours}h ${String(minutes).padStart(2, "0")}m`;
    } else {
      raw = `${minutes}m`;
    }
    return formatNumUnit(raw);
  } catch {
    return null;
  }
}

function formatNumUnit(text: string): ReactNode[] {
  return text.split("").map((ch, i) => {
    const isDigit = ch >= "0" && ch <= "9";
    return (
      <span key={i} className={isDigit ? "num" : "unit"}>
        {ch}
      </span>
    );
  });
}

function getTimeFraction(
  resetsAt: string | null,
  label: string,
): number | null {
  if (!resetsAt) return null;
  try {
    const resetDate = new Date(resetsAt);
    const now = new Date();
    const remainingMs = resetDate.getTime() - now.getTime();

    const isFiveHour =
      label.toLowerCase().includes("5") && label.toLowerCase().includes("hour");
    const periodMs = isFiveHour ? 5 * 60 * 60 * 1000 : 7 * 24 * 60 * 60 * 1000;

    const elapsedMs = periodMs - remainingMs;
    return Math.max(0, Math.min(elapsedMs / periodMs, 1));
  } catch {
    return null;
  }
}

interface BarProps {
  fraction: number;
  cls: string;
  timeFraction: number | null;
}

function MarkerBar({ fraction, cls, timeFraction }: BarProps) {
  return (
    <div className="progress-track marker-track">
      <div
        className={`progress-fill ${cls}`}
        style={{ width: `${fraction * 100}%` }}
      />
      {timeFraction !== null && (
        <div
          className="time-marker"
          style={{ left: `${timeFraction * 100}%` }}
        />
      )}
    </div>
  );
}

function DualBar({ fraction, cls, timeFraction }: BarProps) {
  return (
    <div className="bars">
      <div className="progress-track">
        <div
          className={`progress-fill ${cls}`}
          style={{ width: `${fraction * 100}%` }}
        />
      </div>
      {timeFraction !== null && (
        <div className="dual-row">
          <span className="dual-label">time</span>
          <div className="time-track">
            <div
              className="time-fill"
              style={{ width: `${timeFraction * 100}%` }}
            />
          </div>
        </div>
      )}
    </div>
  );
}

function BackgroundBar({ fraction, cls, timeFraction }: BarProps) {
  return (
    <>
      {timeFraction !== null && (
        <div
          className="bg-time-fill"
          style={{ width: `${timeFraction * 100}%` }}
        />
      )}
      <div className="progress-track">
        <div
          className={`progress-fill ${cls}`}
          style={{ width: `${fraction * 100}%` }}
        />
      </div>
    </>
  );
}

interface UsageRowProps {
  label: string;
  utilization: number;
  resetsAt: string | null;
  timeMode: TimeMode;
}

function UsageRow({
  label,
  utilization,
  resetsAt,
  timeMode,
}: UsageRowProps) {
  const [, setTick] = useState(0);

  useEffect(() => {
    const interval = setInterval(() => setTick((t) => t + 1), 10_000);
    return () => clearInterval(interval);
  }, []);

  const fraction = Math.min(utilization / 100, 1);
  const cls = colorClass(utilization);
  const status = statusText(utilization);
  const pctColor = gradientColor(utilization);
  const countdown = formatCountdown(resetsAt);
  const timeFraction = getTimeFraction(resetsAt, label);
  const isBackground = timeMode === "background";

  return (
    <div className={`row-box${isBackground ? " row-box-bg" : ""}`}>
      <div className="row-top">
        <span className="row-label">{formatNumUnit(label)}</span>
        <span className="row-percent" style={{ color: pctColor }}>
          {Math.round(utilization)}%
          <span
            className="status-label"
            style={{ color: pctColor }}
            title={status.full}
            aria-label={status.full}
          >
            {status.short}
          </span>
        </span>
        <span className="row-countdown">{countdown}</span>
      </div>
      {timeMode === "marker" && (
        <MarkerBar fraction={fraction} cls={cls} timeFraction={timeFraction} />
      )}
      {timeMode === "dual" && (
        <DualBar fraction={fraction} cls={cls} timeFraction={timeFraction} />
      )}
      {timeMode === "background" && (
        <BackgroundBar
          fraction={fraction}
          cls={cls}
          timeFraction={timeFraction}
        />
      )}
    </div>
  );
}

export default UsageRow;
