# Analysis: remove-plugin-system

## Coverage Table

| User story / requirement | Covered by (plan section) | Status |
|--------------------------|---------------------------|--------|
| Goal 1 — remove every visible Plugin Manager surface | Affected Components; API / Interface Changes; Remove the Plugin Manager frontend | full |
| Goal 2 — remove feature-owned frontend implementation | Affected Components; Testing Strategy residual audits; Remove the Plugin Manager frontend | full |
| Goal 3 — remove Rust/Tauri domain, IPC, settings, events, and polling | Affected Components; Data Model; API / Interface Changes; Remove the Rust plugin-management backend | full |
| Goal 4 — remove feature-exclusive dependencies, configuration, generated material, scripts, assets, and active docs | Affected Components; Testing Strategy broad ownership audit; Synchronize active product and architecture material | full |
| Goal 5 — synchronize active `lat.md` architecture | Affected Components; Testing Strategy; Synchronize active product and architecture material | full |
| Goal 6 — retain no executable compatibility path | Architecture Approach; API / Interface Changes; residual audits; Audit boundaries and validate the release | full |
| Goal 7 — pass all applicable zero-warning gates | Testing Strategy; Audit boundaries and validate the release | full |
| US-1 — obsolete Tools destination is gone | Frontend inventory; Manage interface changes; default/stale navigation, rail, palette, keyboard, focus, and responsive smoke checks | full |
| US-2 — all Plugin Manager work and provider mutation stop | Backend inventory; removed commands/events; no-I/O trace; inert-row and provider-state before/after evidence | full |
| US-3 — dead feature abstractions are deleted | Explicit deletion inventory; no-stub breaking contract; source/build/residual checks | full |
| US-4 — current product documentation and assets are truthful | README, screenshot/script, DESIGN/sidecar, and six `lat.md` targets; classified historical boundary | full |
| US-5 — supported surrounding behavior remains | Provider detection/status, updater/process/window-state ownership, analytics evidence, settings, title bar, and four-section Tools validation | full |
| Clarification 1 — provider-owned state remains untouched | Architecture Approach; Data Model; runtime evidence; rollback | full |
| Clarification 2 — old settings rows remain inert | Data Model; upgrade smoke; no-migration boundary | full |
| Clarification 3 — stale destinations use generic Sessions fallback | Architecture Approach; API / Interface Changes; stale navigation smoke | full |
| Clarification 4 — active material changes while history remains | Affected Components; broad residual classification; docs sequencing | full |
| Clarification 5 — shared clients/framework plugins remain | Architecture Approach; Affected Components; ownership and preservation checks | full |
| Clarification 6 — existing gates and manual evidence, no new tests | Testing Strategy; Constitution Check; final validation task | full |

## Remaining Risks

- **Provider-state observation touches real read-only paths.** `QUILL_DATA_DIR`
  isolates Quill data but provider plugins live under provider-owned locations.
  Validation must never override `$HOME`, create missing paths, or invoke
  mutations. `stat`/hash before-and-after evidence plus the 45-second process,
  file, and network trace exposes accidental access or writes.
- **Broad terminology has legitimate survivors.** Framework packages, Tauri
  protocol names, transcript analytics, and history use `plugin` accurately.
  The final Bead notes must classify every retained broad-search hit; exact
  feature-symbol and owned-path searches provide the zero-hit boundary.
- **Deletion has no newly added regression tests.** This is an approved
  authorization constraint, not an omission. Mitigation is reviewer-verifiable
  command output and one recorded result for every manual smoke row.
- **Rust and TypeScript contracts can drift during parallel work.** Backend and
  frontend tasks run independently but neither docs nor final validation starts
  until both join; each task has layer-specific compile and symbol gates.
- **Runtime tracing is Linux-specific.** Cross-platform safety comes from
  deleting platform-neutral feature code, preserving framework configuration,
  and running existing build/tests. Platform-specific UI behavior remains a
  release-validation concern if the implementation environment exposes those
  targets.
- **Rollback details are execution-time evidence.** The validation task must pin
  the pre-removal source commit and copied database/provider fixture in its Bead
  notes, then verify source-only restoration in a disposable worktree. No
  migration rollback exists because no state migration occurs.
- **Concurrent `main` movement can conflict with adjacent work.** The molecule
  worktree uses `specs/016-*` to avoid the active `specs/015-*` worktree.
  Integration must re-detect rewritten or advanced `main`, rebase appropriately,
  and inspect conflicts before squash.

All risks have a planned mitigation and none requires a new product decision.

## Unresolved Questions

None. All six clarification decisions are reflected in the specification and
plan. Implementation must choose temporary fixture paths and complete the
ownership audit, but those are execution details rather than open requirements.

## Constitution Check

| Principle | Verdict | Evidence |
|-----------|---------|----------|
| 1. Local source-backed truth | pass | Provider data, analytics terms, and historical records stay authoritative; residual hits are classified. |
| 2. Established stack and boundaries | pass | The complete React and Rust/Tauri vertical slice is removed while shared owners remain. |
| 3. Responsive execution | pass | Timers, scans, subprocesses, catalog calls, and managed checker state are deleted without replacement work. |
| 4. Recoverable mutation | pass | No database purge or provider mutation occurs; rollback is source-only against unchanged state. |
| 5. Typed failure boundaries | pass | Supported contracts remain typed; removed invokes fail as unknown and invalid sections use generic validation. |
| 6. Zero-warning quality gates | pass | Frontend, Rust, build, hook-runner, residual, diff, and `lat check` gates are mandatory. |
| 7. Authorized behavior testing | pass | No tests are added; existing tests and approved manual evidence are used. |
| 8. Architecture traceability | pass | Six active `lat.md` files are synchronized after code shape joins and validated. |
| 9. Glass Cockpit discipline | pass | Four-item Tools layout, density, focus, keyboard, responsive, and state behavior are explicitly preserved. |
| 10. Measured performance | pass | No speed claim is made. Absence of plugin-owned background work is measured as zero matching file, process, RPC, or network evidence during the 45-second trace. |
| 11. Explicit external transmission | pass | Marketplace/catalog transmission and provider mutations are removed; no new transmission is introduced. |
| 12. Gated delivery | pass | Beads DAG, two human gates, worktree integration, quality checks, and no unauthorized push are explicit. |

No constitution tension or violation remains.

## Recommendation

**GO** — Every goal, user story, clarification, constraint, and constitution
principle has complete plan ownership and verifiable evidence. The feature is a
high-confidence vertical deletion with no migration or external-state mutation;
remaining risks are implementation controls already assigned to the DAG and do
not require further specification changes.
