import {
  lazy,
  Suspense,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { getVersion } from "@tauri-apps/api/app";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { WebviewWindow } from "@tauri-apps/api/webviewWindow";
import { listen } from "@tauri-apps/api/event";
import { useIntegrations } from "../hooks/useIntegrations";
import CommandPalette, {
  type PaletteCommand,
} from "../components/CommandPalette";
import "../styles/manage.css";

// Sections reuse their existing window-view components; the per-window chrome is
// suppressed via manage.css when rendered inside the pane. Lazy-loaded so only
// the active section's chunk is fetched.
const SessionsSection = lazy(() => import("./SessionsWindowView"));
const LearningSection = lazy(() => import("./LearningWindow"));
const PluginsSection = lazy(() => import("./PluginsWindowView"));
const InstancesSection = lazy(() => import("./RestartWindowView"));
const SettingsSection = lazy(() => import("./SettingsWindowView"));

// ── Section icons (14px line glyphs, shared visual language with TitleBar) ──

const SVG = {
  viewBox: "0 0 14 14",
  width: 16,
  height: 16,
  fill: "none",
  stroke: "currentColor",
  strokeWidth: 1.4,
  strokeLinecap: "round" as const,
  strokeLinejoin: "round" as const,
  "aria-hidden": true,
  focusable: false,
};

const SessionsIcon = () => (
  <svg {...SVG}>
    <circle cx="6" cy="6" r="3.4" />
    <line x1="8.6" y1="8.6" x2="11.6" y2="11.6" />
  </svg>
);
const LearningIcon = () => (
  <svg {...SVG}>
    <path d="M7 1.5L8.4 5.6 12.5 7 8.4 8.4 7 12.5 5.6 8.4 1.5 7 5.6 5.6Z" />
  </svg>
);
const PluginsIcon = () => (
  <svg {...SVG}>
    <rect x="2.5" y="2.5" width="3.6" height="3.6" rx="0.5" />
    <rect x="7.9" y="2.5" width="3.6" height="3.6" rx="0.5" />
    <rect x="2.5" y="7.9" width="3.6" height="3.6" rx="0.5" />
    <rect x="7.9" y="7.9" width="3.6" height="3.6" rx="0.5" />
  </svg>
);
const InstancesIcon = () => (
  <svg {...SVG}>
    <rect x="1.6" y="2.6" width="10.8" height="8.8" rx="1.2" />
    <path d="M4 5.6l2 1.5-2 1.5" />
    <line x1="7.4" y1="9.1" x2="9.6" y2="9.1" />
  </svg>
);
const SettingsIcon = () => (
  <svg {...SVG}>
    <line x1="2" y1="3.5" x2="12" y2="3.5" />
    <line x1="9" y1="2" x2="9" y2="5" />
    <line x1="2" y1="7" x2="12" y2="7" />
    <line x1="5" y1="5.5" x2="5" y2="8.5" />
    <line x1="2" y1="10.5" x2="12" y2="10.5" />
    <line x1="9" y1="9" x2="9" y2="12" />
  </svg>
);
const LiveIcon = () => (
  <svg {...SVG}>
    <path d="M1.5 7h2.6l1.4-3.6 2.2 7 1.4-3.4h3.4" />
  </svg>
);
const SearchIcon = () => (
  <svg {...SVG}>
    <circle cx="6" cy="6" r="3.6" />
    <line x1="8.7" y1="8.7" x2="11.8" y2="11.8" />
  </svg>
);
const CloseIcon = () => (
  <svg {...SVG}>
    <path d="M3.2 3.2L10.8 10.8M10.8 3.2L3.2 10.8" />
  </svg>
);

export type ManageSection =
  | "sessions"
  | "learning"
  | "plugins"
  | "instances"
  | "settings";

interface SectionDef {
  id: ManageSection;
  label: string;
  desc: string;
  Icon: () => React.ReactElement;
}

// Memory + Runs stay under Learning (per the confirmed brief), so the rail is
// five flat items.
const SECTIONS: SectionDef[] = [
  {
    id: "sessions",
    label: "Sessions",
    desc: "Full-text search across every past Claude Code and Codex session.",
    Icon: SessionsIcon,
  },
  {
    id: "learning",
    label: "Learning",
    desc: "Learned rules, memory optimization, and analysis run history.",
    Icon: LearningIcon,
  },
  {
    id: "plugins",
    label: "Plugins",
    desc: "Install, enable, and update agent plugins and marketplaces.",
    Icon: PluginsIcon,
  },
  {
    id: "instances",
    label: "Instances",
    desc: "Restart and manage running Claude Code and Codex instances.",
    Icon: InstancesIcon,
  },
  {
    id: "settings",
    label: "Settings",
    desc: "Integrations, context, learning, and performance configuration.",
    Icon: SettingsIcon,
  },
];

const SECTION_IDS = SECTIONS.map((s) => s.id);
const STORAGE_KEY = "quill-manage-section";
const IS_MAC =
  typeof navigator !== "undefined" && navigator.userAgent.includes("Mac");

function resolveInitialSection(): ManageSection {
  const fromUrl = new URLSearchParams(window.location.search).get("section");
  if (fromUrl && SECTION_IDS.includes(fromUrl as ManageSection)) {
    return fromUrl as ManageSection;
  }
  try {
    const stored = localStorage.getItem(STORAGE_KEY);
    if (stored && SECTION_IDS.includes(stored as ManageSection)) {
      return stored as ManageSection;
    }
  } catch {
    /* ignore */
  }
  return "sessions";
}

function ManageNoProvider({
  section,
  onGoToSettings,
}: {
  section: SectionDef;
  onGoToSettings: () => void;
}) {
  const Icon = section.Icon;
  return (
    <div className="manage-noprovider">
      <span className="manage-noprovider-icon">
        <Icon />
      </span>
      <h2 className="manage-noprovider-title">No active provider</h2>
      <p className="manage-noprovider-text">
        Enable Claude Code or Codex to use {section.label}.
      </p>
      <button
        type="button"
        className="manage-noprovider-btn"
        onClick={onGoToSettings}
      >
        Open Settings
      </button>
    </div>
  );
}

function ManageWindowView() {
  const [active, setActive] = useState<ManageSection>(resolveInitialSection);
  const [version, setVersion] = useState("");
  const integrations = useIntegrations();
  const railRefs = useRef<(HTMLButtonElement | null)[]>([]);
  const [paletteOpen, setPaletteOpen] = useState(false);

  useEffect(() => {
    getVersion()
      .then(setVersion)
      .catch(() => {});
  }, []);

  useEffect(() => {
    try {
      localStorage.setItem(STORAGE_KEY, active);
    } catch {
      /* ignore */
    }
  }, [active]);

  // The titlebar cog (and any deep-link) reuses an already-open Manage window;
  // it emits `manage:navigate` so we switch sections instead of being ignored.
  useEffect(() => {
    const un = listen<string>("manage:navigate", (e) => {
      if (SECTION_IDS.includes(e.payload as ManageSection)) {
        setActive(e.payload as ManageSection);
      }
    });
    return () => {
      void un.then((fn) => fn());
    };
  }, []);

  // ⌘K / Ctrl+K toggles the command palette.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && (e.key === "k" || e.key === "K")) {
        e.preventDefault();
        setPaletteOpen((o) => !o);
      }
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, []);

  const handleClose = useCallback(async () => {
    await getCurrentWindow().close();
  }, []);

  const handleBackToLive = useCallback(async () => {
    const main = await WebviewWindow.getByLabel("main");
    await main?.show();
    await main?.setFocus();
  }, []);

  // Palette commands: jump to any section, plus the two window-level actions.
  const commands = useMemo<PaletteCommand[]>(
    () => [
      ...SECTIONS.map((s) => ({
        id: `section:${s.id}`,
        label: s.label,
        hint: "Section",
        Icon: s.Icon,
        run: () => setActive(s.id),
      })),
      {
        id: "action:live",
        label: "Back to Live",
        hint: "Window",
        Icon: LiveIcon,
        run: () => void handleBackToLive(),
      },
      {
        id: "action:close",
        label: "Close Tools",
        hint: "Window",
        Icon: CloseIcon,
        run: () => void handleClose(),
      },
    ],
    [handleBackToLive, handleClose],
  );

  const onRailKeyDown = useCallback(
    (e: React.KeyboardEvent, index: number) => {
      if (e.key === "ArrowDown" || e.key === "ArrowUp") {
        e.preventDefault();
        const delta = e.key === "ArrowDown" ? 1 : -1;
        const next = (index + delta + SECTIONS.length) % SECTIONS.length;
        setActive(SECTIONS[next].id);
        railRefs.current[next]?.focus();
      }
    },
    [],
  );

  const current = SECTIONS.find((s) => s.id === active) ?? SECTIONS[0];
  // Settings is always reachable (so a provider can be enabled); the other
  // sections show an inline no-provider state when none is active. While
  // integrations are still loading, hold the section behind a loading state so
  // we don't mount it and fire backend calls only to swap it straight out.
  const providerPending = active !== "settings" && integrations.loading;
  const showNoProvider =
    active !== "settings" &&
    !integrations.loading &&
    !integrations.hasEnabledProvider;

  return (
    <div className="manage-window">
      <div className="manage-titlebar" data-tauri-drag-region>
        <span className="manage-titlebar-title" data-tauri-drag-region>
          Tools
        </span>
        <button
          type="button"
          className="manage-titlebar-close"
          onClick={() => void handleClose()}
          aria-label="Close"
        >
          <svg {...SVG} width={13} height={13}>
            <path d="M3.2 3.2L10.8 10.8M10.8 3.2L3.2 10.8" />
          </svg>
        </button>
      </div>

      <div className="manage-body">
        <nav className="manage-rail" aria-label="Workspace sections">
          <button
            type="button"
            className="manage-rail-search"
            onClick={() => setPaletteOpen(true)}
            aria-label="Open command palette"
          >
            <span className="manage-rail-search-icon">
              <SearchIcon />
            </span>
            <span className="manage-rail-search-label">Search</span>
            <kbd className="manage-rail-kbd">{IS_MAC ? "⌘K" : "Ctrl K"}</kbd>
          </button>
          <ul className="manage-rail-nav" role="list">
            {SECTIONS.map((s, i) => {
              const isActive = s.id === active;
              const Icon = s.Icon;
              return (
                <li key={s.id}>
                  <button
                    type="button"
                    ref={(el) => {
                      railRefs.current[i] = el;
                    }}
                    className={`manage-rail-item${isActive ? " active" : ""}`}
                    aria-current={isActive ? "page" : undefined}
                    onClick={() => setActive(s.id)}
                    onKeyDown={(e) => onRailKeyDown(e, i)}
                  >
                    <span className="manage-rail-icon">
                      <Icon />
                    </span>
                    <span className="manage-rail-label">{s.label}</span>
                  </button>
                </li>
              );
            })}
          </ul>

          <div className="manage-rail-footer">
            <button
              type="button"
              className="manage-rail-back"
              onClick={() => void handleBackToLive()}
            >
              <span className="manage-rail-icon">
                <LiveIcon />
              </span>
              <span className="manage-rail-label">Live</span>
            </button>
            {version && <span className="manage-rail-version">v{version}</span>}
          </div>
        </nav>

        <main className="manage-content" aria-label={current.label}>
          <Suspense
            fallback={
              <div className="manage-loading">Loading {current.label}…</div>
            }
          >
            {providerPending ? (
              <div className="manage-loading">Loading {current.label}…</div>
            ) : showNoProvider ? (
              <ManageNoProvider
                section={current}
                onGoToSettings={() => setActive("settings")}
              />
            ) : active === "sessions" ? (
              <SessionsSection />
            ) : active === "learning" ? (
              <LearningSection />
            ) : active === "plugins" ? (
              <PluginsSection />
            ) : active === "instances" ? (
              <InstancesSection />
            ) : (
              <SettingsSection />
            )}
          </Suspense>
        </main>
      </div>

      <CommandPalette
        open={paletteOpen}
        onClose={() => setPaletteOpen(false)}
        commands={commands}
      />
    </div>
  );
}

export default ManageWindowView;
