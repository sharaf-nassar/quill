# Feature Specification: Quill Marketing Site (GitHub Pages)

**Feature Branch**: `001-marketing-site`
**Created**: 2026-05-08
**Status**: Draft
**Input**: User description: "we want to create a marketing page for Quill on the github pages for this repo. we want te page to match the app's theme. the page should do an excellent job at highlighting quill's most useful features and how the analytics and insight help when using llms. we want to include updated screenshots with dummy data - ensure to run a separate instance of quill with dummy data as we are running it locally. do as much research as you need to ensure the best quality and ensure we are creating a unique and profressional page"

## Clarifications

### Session 2026-05-08

- Q: Which visual direction should the Quill marketing site take? → A: **Terminal Console** — feels like a sibling of the desktop window. Mono headlines, ASCII rules, inline `[OK]`/`[!]`/`[ERR]` status pills, 11–13px body density echoing the app, color reserved for status (not decoration). Lowest brand-mismatch risk; highest authenticity for the CLI/TUI-adjacent audience.
- Q: How should the site be structured? → A: **Single page with anchored sections** — one HTML deliverable with smooth-scroll between `#hero`, `#live`, `#analytics`, `#context`, `#search`, `#learning`, `#install`. Anchored URLs are deep-link shareable. Lightest tooling, no router, matches the continuous-scroll feel of the chosen visual direction.
- Q: How should the dummy-data Quill instance be isolated from the maintainer's personal Quill state? → A: **Env-var path override + launcher script.** Quill MUST read `QUILL_DATA_DIR` and `QUILL_RULES_DIR` (and any other override needed for hooks/learned-rules dirs) at startup before resolving any default path; `scripts/populate_dummy_data.py` MUST accept matching `--data-dir` / `--rules-dir` flags; a cross-platform launcher (`scripts/run_quill_demo.sh` plus a PowerShell equivalent) MUST set up a sandbox dir, run the seeder against it, and launch Quill against it. Production builds MUST NOT silently honor these overrides without an explicit opt-in flag (e.g., `QUILL_DEMO_MODE=1`) so a stray env var never redirects a user's real Quill.
- Q: How should the site publish to GitHub Pages? → A: **GitHub Actions workflow.** A `.github/workflows/pages.yml` builds (or copies) the site source from `marketing-site/` and deploys via the official `actions/deploy-pages` action on merges to the default branch. Site source lives under `marketing-site/` in this repo. This contract is portable across plain HTML and any future static-site-generator without re-architecting.
- Q: How rich should the hero demo media be? → A: **Static screenshot + lightweight CSS micro-animation** — the hero anchors on a high-quality screenshot of the main window, augmented with a small CSS/SVG motion accent (e.g., a pace marker advancing across a faux usage row, or a sparkline drawing once on scroll-into-view). No `<video>` tag. Total motion overhead under ~5 KB. MUST gracefully degrade to the static screenshot under `prefers-reduced-motion: reduce` and with JavaScript disabled.

### Session 2026-05-10

- Q: The first shipped page looks insufficiently professional; how should the redesign evolve without becoming generic SaaS or AI slop? → A: **Instrument Dossier** — a light, editorial evidence-packet direction. The page uses warm technical paper, hard rules, serif display type, mono labels, cobalt action color, and dark product screenshots as the primary proof. It stays static, avoids third-party assets, and keeps the same seven anchored sections.

### Session 2026-05-11

- Q: The evidence-packet page still feels too flat and unimpressive; how should the page move further upmarket? → A: **Signal Theater** — a dark, cinematic desktop-instrument direction. The page keeps real Quill screenshots and stable anchors, but uses asymmetric hero composition, gapless screenshot bento, horizontal proof accordion, GSAP-pinned scroll narrative, scrubbed text reveal, and a focused install close. JavaScript is progressive enhancement only; content remains readable without it.

### Session 2026-05-12

- Q: The Signal Theater palette used lime/chartreuse that does not match the Quill app or real logo; what should the brand palette be? → A: **Logo-aligned dark Quill palette** — use the actual quill icon in the marketing header and favicon, keep the app's `#121216` dark base, use cyan and purple from the logo for brand accents, and reserve green/yellow/red for product status semantics.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - First-time visitor evaluates Quill in under a minute (Priority: P1)

A developer who already uses Claude Code or Codex hears about Quill (link from social media, README, or word of mouth), opens the marketing site, and within ~30 seconds understands what Quill does, sees what the app actually looks like, and either clicks through to the GitHub repo to install or bookmarks it for later.

**Why this priority**: This is the conversion path. Every other story exists to support it. If a visitor cannot understand the product within seconds and cannot see what they'll be installing, no other section of the site matters. A working landing experience with a clear hero and at least one screenshot is the MVP.

**Independent Test**: Open the deployed page on a clean browser at desktop and mobile widths, time-box a 30-second comprehension test with a developer unfamiliar with Quill, confirm they can describe the product in one sentence and locate both the primary call-to-action and a screenshot of the live app.

**Acceptance Scenarios**:

1. **Given** a visitor lands on the page on a 1366×768 viewport, **When** the page first paints, **Then** the hero headline, supporting one-liner, primary call-to-action, and at least one in-app screenshot are visible without scrolling.
2. **Given** a visitor reads only the hero, **When** they describe the product to a peer, **Then** they correctly identify Quill as a usage and analytics companion for Claude Code and Codex (not a chat client, not a CLI replacement).
3. **Given** a visitor wants to install or learn more, **When** they click the primary call-to-action, **Then** they reach the GitHub repository or releases page.
4. **Given** a visitor on a 375px-wide mobile screen, **When** they scroll the hero, **Then** the layout reflows with no horizontal scrollbar and the call-to-action remains tappable.

---

### User Story 2 - Visitor explores feature deep-dives with annotated screenshots (Priority: P2)

A visitor curious enough to scroll past the hero wants to see what each feature actually looks like and why it matters when working with an LLM. They scroll through dedicated sections covering live usage limits, the analytics dashboard, context savings, session search, and the learning system, each with at least one screenshot of the real UI populated with realistic-looking data and a short benefit-oriented description.

**Why this priority**: Developers who use Claude Code or Codex daily are skeptical of marketing copy and trust screenshots. This section converts curiosity into intent-to-install. It is independently testable: removing it would still leave a working hero (P1), but the page would be much less persuasive.

**Independent Test**: Verify each declared feature has its own clearly labelled section, a description tying the feature to a real user benefit (e.g. "see exactly how much of your Claude Pro subscription is left before the next reset"), and at least one screenshot. Confirm screenshots show only dummy data — no real project names, host names, session contents, or personal identifiers.

**Acceptance Scenarios**:

1. **Given** the published site, **When** a visitor scrolls past the hero, **Then** they encounter sections for at least Live Usage, Analytics Dashboard, Context Savings, Session Search, and Learning System — in that order or another deliberate narrative order.
2. **Given** any feature section, **When** the visitor reads the heading, **Then** the heading communicates a benefit (e.g., "Know your limits before you hit them") rather than a feature label alone.
3. **Given** any screenshot on the site, **When** an observer inspects it, **Then** every visible identifier (project name, host, session text, branch name, file path) is fictional or generic dummy data.
4. **Given** the Analytics Dashboard section, **When** the visitor reads it, **Then** the description explicitly explains how analytics and insights help while using an LLM (latency, token efficiency, context savings, code velocity, routing cost) rather than just listing chart types.

---

### User Story 3 - Maintainer regenerates screenshots without leaking real data (Priority: P2)

A Quill maintainer needs to refresh the marketing screenshots after a UI change. They run a separate, dedicated instance of Quill on their development machine that points at a pre-seeded dummy dataset (fake projects, fake sessions, plausible token counts, fake learned rules), capture the relevant views, and replace the screenshots in the site source — all without touching their personal `~/.claude` or `~/.codex` directories or the Quill database their day-to-day work relies on.

**Why this priority**: Without this workflow, screenshots either go stale (the site silently misrepresents the product) or maintainers ship real personal data publicly. This story is independently testable from the visitor-facing pages: a maintainer can validate the dummy-data instance produces good screenshots before any site code changes.

**Independent Test**: Follow the documented dummy-instance workflow from a clean checkout on a development machine, verify the resulting Quill window shows realistic-looking but obviously fictional data across every screen the marketing site references, capture screenshots, and confirm none of the captured pixels contain real local data.

**Acceptance Scenarios**:

1. **Given** a maintainer wants new screenshots, **When** they follow the documented dummy-instance procedure, **Then** they obtain a running Quill window populated with dummy data without overwriting their normal Quill database, settings, or provider configuration.
2. **Given** the dummy data is loaded, **When** the maintainer opens each marketed view (live usage, all analytics tabs, sessions, learning, context tab), **Then** every view renders with non-empty, plausible-looking data.
3. **Given** a freshly captured screenshot set, **When** the maintainer inspects every image, **Then** zero real personal data is visible (no real project paths, hostnames, session prompts, or git branch names).
4. **Given** the maintainer finishes capturing, **When** they tear down the dummy instance, **Then** their personal Quill installation is unchanged.

---

### User Story 4 - Developer evaluates technical fit (Priority: P3)

A developer past the marketing pitch wants to confirm Quill fits their setup before they install: which providers it supports (Claude Code, Codex, MiniMax), what platforms it runs on, how it integrates (hooks, MCP), what data it stores locally, and where the source lives.

**Why this priority**: This audience is smaller and is largely served by the linked GitHub README. A "Built for / Integrates with / Runs on" strip and clear repo links satisfy most of it without duplicating the README.

**Independent Test**: Visit the deployed page, confirm the supported-provider, supported-platform, and "view source / star on GitHub" affordances are all present and reach the right destinations.

**Acceptance Scenarios**:

1. **Given** the page, **When** the developer scans for "what providers does this support?", **Then** they find an explicit list naming Claude Code, Codex, and MiniMax.
2. **Given** the page, **When** the developer wants to see the source, **Then** a clearly labelled link reaches the GitHub repository.
3. **Given** the page, **When** the developer wants the latest release, **Then** a clearly labelled link reaches the GitHub releases page.

---

### Edge Cases

- **Stale screenshots after a UI change**: The site claims to show the product accurately. If a UI change ships and screenshots are not regenerated, the site lies. Mitigation surface: a single documented capture procedure, screenshots versioned in the repo so visual diffs surface in pull requests.
- **Real personal data accidentally captured**: A maintainer in a hurry captures from their personal Quill instance instead of the dummy one. Mitigation surface: dummy instance must be visually distinct (e.g., obviously-fake project names like "Acme Demo", "Internal Tooling") so reviewers spot real data immediately.
- **Page rendered with JavaScript disabled**: Some readers (security-conscious developers, accessibility tools, link previews, search-engine crawlers) request the page without executing scripts. Hero content, screenshots, and primary call-to-action must still be readable.
- **GitHub Pages outage or rate limit**: GitHub Pages occasionally has outages. The site must fail gracefully (browser-default error rather than a half-rendered broken state); no critical content should depend on third-party CDNs that could be unreachable.
- **Visitor on a high-DPI display**: Screenshots captured at 1× look blurry on Retina/HiDPI displays. Captures should be at 2× or use formats that scale cleanly.
- **Visitor with motion sensitivity**: Any animated decorations must respect the `prefers-reduced-motion` user preference.
- **Large viewport (4K/ultrawide)**: Content must not stretch to absurd line lengths; readable measure must be enforced even on wide viewports.
- **Search-engine crawler**: Page should be discoverable for queries like "Claude Code usage tracker", with appropriate page title, meta description, and OpenGraph image.
- **Social-share preview**: When linked on X, Bluesky, or Slack, the preview should show a meaningful image (a hero screenshot or the Quill logo) and a meaningful one-line description, not just the URL.
- **Visitor who lands deep-link on a feature anchor**: Direct links to feature sections (`#analytics`, `#context`) should scroll smoothly and account for any sticky header.

## Requirements *(mandatory)*

### Functional Requirements

#### Site delivery and hosting

- **FR-001**: The marketing site MUST be hosted on GitHub Pages from this repository, reachable at the project's GitHub Pages URL with no paid services required.
- **FR-002**: The site source MUST live inside this repository under a `marketing-site/` directory so site changes are reviewable in the same pull-request flow as app changes.
- **FR-003**: The site MUST publish automatically via a GitHub Actions workflow (`.github/workflows/pages.yml`) using the official `actions/deploy-pages` action on every merge to the default branch — no manual upload step. Workflow runs MUST be observable in the Actions UI and a failed deploy MUST surface as a failed Actions run.
- **FR-004**: The site MUST be a static deliverable (no server-side runtime required), consistent with GitHub Pages constraints.

#### Visual identity and theme

- **FR-005**: The site's visual identity MUST adopt the **Signal Theater** direction (chosen 2026-05-11, palette corrected 2026-05-12): the page reads as a premium desktop instrument panel for AI coding telemetry. It uses the real Quill logo mark, the app's dark `#121216` surface, clipped geometry, dense screenshot proof, cyan/purple logo accents, and scroll-driven motion as progressive enhancement.
- **FR-006**: The site MUST avoid generic SaaS-landing-page conventions (large rounded cards, glassmorphism, pastel gradients, hero-only-text with no product UI shown, oversized pill buttons, soft-shadow-heavy layout). Geometry MUST default to square or lightly rounded (≤6px corners) consistent with the Signal Theater direction.
- **FR-007**: Typography MUST be deliberate and dependency-free: a Cabinet Grotesk-first display stack with local fallbacks for headlines, a readable local sans stack for prose, and a mono stack (`ui-monospace`, `JetBrains Mono`, `SF Mono`, etc.) for labels, frame chrome, metric pins, code samples, and section eyebrows. The site MUST NOT import remote fonts or bundled Inter/Roboto.
- **FR-008**: Color contrast MUST meet WCAG 2.1 AA for body text, headlines, status pills, and call-to-action labels.

#### Content structure

- **FR-009**: The site MUST include a hero section with a headline, a one-line value proposition, a primary call-to-action, and a primary screenshot of the desktop app's main window. The screenshot MAY be augmented with CSS/JS motion that is not required for comprehension. The hero MUST NOT use a `<video>` tag. The hero MUST degrade to a fully readable static screenshot when JavaScript is disabled, GSAP fails to load, or `prefers-reduced-motion: reduce` is set.
- **FR-010**: The site MUST include dedicated feature sections for at least Live Usage, Analytics Dashboard (Now, Trends, Charts, Context tabs), Context Savings, Session Search, and Learning System.
- **FR-011**: Each feature section MUST include a benefit-oriented heading, a short description, and at least one screenshot showing that feature in the actual UI.
- **FR-012**: The Analytics section MUST explicitly explain *how analytics and insights help when working with an LLM* — covering at minimum: subscription-usage awareness (Pro/Max/Plus 5-hour and 7-day windows), latency visibility, token-efficiency feedback, context savings, code velocity, and routing-cost transparency.
- **FR-013**: The site MUST include a "supported providers" affordance naming Claude Code, Codex, and MiniMax with the correct integration semantics for each.
- **FR-014**: The site MUST include a "supported platforms" affordance covering the platforms Quill currently ships for.
- **FR-015**: The site MUST link to the GitHub repository, the latest releases page, and the project's existing documentation surface.
- **FR-016**: A short, accurate footer MUST identify the project, link the source, and credit the license.
- **FR-016a**: The site MUST be a single HTML page with anchored sections at minimum `#hero`, `#live`, `#analytics`, `#context`, `#search`, `#learning`, and `#install`. Deep links to any anchored section MUST land at the correct section on first paint, accounting for any sticky header offset. No client-side router is required.

#### Screenshots and dummy data

- **FR-017**: All UI screenshots MUST be captured from a separate Quill instance running against pre-seeded dummy data, not from any maintainer's personal Quill installation.
- **FR-018**: Isolation MUST be achieved through environment-variable path overrides. Specifically:
  - Quill MUST read `QUILL_DATA_DIR` and `QUILL_RULES_DIR` (and any additional override needed for provider hook directories) at startup, BEFORE it computes default paths, and use them when set.
  - Production behavior MUST be safe by default: the override MUST require an explicit opt-in (e.g., a `QUILL_DEMO_MODE=1` flag, a `--data-dir` CLI argument, or equivalent) so a stray env var in a maintainer's shell never redirects their real Quill installation to an unrelated directory.
  - `scripts/populate_dummy_data.py` MUST accept matching `--data-dir` / `--rules-dir` flags so the seeder can target an arbitrary sandbox without touching the platform default.
  - The repository MUST ship a cross-platform launcher (`scripts/run_quill_demo.sh` for POSIX shells and an equivalent PowerShell script for Windows) that creates a fresh sandbox directory, runs the seeder against it, and launches Quill against it — leaving the maintainer's `~/.local/share/com.quilltoolkit.app/`, `~/Library/Application Support/com.quilltoolkit.app/`, `~/.config/quill/`, `~/.claude/`, and `~/.cache/quill/` untouched.
- **FR-019**: Dummy data MUST be obviously fictional on inspection (project names, host names, branch names, learned-rule text) so a real-data leak would be immediately spotted at review time.
- **FR-020**: Dummy data MUST be plausible (non-empty, non-uniform, realistic-looking distributions of token counts, session lengths, time ranges) so screenshots demonstrate the product's value rather than empty states.
- **FR-021**: Screenshots MUST be captured at sufficient resolution to render crisply on high-DPI displays.
- **FR-022**: Every screenshot used on the site MUST cover at least one feature claimed nearby in copy, and every claimed feature MUST have at least one screenshot.

#### Responsiveness, performance, and resilience

- **FR-023**: The site MUST be usable on viewport widths from 320px (small mobile) up to 2560px (large desktop) without horizontal scroll.
- **FR-024**: Hero content (headline, one-liner, call-to-action, primary screenshot) MUST be readable with JavaScript disabled.
- **FR-025**: All animated or motion-driven decorations MUST respect the user's `prefers-reduced-motion` preference.
- **FR-026**: Page weight and asset loading MUST be lean enough to achieve a Lighthouse Performance score of at least 90 on both mobile and desktop emulations.
- **FR-027**: The site MUST set page title, meta description, OpenGraph, and Twitter card metadata so social shares preview correctly.
- **FR-028**: The site MUST NOT load any user-tracking or third-party analytics by default for v1 (privacy-respecting baseline; opt-in tracking is an explicit follow-up if ever added).

#### Maintainability

- **FR-029**: Adding, replacing, or reordering a feature section MUST be possible without restructuring the entire site.
- **FR-030**: A maintainer MUST be able to preview the site locally before opening a pull request.
- **FR-031**: The site source MUST be free of placeholder Lorem-ipsum, "TODO", or unfilled template strings at release time.

### Key Entities

- **Marketing Site**: The full GitHub-Pages-hosted static deliverable. Owns visual identity, copy, screenshot assets, and metadata. Lives in this repository.
- **Hero Section**: Above-the-fold visitor-conversion surface. Owns the headline, one-line value proposition, primary call-to-action, and primary screenshot.
- **Feature Section**: A repeatable content block with a benefit-oriented heading, short description, screenshot(s), and optional supporting copy. The site has one per highlighted feature.
- **Screenshot Asset**: An image file captured from the dummy-data Quill instance. Owns its high-DPI rendering and its mapping to a specific feature section.
- **Dummy Dataset**: A pre-seeded fictional state (projects, sessions, tokens, learned rules, context-savings events, plugins) used only by the screenshot-capture instance. Not tied to any real user data.
- **Capture Workflow**: The maintainer-facing procedure to spin up a Quill instance pointed at the dummy dataset, take screenshots, and shut it down without touching personal state.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: A first-time visitor unfamiliar with Quill can correctly describe what the product does in one sentence within 30 seconds of page load (validated with at least 5 unfamiliar developers).
- **SC-002**: At least 80% of those same visitors locate both the primary call-to-action and at least one in-app screenshot without prompting.
- **SC-003**: The hero (headline, one-liner, call-to-action, primary screenshot) is fully visible above the fold on a 1366×768 viewport.
- **SC-004**: The site achieves a Lighthouse Performance score of at least 90 on both mobile and desktop emulations.
- **SC-005**: Largest Contentful Paint is under 2.0 seconds on a simulated broadband connection.
- **SC-006**: Cumulative Layout Shift stays below 0.1 across the page lifecycle.
- **SC-007**: The site renders without horizontal scroll on viewport widths from 320px through 2560px and looks intentional at every step.
- **SC-008**: Every feature claimed in copy has at least one screenshot, and zero screenshots contain real personal data — verified by a pre-publish reviewer pass.
- **SC-009**: All body text, headlines, and call-to-action labels meet WCAG 2.1 AA contrast.
- **SC-010**: The site renders consistently on the latest two stable versions of Chromium, Firefox, and WebKit.
- **SC-011**: A maintainer can produce a new full screenshot set, end-to-end, in under 30 minutes following the documented capture workflow.
- **SC-012**: The first published version contains zero placeholder strings, broken internal links, or 404'd asset references.
- **SC-013**: When the site URL is shared on a chat or social platform, the preview shows a meaningful image and one-line description (no bare URL fallbacks).

## Assumptions

- The repository remains publicly hosted on GitHub and GitHub Pages for the repository is enabled (free tier is sufficient for v1).
- The marketing site is a static deliverable; no server-side runtime is needed. Forms (waitlist, contact, newsletter) are out of scope for v1; if introduced later they will use a third-party endpoint, not a backend service.
- A custom domain is not required for v1; the default `*.github.io` URL is acceptable. Custom domain support remains possible later without re-architecting.
- The chosen visual direction is the 2026-05-11 Signal Theater redesign. It aligns the marketing page with the app's dark technical UI while keeping screenshots as the source of visual truth. A pivot to generic SaaS styling remains explicitly out of scope.
- The site is single-page (or single-page-with-anchored-sections) for v1. A separate documentation site, blog, changelog page, or multi-page expansion is out of scope here and can be added later.
- Localization is out of scope for v1; English-only content is acceptable.
- User-tracking analytics on the marketing site are out of scope for v1 (privacy-respecting baseline). If introduced later, it will be a deliberate, opt-in decision with explicit copy.
- Screenshots are captured manually (or via a small documented helper) on a maintainer's machine. Automated cross-platform CI screenshot capture is out of scope for v1; the manual capture procedure must be reliable enough that this remains acceptable.
- The dummy-data instance can run on the same machine as a maintainer's personal Quill installation as long as it is fully isolated (separate config dir, separate database, separate hook installation paths) so personal state is never modified.
- Real-time content (current download counts, GitHub stars, latest version pulled live) is nice-to-have but not required for v1; a static "latest tested version" footnote is acceptable.
- The site must remain consistent with the existing project README and `lat.md/` documentation; if marketing copy ever diverges from the source of truth, the source of truth wins and the marketing copy must be updated.
