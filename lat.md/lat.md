This directory defines the high-level concepts, business logic, and architecture of this project using markdown. It is managed by [lat.md](https://www.npmjs.com/package/lat.md) — a tool that anchors source code to these definitions. Install the `lat` command with `npm i -g lat.md` and run `lat --help`.

- [[architecture]] — Tech stack, multi-window design, module map, communication layers
- [[frontend]] — React components, custom hooks, state management, types, styling
- [[backend]] — Rust modules, database schema, HTTP API, IPC commands, events
- [[features]] — Live usage, analytics, learning, session search, restart, memory optimizer
- [[data-flow]] — Token reporting, learning analysis, session indexing, and memory optimization
- [[infrastructure]] — CI/CD pipeline, release process, build config, code quality, scripts
- [[runtime-rollup-tests]] — Runtime finalization, replacement, and ingest-budget test specs
- [[model-rollup-tests]] — Model backfill resume, authority, handoff, and maintenance test specs
- [[rollup-retention-tests]] — Fold-before-prune coverage and hourly-authority test specs
- [[frontend-cache-tests]] — Frontend invoke cache, refresh cadence, and lifecycle test specs
- [[widget-range-tests]] — Exact comparison windows and unique breakdown query test specs
- [[view-reader-tests]] — Slow-reader contention and concurrent-ingest test specs
