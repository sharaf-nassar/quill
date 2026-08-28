import { invoke } from "@tauri-apps/api/core";
import { useEffect, useRef, useState } from "react";
import type { StartupStatus } from "../types";

function formatBytes(bytes: number): string {
  const units = ["B", "KB", "MB", "GB", "TB"];
  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return `${value.toFixed(unit >= 3 ? 1 : 0)} ${units[unit]}`;
}

function formatEta(seconds: number): string {
  if (seconds < 60) return "<1m left in backup";
  if (seconds < 3600) return `~${Math.max(1, Math.round(seconds / 60))}m left in backup`;
  const hours = Math.floor(seconds / 3600);
  const minutes = Math.round((seconds % 3600) / 60);
  return `~${hours}h ${minutes}m left in backup`;
}

// @lat: [[backend#Backend#Entry Point#Startup migration surface]]
export default function MigrationView({ status }: { status: StartupStatus }) {
  const sample = useRef<{ at: number; bytes: number; rate: number } | null>(null);
  const [etaSeconds, setEtaSeconds] = useState<number | null>(null);

  useEffect(() => {
    const previous = document.documentElement.dataset.view;
    document.documentElement.dataset.view = "migration";
    return () => {
      document.documentElement.dataset.view = previous ?? "main";
    };
  }, []);

  const hasByteProgress =
    status.completedBytes !== null &&
    status.totalBytes !== null &&
    status.totalBytes > 0;

  useEffect(() => {
    if (!hasByteProgress || status.completedBytes === null || status.totalBytes === null) {
      sample.current = null;
      setEtaSeconds(null);
      return;
    }

    const now = performance.now();
    const previous = sample.current;
    if (previous && status.completedBytes > previous.bytes) {
      const elapsedSeconds = (now - previous.at) / 1000;
      if (elapsedSeconds <= 0) return;
      const measuredRate = (status.completedBytes - previous.bytes) / elapsedSeconds;
      const rate = previous.rate > 0 ? previous.rate * 0.7 + measuredRate * 0.3 : measuredRate;
      sample.current = { at: now, bytes: status.completedBytes, rate };
      setEtaSeconds(Math.max(0, (status.totalBytes - status.completedBytes) / rate));
    } else if (!previous) {
      sample.current = { at: now, bytes: status.completedBytes, rate: 0 };
    }
  }, [hasByteProgress, status.completedBytes, status.totalBytes]);
  const progress = hasByteProgress
    ? Math.min(100, (status.completedBytes! / status.totalBytes!) * 100)
    : null;
  const isError = status.state === "error";
  const roundedProgress = progress === null ? null : Math.round(progress);
  const progressLabel =
    roundedProgress === null ? "" : roundedProgress === 100 ? "100%" : `~${roundedProgress}%`;
  const valueText = hasByteProgress
    ? `approximately ${roundedProgress}% complete, ${formatBytes(status.completedBytes!)} of ${formatBytes(status.totalBytes!)}`
    : status.stage;
  const readout = hasByteProgress
    ? [
        progressLabel,
        `${formatBytes(status.completedBytes!)} / ${formatBytes(status.totalBytes!)}`,
        etaSeconds === null ? null : formatEta(etaSeconds),
      ]
        .filter(Boolean)
        .join(" · ")
    : isError
      ? "STOPPED"
      : "WORKING";

  return (
    <main className="migration-shell" data-state={status.state}>
      <header className="migration-titlebar" data-tauri-drag-region>
        <span className="migration-wordmark" data-tauri-drag-region>
          QUILL
        </span>
        <span className="migration-mode" data-tauri-drag-region>
          DATABASE UPDATE
        </span>
        <button
          className="migration-quit"
          type="button"
          aria-label="Quit Quill"
          onClick={() => void invoke("quit_during_startup")}
        >
          ×
        </button>
      </header>

      <section className="migration-body">
        <h1>{isError ? "Database update stopped" : "Updating local database"}</h1>
        <p>{status.detail}</p>

        <div className="migration-readout">
          <span aria-live="polite" aria-atomic="true">
            {status.stage}
          </span>
          <span className="migration-bytes">{readout}</span>
        </div>
        <div
          className="migration-track"
          data-indeterminate={progress === null && !isError ? "true" : undefined}
          role="progressbar"
          aria-label="Database update progress"
          aria-valuemin={progress === null ? undefined : 0}
          aria-valuemax={progress === null ? undefined : 100}
          aria-valuenow={roundedProgress ?? undefined}
          aria-valuetext={valueText}
        >
          <span
            className="migration-fill"
            style={progress === null ? undefined : { width: `${progress}%` }}
          />
        </div>
      </section>
    </main>
  );
}
