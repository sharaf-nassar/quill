import {
  useCallback,
  useLayoutEffect,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { createPortal } from "react-dom";
import type {
  ContextPreservationStatus,
  IndicatorPrimaryProvider,
  IntegrationProvider,
  LayoutMode,
  ProviderStatus,
} from "../../types";

interface ProviderMenuProps {
  className?: string;
  statuses: ProviderStatus[];
  loading: boolean;
  error: string | null;
  inFlightProviders: ReadonlySet<IntegrationProvider>;
  contextPreservation: ContextPreservationStatus;
  contextPreservationInFlight: boolean;
  brevityInFlightProviders: ReadonlySet<IntegrationProvider>;
  indicatorPrimaryProvider: IndicatorPrimaryProvider;
  onRequestToggle: (
    provider: IntegrationProvider,
    nextEnabled: boolean,
  ) => void;
  onContextPreservationToggle: (enabled: boolean) => void;
  onBrevityToggle: (provider: IntegrationProvider, enabled: boolean) => void;
  onIndicatorPrimaryProviderChange: (provider: IndicatorPrimaryProvider) => void;
  layoutMode?: LayoutMode;
  onLayoutModeChange?: (mode: LayoutMode) => void;
  onRescan?: () => void;
  rescanning?: boolean;
}

const BREVITY_PROVIDERS: ReadonlyArray<IntegrationProvider> = ["claude", "codex"];

function providerLabel(provider: IntegrationProvider): string {
  if (provider === "claude") return "Claude Code";
  if (provider === "codex") return "Codex";
  return "MiniMax";
}

type ToggleTone = "on" | "off" | "na" | "setup" | "busy";

interface ToggleState {
  label: string;
  tone: ToggleTone;
  disabled: boolean;
}

function integrationToggleState(
  status: ProviderStatus,
  busy: boolean,
): ToggleState {
  if (busy) return { label: "...", tone: "busy", disabled: true };
  if (status.enabled) return { label: "ON", tone: "on", disabled: false };
  if (!status.detectedCli) return { label: "N/A", tone: "na", disabled: true };
  if (status.setupState === "missing") {
    return { label: "SETUP", tone: "setup", disabled: false };
  }
  return { label: "OFF", tone: "off", disabled: false };
}

function brevityToggleState(
  enabled: boolean,
  providerEnabled: boolean,
  busy: boolean,
): ToggleState {
  if (busy) return { label: "...", tone: "busy", disabled: true };
  if (!providerEnabled) return { label: "—", tone: "na", disabled: true };
  return enabled
    ? { label: "ON", tone: "on", disabled: false }
    : { label: "OFF", tone: "off", disabled: false };
}

function contextToggleState(
  enabled: boolean,
  busy: boolean,
): ToggleState {
  if (busy) return { label: "...", tone: "busy", disabled: true };
  return enabled
    ? { label: "ON", tone: "on", disabled: false }
    : { label: "OFF", tone: "off", disabled: false };
}

type SectionKey = "layout" | "status" | "context" | "brevity" | "integrations";

function renderInlineCode(text: string): ReactNode[] {
  return text.split(/(`[^`]+`)/g).map((part, idx) => {
    if (part.startsWith("`") && part.endsWith("`") && part.length > 1) {
      return <code key={idx}>{part.slice(1, -1)}</code>;
    }
    return part;
  });
}

interface SectionInfo {
  title: string;
  blurb: string;
  body: ReadonlyArray<string>;
}

const SECTION_INFO: Record<SectionKey, SectionInfo> = {
  layout: {
    title: "Layout",
    blurb: "Orientation of the main window's split pane.",
    body: [
      "Stacked places Live Usage above the Analytics dashboard with a horizontal divider; Side-by-side places Live on the left and Analytics on the right with a vertical divider.",
      "Each orientation persists its own split ratio (15–85%) in localStorage (`quill-split-ratio` / `quill-split-ratio-h`). Drag the divider or use Arrow keys to resize.",
      "The choice is saved to `quill-layout-mode` and applies to the next launch.",
    ],
  },
  status: {
    title: "Status Provider",
    blurb: "Which provider's usage drives the tray icon and live badge.",
    body: [
      "Auto lets Quill pick the most active enabled provider. Selecting a specific provider pins the indicator to that provider's quotas, even if another provider has higher utilization.",
      "Backed by the `set_indicator_primary_provider` IPC command — emits an `indicator-updated` event so the tray title, tray summary rows, and this menu stay in sync without re-polling.",
    ],
  },
  context: {
    title: "Working Context Preservation",
    blurb: "Keep large transient context out of the active LLM transcript.",
    body: [
      "When ON, Quill installs context-routing hooks, the `quill_*` MCP tools (index_context, search_context, execute, fetch_and_index, etc.), continuity capture, and context-savings telemetry into every enabled provider.",
      "Hooks block raw WebFetch and noisy `curl`/`wget` dumps and route Bash, Read, Grep, build, and test output through the MCP. Large outputs become `source:N` and `chunk:N` refs so the assistant stays compact and pulls exact details only on demand.",
      "Disabling redeploys the base provider integration without context hooks. Historical context savings stay searchable in Analytics → Context.",
      "Default is OFF. The setting applies globally across Claude Code and Codex.",
    ],
  },
  brevity: {
    title: "Brevity Profile",
    blurb: "Caveman-style prose compression injected into the agent file.",
    body: [
      "Per-provider toggle. Writes a `<!-- quill-managed:brevity:start -->…<!-- end -->` block into `~/.claude/CLAUDE.md` (Claude Code) or `~/.codex/AGENTS.md` (Codex).",
      "The block tells the assistant to drop articles, hedging, and filler in prose responses while preserving verbatim: code blocks, inline code, URLs, file paths, command names, library and proper-noun names, numbers, env vars, and markdown structure.",
      "MiniMax has no managed agent file and is excluded. If `AGENTS.md` is a symlink to `CLAUDE.md` the writer detects the shared canonical path and writes only once.",
      "Disabling strips only the managed block; the rest of the file stays intact.",
    ],
  },
  integrations: {
    title: "Integrations",
    blurb: "Per-provider Quill assets — hooks, MCP server, commands, instructions.",
    body: [
      "Enabling installs scripts under `~/.config/quill/scripts/`, MCP tools under `~/.config/quill/mcp/`, custom commands under `~/.<provider>/commands/`, hook registrations in the provider's `settings.json`, an MCP server entry, and a managed instruction block in `CLAUDE.md` / `AGENTS.md`.",
      "Disabling removes every Quill-deployed asset. Historical analytics, sessions, learning data, and stored context stay in the local Quill database.",
      "ON — fully installed and active.  OFF — provider detected but Quill assets not installed.  SETUP — auto-deployment is pending; click to run it.  N/A — provider CLI not detected on this machine.  … — toggle in flight.",
      "MiniMax requires an API key on first enable; the key is stored locally and used only against the MiniMax API.",
    ],
  },
};

const TOOLTIP_WIDTH = 252;
const TOOLTIP_GAP = 8;
const TOOLTIP_VIEWPORT_PAD = 8;

interface TooltipPos {
  top: number;
  left: number;
  placement: "left" | "below";
}

function ProviderMenu({
  className,
  statuses,
  loading,
  error,
  inFlightProviders,
  contextPreservation,
  contextPreservationInFlight,
  brevityInFlightProviders,
  indicatorPrimaryProvider,
  onRequestToggle,
  onContextPreservationToggle,
  onBrevityToggle,
  onIndicatorPrimaryProviderChange,
  layoutMode,
  onLayoutModeChange,
  onRescan,
  rescanning = false,
}: ProviderMenuProps) {
  const enabledProviders = statuses
    .filter((status) => status.enabled)
    .map((status) => status.provider);
  const unavailablePreferredProvider =
    indicatorPrimaryProvider != null &&
    !enabledProviders.includes(indicatorPrimaryProvider);

  const ctxState = contextToggleState(
    contextPreservation.enabled,
    contextPreservationInFlight,
  );

  const menuRef = useRef<HTMLDivElement>(null);
  const [tooltip, setTooltip] = useState<{
    section: SectionKey;
    pos: TooltipPos;
    override?: { title: string; body: string[] };
  } | null>(null);

  const measure = useCallback(
    (anchor: HTMLElement): TooltipPos => {
      const rect = anchor.getBoundingClientRect();
      const menu = menuRef.current?.getBoundingClientRect();
      const vw = window.innerWidth;
      const vh = window.innerHeight;
      const leftEdge = (menu?.left ?? rect.left) - TOOLTIP_GAP - TOOLTIP_WIDTH;
      if (leftEdge >= TOOLTIP_VIEWPORT_PAD) {
        const top = Math.min(
          Math.max(rect.top, TOOLTIP_VIEWPORT_PAD),
          vh - TOOLTIP_VIEWPORT_PAD,
        );
        return { top, left: leftEdge, placement: "left" };
      }
      const fallbackTop = (menu?.bottom ?? rect.bottom) + TOOLTIP_GAP;
      const fallbackLeft = Math.min(
        Math.max(TOOLTIP_VIEWPORT_PAD, (menu?.left ?? rect.left)),
        vw - TOOLTIP_WIDTH - TOOLTIP_VIEWPORT_PAD,
      );
      return { top: fallbackTop, left: fallbackLeft, placement: "below" };
    },
    [],
  );

  const handleEnter = useCallback(
    (section: SectionKey) =>
      (event: React.MouseEvent<HTMLElement>) => {
        const pos = measure(event.currentTarget);
        setTooltip({ section, pos });
      },
    [measure],
  );

  const handleEnterOverride = useCallback(
    (override: { title: string; body: string[] }) =>
      (event: React.MouseEvent<HTMLElement>) => {
        const pos = measure(event.currentTarget);
        setTooltip({ section: "integrations", pos, override });
      },
    [measure],
  );

  const handleLeave = useCallback(() => {
    setTooltip(null);
  }, []);

  // Hide the tooltip if the menu scrolls or the window resizes.
  useLayoutEffect(() => {
    if (!tooltip) return;
    const close = () => setTooltip(null);
    window.addEventListener("resize", close);
    const menu = menuRef.current;
    menu?.addEventListener("scroll", close);
    return () => {
      window.removeEventListener("resize", close);
      menu?.removeEventListener("scroll", close);
    };
  }, [tooltip]);

  const sectionProps = (section: SectionKey) => ({
    onMouseEnter: handleEnter(section),
    onMouseLeave: handleLeave,
  });

  return (
    <>
      <div
        ref={menuRef}
        className={className ? `provider-menu ${className}` : "provider-menu"}
        role="menu"
        aria-label="Quill settings"
      >
        {layoutMode != null && onLayoutModeChange != null && (
          <div className="pmenu-row" {...sectionProps("layout")}>
            <span className="pmenu-label">Layout</span>
            <div className="pmenu-icon-group">
              <button
                className={`pmenu-icon-btn${layoutMode === "stacked" ? " is-active" : ""}`}
                onClick={() => onLayoutModeChange("stacked")}
                aria-pressed={layoutMode === "stacked"}
                aria-label="Stacked layout"
              >
                <svg width="12" height="12" viewBox="0 0 16 16" aria-hidden="true">
                  <rect x="2" y="2" width="12" height="5" rx="1" fill="currentColor" />
                  <rect x="2" y="9" width="12" height="5" rx="1" fill="currentColor" />
                </svg>
              </button>
              <button
                className={`pmenu-icon-btn${layoutMode === "side-by-side" ? " is-active" : ""}`}
                onClick={() => onLayoutModeChange("side-by-side")}
                aria-pressed={layoutMode === "side-by-side"}
                aria-label="Side by side layout"
              >
                <svg width="12" height="12" viewBox="0 0 16 16" aria-hidden="true">
                  <rect x="1" y="2" width="6" height="12" rx="1" fill="currentColor" />
                  <rect x="9" y="2" width="6" height="12" rx="1" fill="currentColor" />
                </svg>
              </button>
            </div>
          </div>
        )}

        <div className="pmenu-row" {...sectionProps("status")}>
          <label className="pmenu-label" htmlFor="indicator-primary-provider">
            Status
          </label>
          <select
            id="indicator-primary-provider"
            className="pmenu-select"
            value={indicatorPrimaryProvider ?? ""}
            disabled={loading}
            onChange={(event) =>
              onIndicatorPrimaryProviderChange(
                (event.target.value || null) as IndicatorPrimaryProvider,
              )
            }
            aria-label="Status provider"
          >
            <option value="">Auto</option>
            {unavailablePreferredProvider ? (
              <option value={indicatorPrimaryProvider ?? ""} disabled>
                {providerLabel(indicatorPrimaryProvider)} (n/a)
              </option>
            ) : null}
            {enabledProviders.map((provider) => (
              <option key={provider} value={provider}>
                {providerLabel(provider)}
              </option>
            ))}
          </select>
        </div>

        <div className="pmenu-row" {...sectionProps("context")}>
          <span className="pmenu-label">Context</span>
          <button
            className={`pmenu-toggle pmenu-toggle--${ctxState.tone}`}
            disabled={ctxState.disabled || loading}
            onClick={() => onContextPreservationToggle(!contextPreservation.enabled)}
            aria-pressed={contextPreservation.enabled}
          >
            {ctxState.label}
          </button>
        </div>

        <div className="pmenu-group" {...sectionProps("integrations")}>
          Integrations
        </div>
        {onRescan ? (
          <div className="pmenu-row" {...sectionProps("integrations")}>
            <span className="pmenu-name">Rescan PATH</span>
            <button
              className={`pmenu-toggle pmenu-toggle--${rescanning ? "busy" : "off"}`}
              disabled={rescanning || loading}
              onClick={() => onRescan()}
              aria-label="Rescan installed CLIs"
              title="Re-search PATH for claude / codex"
            >
              {rescanning ? "..." : "RUN"}
            </button>
          </div>
        ) : null}
        {loading ? (
          <div className="pmenu-empty" {...sectionProps("integrations")}>
            checking…
          </div>
        ) : error ? (
          <div
            className="pmenu-empty pmenu-empty--error"
            {...sectionProps("integrations")}
          >
            {error}
          </div>
        ) : (
          statuses.map((status) => {
            const busy = inFlightProviders.has(status.provider);
            const state = integrationToggleState(status, busy);
            const attempts = status.lastDetectionAttempts ?? [];
            const showDiagnostic = state.tone === "na" && attempts.length > 0;
            const rowProps = showDiagnostic
              ? {
                  onMouseEnter: handleEnterOverride({
                    title: `${providerLabel(status.provider)} CLI not found`,
                    body: [
                      "Quill checked these locations and didn't find the CLI:",
                      // Wrap each path in backticks so the existing
                      // renderInlineCode parser styles it as <code>, separating
                      // path lines visually from the surrounding prose.
                      ...attempts.map((path) => `\`${path}\``),
                      "Install the CLI in any of these locations, or update your shell PATH and click Rescan PATH above.",
                    ],
                  }),
                  onMouseLeave: handleLeave,
                }
              : sectionProps("integrations");
            return (
              <div
                key={status.provider}
                className="pmenu-row"
                {...rowProps}
              >
                <span className="pmenu-name">{providerLabel(status.provider)}</span>
                <button
                  className={`pmenu-toggle pmenu-toggle--${state.tone}`}
                  disabled={state.disabled}
                  onClick={() => onRequestToggle(status.provider, !status.enabled)}
                  aria-pressed={status.enabled}
                >
                  {state.label}
                </button>
              </div>
            );
          })
        )}

        <div className="pmenu-group" {...sectionProps("brevity")}>
          Brevity
        </div>
        {BREVITY_PROVIDERS.map((provider) => {
          const status = statuses.find((s) => s.provider === provider);
          const enabled = !!status?.brevityEnabled;
          const providerEnabled = !!status?.enabled;
          const busy = brevityInFlightProviders.has(provider);
          const state = brevityToggleState(enabled, providerEnabled, busy);
          return (
            <div
              key={`brevity-${provider}`}
              className="pmenu-row"
              {...sectionProps("brevity")}
            >
              <span className="pmenu-name">{providerLabel(provider)}</span>
              <button
                className={`pmenu-toggle pmenu-toggle--${state.tone}`}
                disabled={state.disabled || loading}
                onClick={() => onBrevityToggle(provider, !enabled)}
                aria-pressed={enabled}
              >
                {state.label}
              </button>
            </div>
          );
        })}
      </div>

      {tooltip
        ? createPortal(
            <div
              className={`pmenu-tooltip pmenu-tooltip--${tooltip.pos.placement}`}
              role="tooltip"
              style={{
                top: tooltip.pos.top,
                left: tooltip.pos.left,
                width: TOOLTIP_WIDTH,
              }}
            >
              <div className="pmenu-tooltip-title">
                {tooltip.override
                  ? tooltip.override.title
                  : SECTION_INFO[tooltip.section].title}
              </div>
              {tooltip.override ? null : (
                <div className="pmenu-tooltip-blurb">
                  {SECTION_INFO[tooltip.section].blurb}
                </div>
              )}
              {(tooltip.override
                ? tooltip.override.body
                : SECTION_INFO[tooltip.section].body
              ).map((line, idx) => (
                <p key={idx} className="pmenu-tooltip-body">
                  {renderInlineCode(line)}
                </p>
              ))}
            </div>,
            document.body,
          )
        : null}
    </>
  );
}

export default ProviderMenu;
