import { useState, useRef, useEffect } from "react";
import LiveSummaryModule from "./live/LiveSummaryModule";
import ProviderUsageModule from "./live/ProviderUsageModule";
import type {
  IntegrationProvider,
  ProviderCredits,
  TimeMode,
  UsageBucket,
  UsageData,
} from "../types";
import { providerLabel } from "../utils/providers";

const TIME_MODES: { key: TimeMode; label: string; tip: string }[] = [
  {
    key: "marker",
    label: "Pace marker",
    tip: "Vertical line on the usage bar showing expected pace",
  },
  {
    key: "dual",
    label: "Dual bars",
    tip: "Second bar below usage showing time elapsed in period",
  },
  {
    key: "background",
    label: "Background fill",
    tip: "Row background fills as time passes toward reset",
  },
];

interface UsageDisplayProps {
  data: UsageData | null;
  timeMode: TimeMode;
  enabledProviders: IntegrationProvider[];
  onTimeModeChange: (mode: TimeMode) => void;
}

interface ProviderSection {
  provider: IntegrationProvider;
  buckets: UsageBucket[];
}

function buildProviderSections(
  enabledProviders: IntegrationProvider[],
  buckets: UsageBucket[],
): ProviderSection[] {
  const providers =
    enabledProviders.length > 0 ? enabledProviders : (["claude", "codex"] as const);

  return providers.flatMap((provider) => {
    const providerBuckets = buckets.filter((bucket) => bucket.provider === provider);
    return providerBuckets.length > 0
      ? [{ provider, buckets: providerBuckets }]
      : [];
  });
}

function UsageDisplay({
  data,
  timeMode,
  enabledProviders,
  onTimeModeChange,
}: UsageDisplayProps) {
  const [open, setOpen] = useState(false);
  const [focusIdx, setFocusIdx] = useState(-1);
  const menuRef = useRef<HTMLDivElement>(null);
  const itemRefs = useRef<(HTMLButtonElement | null)[]>([]);

  useEffect(() => {
    if (!open) return;
    const handler = (e: MouseEvent) => {
      if (menuRef.current && !menuRef.current.contains(e.target as Node)) {
        setOpen(false);
      }
    };
    document.addEventListener("mousedown", handler);
    return () => document.removeEventListener("mousedown", handler);
  }, [open]);

  useEffect(() => {
    if (open && focusIdx >= 0 && itemRefs.current[focusIdx]) {
      itemRefs.current[focusIdx]!.focus();
    }
  }, [open, focusIdx]);

  if (!data) {
    return <div className="loading">{"Loading\u2026"}</div>;
  }

  if (data.error) {
    console.error("Usage fetch error:", data.error);
    const lowerError = data.error.toLowerCase();
    const msg =
      lowerError.includes("credential") || lowerError.includes("claude /login")
        ? data.error
        : "Failed to load usage data";
    if (data.buckets.length === 0) {
      return (
        <div className="error-label" role="alert">
          {msg}
        </div>
      );
    }
  }

  const providerSections = buildProviderSections(enabledProviders, data.buckets);

  if (providerSections.length === 0) {
    return <div className="loading">No usage data</div>;
  }

  const showProviderHeadings = enabledProviders.length > 1;

  const creditsByProvider = new Map<IntegrationProvider, ProviderCredits>(
    data.provider_credits.map((c) => [c.provider, c]),
  );

  return (
    <div className="usage-display">
      <LiveSummaryModule enabledProviders={enabledProviders} />
      {providerSections.length > 0 && (
        <div className="col-header">
          <span className="col-limits">Limits</span>
          <span className="col-center-cog">
            <button
              className="titlebar-cog"
              onClick={() => setOpen((v) => !v)}
              onKeyDown={(e) => {
                if (
                  e.key === "ArrowDown" ||
                  e.key === "Enter" ||
                  e.key === " "
                ) {
                  e.preventDefault();
                  setOpen(true);
                  const activeIdx = TIME_MODES.findIndex(
                    (m) => m.key === timeMode,
                  );
                  setFocusIdx(activeIdx >= 0 ? activeIdx : 0);
                }
              }}
              aria-label="Display settings"
              aria-haspopup="true"
              aria-expanded={open}
            >
              &#9881;
            </button>
            {open && (
              <div
                className="cog-menu cog-menu-center"
                ref={menuRef}
                role="menu"
                aria-label="Time display mode"
                onKeyDown={(e) => {
                  if (e.key === "Escape") {
                    setOpen(false);
                    setFocusIdx(-1);
                    e.stopPropagation();
                  } else if (e.key === "ArrowDown") {
                    e.preventDefault();
                    setFocusIdx((i) => Math.min(i + 1, TIME_MODES.length - 1));
                  } else if (e.key === "ArrowUp") {
                    e.preventDefault();
                    setFocusIdx((i) => Math.max(i - 1, 0));
                  } else if (e.key === "Enter" || e.key === " ") {
                    e.preventDefault();
                    if (focusIdx >= 0 && focusIdx < TIME_MODES.length) {
                      onTimeModeChange(TIME_MODES[focusIdx].key);
                      setOpen(false);
                      setFocusIdx(-1);
                    }
                  }
                }}
              >
                <div className="cog-menu-header">Time Display</div>
                {TIME_MODES.map((m, i) => (
                  <button
                    key={m.key}
                    ref={(el) => {
                      itemRefs.current[i] = el;
                    }}
                    className={`cog-menu-item${timeMode === m.key ? " active" : ""}`}
                    role="menuitem"
                    tabIndex={focusIdx === i ? 0 : -1}
                    aria-label={m.tip}
                    onClick={() => {
                      onTimeModeChange(m.key);
                      setOpen(false);
                      setFocusIdx(-1);
                    }}
                  >
                    {m.label}
                  </button>
                ))}
              </div>
            )}
          </span>
          <span className="col-resets">Resets In</span>
        </div>
      )}
      {data.provider_errors.length > 0 && (
        <div className="usage-provider-errors" role="status" aria-live="polite">
          {data.provider_errors.map((providerError) => (
            <div
              key={providerError.provider}
              className="usage-provider-error"
            >
              <span className="usage-provider-error__label">
                {providerLabel(providerError.provider)}
              </span>
              <span className="usage-provider-error__message">
                {providerError.message}
              </span>
            </div>
          ))}
        </div>
      )}
      <div className="usage-providers">
        {providerSections.map((section) => (
          <ProviderUsageModule
            key={section.provider}
            provider={section.provider}
            buckets={section.buckets}
            timeMode={timeMode}
            showHeading={showProviderHeadings}
            credits={creditsByProvider.get(section.provider)}
          />
        ))}
      </div>
    </div>
  );
}

export default UsageDisplay;
