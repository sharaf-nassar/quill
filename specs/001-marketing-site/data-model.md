# Data Model — Quill Marketing Site

The marketing site has no runtime database. Its operational data model covers the dev-only browser fixture profile, canonical screenshot assets, and static site files.

## 1. Browser marketing fixture profile

The publishing workflow renders the actual React app in plain-browser development mode. `src/main.tsx` installs `src/mocks/installBrowserMock.ts`, which routes normal Tauri IPC calls to `src/mocks/ipcFixtures.ts`.

The URL query `screenshot=marketing` selects the publishing profile. It exists only behind `import.meta.env.DEV` and therefore cannot ship in production.

| Domain | Marketing fixture content |
|---|---|
| Providers | Claude Code, Codex, and Pi enabled; MiniMax filtered out |
| Limits | Claude and Codex windows spanning normal, warning, stale, and critical states |
| Usage models | Opus, Sonnet, Terra, Sol, and Pi-routed Fable with staggered six-hour curves |
| Live sessions | Claude, Pi, and Codex rows with full project names, agent counts, live agent models, runtime, turns, and tokens |
| Model view | One running-now row per Claude/Codex/Pi and five ranked models |
| Session Search | `parser` query with one result per Claude/Codex/Pi plus selected surrounding context |
| Learning | Active shared/Claude rules and one Codex candidate |
| Memories | Four projects and four provider-aware files |
| Context | Preservation, retrieval, routing, and source-reuse totals |
| Settings | Claude/Codex/Pi integrations, context preservation, telemetry, and Brevity enabled; retention watermark absent |

**Invariants**:

- Fixture values are deterministic and contain no real paths, hosts, sessions, prompts, or credentials.
- The visible `MOCK DATA` badge remains present in ordinary Browser Mock Mode but is suppressed for the marketing profile.
- Marketing-specific data selection may alter fixture responses only. It must not branch production component behavior.
- Empty marketed views are defects in `src/mocks/ipcFixtures.ts`, not a reason to fabricate pixels or seed SQLite.

The sandboxed Tauri launcher and Python seeder remain separate backend-development tools. They are not part of the marketing screenshot data model.

## 2. Screenshot asset naming

Captured PNGs land in `marketing-site/assets/screenshots/`.

| Filename | Captured from | Used by section |
|---|---|---|
| `hero.png` | Widget → Usage, 6H, Model, Sessions | `#hero`, `#analytics` |
| `live.png` | Exact copy of `hero.png` | `#live` |
| `models.png` | Widget → Models, 7D | `#models` |
| `analytics-context.png` | Widget → Context, 6H | `#context` |
| `sessions.png` | Tools → Sessions, query + detail | `#search` |
| `learning.png` | Tools → Learning → Rules | `#learning` |
| `memory.png` | Tools → Learning → Memories | `#memory` |
| `settings.png` | Tools → Settings → Integrations | `#integrations` |
| `brevity.png` | Tools → Settings → Context | `#brevity` |
| `logo.png` | Real Quill app icon | header + favicon |

**Capture conventions**:

- `scripts/capture_screenshots_docker.sh` is the publishing entry point.
- `scripts/capture_browser_screenshots.mjs` drives the app through Chrome DevTools Protocol.
- Widget viewports are 480×800 CSS pixels; Tools viewports are 960×680; device scale factor is 2.
- Widget PNGs are 960×1600. Tools PNGs are 1920×1360.
- Chromium captures the rendered surface directly. No image generator, HTML reimplementation, post-capture scaling, or manual crop participates.
- The Node driver validates PNG headers, dimensions, and `hero.png == live.png`.
- Every HTML use has authored alt text and explicit source dimensions.

## 3. Runtime isolation

The Docker image contains Node, Chromium, npm dependencies, and frontend source only.

At runtime:

- Docker networking is disabled.
- No host display socket, home directory, Quill database, transcript root, or provider config is mounted.
- Vite and Chromium communicate only over container loopback.
- The host wrapper first copies captures into a temporary directory and verifies all nine files exist before replacing tracked assets.

## 4. Site source layout

```text
marketing-site/
├── index.html
├── styles.css
├── motion.js
├── README.md
└── assets/
    ├── fonts/
    ├── screenshots/
    ├── logo.png
    └── logo-mark.png
```

**Discipline**:

- One HTML page; no client-side router.
- One CSS file and local fonts only.
- JavaScript is progressive enhancement; content and anchors work without it.
- No tracking, third-party analytics, remote fonts, or third-party runtime scripts.

## Relationships

- Browser Mock Mode owns screenshot data.
- The CDP driver owns navigation and capture.
- The Docker wrapper owns isolation and host publication.
- `marketing-site/index.html` consumes canonical assets.

There is no runtime data model for the deployed site itself.
