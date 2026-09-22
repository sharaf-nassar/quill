# CPA Regression Tests

CPA regressions protect connection isolation, observation truthfulness, bounded polling, and provider-scoped quota semantics without accessing live credentials or services.

## Atomic lifecycle storage

Connection replacement and disconnect roll back all settings/runtime/history changes on SQLite failure. Direct-provider history survives every CPA purge; same-endpoint reconnect retains CPA analytics while resetting runtime state.

## Authoritative snapshots and timestamps

Successful full reads replace the entire window set. Failed reads retain last-known values and observation times, not newly sampled history. Repeated passive timestamps add no samples, and a later full success clears failure/backoff state.

## Passive observation truthfulness

Only fresh recognized headers produce partial observations. Claude fractions become percentages; Codex percentages remain percentages.

Missing/future/expired observation timestamps and non-default active-limit namespaces cannot imply default quota. Inventory retrieval time is not a fallback for missing quota observation time: CPA omits that field when `QuotaState.ObservedAt` is zero. Missing scoped coverage still requires an active read.

## Scoped Codex quota and credits

Default, code-review, and additional limits retain distinct keys even at equal durations. Scoped exhaustion never excludes the whole account.

Credit balance and zero applicable reset-credit counts remain supplementary metadata, never capacity percentages.

## Fair bounded scheduling

Oldest-attempt-first selection reaches accounts beyond the 16-call cap. Launches remain 250ms apart with at most three active requests. A management rejection or CPA transport failure observed during the stagger prevents subsequent launches.

## Management authentication suppression

Persisted management rejection suppresses automatic and forced refresh after restart. Different endpoints cannot inherit old source state; explicit reconnect clears suppression.

No deliberately invalid credential is sent to a live CPA service.

## Truthful widget quota state

CPA rows omit diagnostic paragraphs about observation times, quota state, retries, credits, and model cooldowns. Cached account and pool values retain utilization colors; auth/server failures never label sync as live.

Equal-duration scoped windows remain independently labeled. Percentages, account status labels, and per-window reset timers remain visible.

## Partial scoped responses

Malformed optional Codex scopes preserve valid default windows and valid sibling windows inside that scope. Partial readings retain the last complete scoped set with cached/error state until a clean full response replaces it.

## Inactive account diagnostics

Disabled or unavailable accounts retain local failure details but do not permanently degrade readable healthy siblings. A provider with no readable accounts still exposes its remaining quota state.

## HTTP rejection persistence

Real local fixture HTTP 401, 403, and 429 responses persist their classification and suppression through database reopen. Forced refresh cannot retry a rejected key or bypass Retry-After; management and upstream 429 scope remain distinct.

## Lifecycle serialization

Disconnect waits for the actual shared refresh lock. A simulated in-flight poll writes its final observations before disconnect atomically removes connection, runtime, and history, preventing post-purge resurrection.
