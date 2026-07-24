# Constitution

The engineering principles governing this repository. Read by the `speckit`
formula's spec-review, plan, and analyze gates — every feature is checked
against these.

## Principles

1. **Local source-backed truth** — Quill remains local-first; local evidence is
   authoritative, gaps stay explicit, and analytics never invent data.
   _Rationale:_ Trust and honest accounting define the product.
2. **Established stack and boundaries** — Extend Rust/Tauri domain, storage,
   and IPC layers plus React strict-TypeScript feature layers.
   _Rationale:_ This preserves existing cross-platform architecture and
   ownership.
3. **Responsive execution** — Keep database, network, and heavy I/O work off
   Tauri setup and UI threads, and bound background work.
   _Rationale:_ Blocking setup stalls window creation and damages
   responsiveness.
4. **Recoverable mutation** — Make state changes transactional, serialized
   where shared, reversible, and last-known-good preserving.
   _Rationale:_ User-owned configuration and local data must survive
   interruption.
5. **Typed failure boundaries** — Expected failures are typed and display-safe;
   unexpected failures retain context and are never silently swallowed.
   _Rationale:_ This enables region-local recovery without hiding defects.
6. **Zero-warning quality gates** — Applicable formatting, lint, typecheck,
   build, and existing tests must pass before completion or release.
   _Rationale:_ Consistent gates stop defects and warning debt from advancing.
7. **Authorized behavior testing** — Adding automated test code requires
   explicit user authorization; when authorized, pin invariants at their owning
   layer and link key tests one-to-one with `lat.md` specs.
   _Rationale:_ This distinguishes mandatory validation from permission to
   expand tests.
8. **Architecture traceability** — Update `lat.md` for behavior, architecture,
   or test changes and require `lat check` before completion.
   _Rationale:_ Implementation, design intent, and test specifications must stay
   synchronized.
9. **Glass Cockpit discipline** — `PRODUCT.md` and `DESIGN.md` govern UI
   changes, including two densities, semantic color, stable numerics, keyboard
   focus, contrast, and reduced motion.
   _Rationale:_ One instrument-grade vocabulary must span every surface.
10. **Measured performance** — Performance-sensitive work defines explicit
    budgets and demonstrates them with reproducible measurements.
    _Rationale:_ Responsiveness must be acceptance evidence, not subjective
    judgment.
11. **Explicit external transmission** — Off-device data transmission must be
    opt-in, minimal, scrubbed, documented, and user-controlled.
    _Rationale:_ Local-first privacy requires informed control over every
    external boundary.
12. **Gated delivery** — Release only after required gates, track work in
    Beads, and commit, sync, or push only with explicit authority.
    _Rationale:_ Review control and durable project state must survive every
    delivery workflow.
