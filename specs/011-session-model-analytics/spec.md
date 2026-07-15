# Feature Specification: Session Model Analytics

**Feature Branch**: `011-session-model-analytics`

**Created**: 2026-07-13

**Status**: Ready for Implementation

**Input**: User description: "$speckit-specify we want to add ssaving session model ids where relevant and add a new Models tabs to our analytics section that displays the most relevant data on models and their usage/history. the models ids should not be hardcoded though to ensure we never need to update the codebase to support new models the user might user."

## Overview

Give operators a trustworthy account of which concrete models their coding-agent
sessions used, how usage changed over time, and where attribution is incomplete.
Quill currently distinguishes providers but discards model identity from normal
session analytics, leaving users unable to compare model use or investigate
sessions that changed models.

The feature records every accepted model identifier exactly after the accepted
identifier's sole surrounding-whitespace removal. Model identifiers are data, not
a predefined product catalog: an identifier Quill has never seen before must
appear automatically without a Quill release, configuration change, alias update,
or pricing-table update.

A new Models tab presents model-attributed token history, coverage, model totals,
and session drill-downs. Historical sessions are reprocessed from all retained
local transcripts. Where the source does not contain reliable model identity,
the feature reports an honest attribution gap instead of guessing.

## Clarifications

### Session 2026-07-13

- Q: How much existing history should be backfilled? → A: Reprocess all locally
  available transcripts; records whose model cannot be recovered remain
  explicitly unattributed.
- Q: Should Quill store one model per session or individual model observations?
  → A: Preserve individual observations so model changes and subagent model use
  remain accurate.
- Q: What is included in the first Models tab? → A: Attribution coverage,
  distinct models, multi-model sessions, token history, model totals, provider
  filtering, and recent-session/model-switch detail. Cost, per-model runtime,
  model catalogs, and friendly-name mappings are deferred.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Identify models consuming tokens (Priority: P1)

A Quill user opens Analytics, selects Models, and sees which observed model
identifiers consumed tokens during the selected period. The user can compare
models using totals for input, output, cache creation, and cache reads while
retaining provider context.

**Why this priority**: This answers the primary questions behind the feature:
"which models am I using?" and "where are my tokens going?" Without this view,
capturing model identifiers delivers no operator value.

**Independent Test**: With model-attributed session activity from at least two
models, open Models and confirm both appear with provider identity, token totals,
turn count, session count, cache-read share, and first/last-seen timestamps.

**Acceptance Scenarios**:

1. **Given** attributed session activity from two models in the selected period,
   **When** the user opens Models, **Then** both raw model identifiers appear with
   correct provider and usage totals.
2. **Given** an accepted model identifier Quill has never encountered, **When** its
   first attributable session record is processed, **Then** the identifier
   appears automatically without a product or configuration update.
3. **Given** identical raw model identifiers reported by two providers, **When**
   the user views all providers, **Then** the two identities remain distinguishable
   by provider and their usage is not merged.
4. **Given** the user selects one model, **When** the history view updates,
   **Then** the selected model's usage is emphasized without hiding the selected
   time range or attribution coverage.
5. **Given** the selected range contains session activity from multiple providers,
   **When** the user chooses one provider, **Then** every summary, model row,
   history point, and session detail is limited to that provider.
6. **Given** one provider has token-bearing session activity but every model
   identifier is missing, **When** the user selects that provider, **Then** Models
   shows 0% attributed-token coverage and explains that provider model identity
   was not available.

---

### User Story 2 - Understand model history and coverage (Priority: P2)

The user can review model usage over `1H`, `24H`, `7D`, or `30D`, including
recoverable history from sessions that predate the feature. The view states how
much relevant activity has a known model and how much remains unattributed.

**Why this priority**: Historical trends are only trustworthy when users can see
the boundary between attributable and missing data. Silent omission would make
model comparisons look more complete than they are.

**Independent Test**: Start with retained transcripts containing a mix of known
and missing model identifiers, complete the history reprocessing, and verify that
the Models tab shows recovered models plus an attribution-coverage value that
accounts for the missing records.

**Acceptance Scenarios**:

1. **Given** retained transcripts with recoverable model identifiers, **When**
   Quill completes the first post-upgrade history pass, **Then** those identifiers
   and their recoverable usage appear without user action.
2. **Given** historical records whose model identifier is absent or unreliable,
   **When** coverage is calculated, **Then** those records contribute to the
   unattributed portion and do not create a model named `unknown`.
3. **Given** the user changes the time range, **When** Models refreshes, **Then**
   summaries, history, model rows, and detail all use the same selected period.
4. **Given** the same transcript history is processed again, **When** Models
   refreshes, **Then** totals and switch counts remain unchanged.
5. **Given** history reprocessing is pending, running, or partially complete,
   **When** the user opens Models, **Then** the current state and incomplete
   coverage are visible while all recovered results remain usable.
6. **Given** one or more retained transcripts cannot be read, **When** the history
   pass ends, **Then** Models reports a partial or failed result, identifies the
   amount of history not processed, and offers a retry without discarding valid
   results.
7. **Given** active filters contain no session activity but session or model data
   exists outside those filters, **When** Models renders, **Then** the empty state
   identifies the filter mismatch rather than claiming that no sessions or model
   evidence exist.

---

### User Story 3 - Investigate multi-model sessions (Priority: P3)

The user can identify sessions that used more than one model, inspect which model
was primary, and see model changes in order. Parent sessions and subagent chains
remain separate so concurrent or interleaved work does not create false switches.

**Why this priority**: Coding-agent sessions can route work through different
models. A single session-level label would hide that behavior and misattribute
subagent activity.

**Independent Test**: Process one parent session that changes models and one
subagent using a different model; verify the parent shows its real within-chain
switch while parent/subagent interleaving does not add a switch.

**Acceptance Scenarios**:

1. **Given** one ordered session chain uses model A and later model B, **When**
   the user opens its detail, **Then** both models and one within-chain switch are
   shown in chronological order.
2. **Given** a parent chain uses model A while a subagent chain uses model B,
   **When** their activity interleaves, **Then** each chain retains its model
   history and no parent-to-subagent switch is counted.
3. **Given** a session uses several models, **When** a primary model is displayed,
   **Then** the model with the most attributed tokens is primary, with turn count
   followed by provider and raw identifier used as deterministic tie-breakers.
4. **Given** a selected model has more than 20 matching sessions, **When** the user
   opens its detail, **Then** the 20 most recently active sessions appear first
   and the user can reveal older sessions until every in-scope match is reachable.
5. **Given** the user opens a multi-model session, **When** its detail renders,
   **Then** the primary model and every observed model change are shown in
   chronological order within each parent or subagent chain.

### Edge Cases

- A session or turn has token usage but no model identifier. Usage remains part
  of attribution coverage but is not assigned to a guessed model.
- A record has a model identifier but no attributable token values. It can
  contribute to observed turns, sessions, first/last seen, and switching, but
  not to model token totals.
- A provider reports cumulative token counters more than once. Reprocessing and
  live refreshes must not count the same consumed tokens multiple times.
- A model identifier contains mixed case, punctuation, a dated version, or an
  alias. Punctuation is rendered safely, and the accepted identifier is preserved
  rather than normalized or mapped to another value.
- A model identifier is empty after surrounding whitespace is removed, exceeds
  256 Unicode scalar values, contains a control character, or is not text. It is
  not accepted as model identity, and the rest of the session is still processed.
- Two providers report the same raw identifier. Their model identities and totals
  remain provider-qualified.
- A model changes several times within one parent or subagent chain. Every real
  ordered transition counts once; repeated consecutive observations of the same
  model do not.
- A chain contains model A, then an observation with missing or unreliable model
  identity, then model B. The gap breaks adjacency, so no A-to-B switch is
  inferred.
- Parent and subagent events interleave. Switches are calculated independently
  within each chain.
- A historical transcript contains model identity but incomplete token details.
  Quill reports only the dimensions supported by that evidence and keeps the
  coverage limitation visible.
- Only older aggregate history remains and the original model-bearing session
  records have been deleted. Quill does not infer a model from nearby activity.
- History reprocessing is interrupted. It can resume without duplicate model
  observations or changed totals.
- A retained transcript is appended, rewritten, or removed after it was processed.
  The next history pass adds new evidence, replaces changed evidence, and removes
  stale evidence so Models continues to match the retained source.
- A session is deleted or expires under existing retention behavior. Its model
  observations and aggregates no longer appear.
- A provider supplies session activity but no reliable model identity. Models
  explains that attribution is unavailable rather than treating the provider as
  a model named `unknown`.
- Model-attributed transcript data exists while live usage snapshots do not. The
  Models tab remains accessible and renders transcript-derived analytics.
- No session-model evidence exists at all. Models renders an actionable empty
  state and does not hide the tab.
- Cache-token dimensions are missing for a model. Cache-read share is displayed as
  unavailable, not as 0%.
- A large number of distinct identifiers exists because users select aliases,
  snapshots, or newly released models. The complete set remains discoverable
  without assigning arbitrary model colors.

## Requirements *(mandatory)*

### Definitions

- **Supported session record**: A locally retained record from a session source
  Quill already processes for session analytics. Support describes the source
  record shape, never a list of allowed model identifiers.
- **Reliable model evidence**: An explicit field in the source session record that
  identifies the model that produced that record. A configured default, nearby
  model observation, quota label, or inferred family name is not reliable model
  evidence.
- **Accepted raw model identifier**: A text value that contains 1–256 Unicode
  scalar values after surrounding whitespace is removed and contains no control
  characters. Punctuation, mixed case, dates, and provider-specific separators
  are allowed. The value is stored and displayed exactly after that sole
  whitespace-removal step.
- **Identifier order**: Case-sensitive, locale-independent lexicographic order by
  Unicode scalar value, first for provider and then for raw model identifier.
- **Observation token amount**: The sum of the input, output, cache-creation, and
  cache-read token dimensions reliably reported for one session observation.
  Repeated cumulative reports of the same consumption count once. An observation
  with no token dimensions is tokenless.
- **Attributed-token coverage**: `100 × attributed token amount ÷ total token
  amount` across all token-bearing session observations in the active range and
  provider scope. The attributed and unattributed amounts use the same population
  and sum to the denominator. When that denominator is zero, coverage is shown as
  unavailable rather than 0%. Tokenless observations remain eligible for turn,
  session, first/last-seen, and switch history but not token coverage.

### Functional Requirements

- **FR-001**: System MUST capture the accepted raw model identifier from every
  supported session record that contains reliable model evidence.
- **FR-002**: System MUST treat observed model identifiers as dynamic data and
  MUST NOT require a hard-coded allowlist, model enumeration, alias registry,
  version registry, or application update before accepting a new identifier.
- **FR-003**: System MUST preserve each accepted raw model identifier according to
  the accepted-identifier definition for historical identity and display, and
  MUST render allowed punctuation safely.
- **FR-004**: System MUST qualify model identity by provider so equal raw
  identifiers from different providers cannot collide.
- **FR-005**: Each model usage observation MUST retain its provider, session,
  ordered parent-or-subagent chain, observation time, raw model identifier, and
  any token dimensions reliably present in the source.
- **FR-006**: Missing model identity MUST be represented as unattributed coverage,
  not as a fabricated model identifier and not by assigning the activity to a
  nearby observation.
- **FR-007**: System MUST reprocess all retained local session transcripts after
  upgrade and recover every model observation supported by those transcripts.
- **FR-008**: Historical reprocessing MUST be resumable and safe to repeat:
  repeating it over unchanged history MUST NOT change model totals, turn counts,
  session counts, or switch counts.
- **FR-009**: Malformed records, unsupported record shapes, missing timestamps,
  absent or independently invalid token dimensions, or unaccepted model
  identifiers MUST be isolated to the affected record or dimension and MUST NOT
  prevent other valid records in the same source from being processed.
- **FR-010**: Model attribution MUST NOT change or double-count existing session
  and token totals; it adds an attribution dimension to reliable evidence.
- **FR-011**: A session MUST be allowed to reference an unbounded set of observed
  models, including model changes during a session. System MUST NOT impose an
  application-defined model-count cap or persist one authoritative session-model
  field that replaces the observation history.
- **FR-012**: Model-switch counts MUST be calculated only between consecutive
  model observations in the same ordered parent or subagent chain.
- **FR-012a**: A missing or unreliable model observation MUST break switch
  adjacency; System MUST NOT infer a switch between the known models on opposite
  sides of that gap.
- **FR-013**: Consecutive observations of the same provider-qualified model MUST
  NOT count as a switch.
- **FR-014**: Parent and subagent chains MUST retain independent model histories;
  interleaving between chains MUST NOT create switches.
- **FR-015**: When a primary model is needed for a multi-model session, System
  MUST select the model with the most attributed tokens, then the most observed
  turns, then the earliest provider followed by raw model identifier under the
  identifier-order definition.
- **FR-016**: Existing session deletion and retention behavior MUST remove the
  corresponding model analytics so deleted session activity does not remain in
  summaries, provider inventory, or history. Model analytics MUST NOT introduce
  an independent time-to-live; retained source/session lifecycle remains
  authoritative.
- **FR-017**: Analytics MUST include an always-visible **Models** tab independent
  of whether live provider-usage snapshots exist.
- **FR-018**: Models MUST support `1H`, `24H`, `7D`, and `30D` ranges and apply
  the active range consistently to every summary, history, row, and detail view.
- **FR-019**: Users MUST be able to filter Models by all providers or by each
  session provider represented in the selected range, including a provider whose
  represented activity is entirely unattributed. Sources suppressed by existing
  deletion or retention actions MUST NOT contribute represented activity.
- **FR-020**: Models MUST display compact summary values for attributed-token
  coverage, distinct provider-qualified models, and multi-model sessions.
- **FR-021**: Models MUST display model-attributed token history over the selected
  range and allow one model to be selected for visual focus.
- **FR-022**: Models MUST display a sortable row for every observed model in scope
  with raw identifier, provider, attributed tokens, observed turns, sessions,
  cache-read share, first seen, and last seen. Every displayed column MUST be a
  sort option. The default order MUST be attributed tokens descending, followed
  by provider and raw identifier under the identifier-order definition for stable
  ties.
- **FR-022a**: Cache-read share MUST equal cache-read tokens divided by the sum of
  input, cache-creation, and cache-read tokens. It MUST display as unavailable
  when the required source dimensions are absent or their denominator is zero.
- **FR-023**: Selecting a model MUST initially expose its 20 most recently active
  sessions and allow the user to reveal older results until every in-scope match
  is reachable. Each session MUST show provider, identity, attributed tokens,
  last activity, primary model, and whether it contains within-chain switches.
- **FR-023a**: Opening a session from model detail MUST show every observed model
  change chronologically within each parent or subagent chain, while keeping the
  chains visually distinct.
- **FR-024**: Models MUST show attribution coverage alongside model totals so
  unattributed activity cannot be mistaken for zero usage. The attributed and
  unattributed portions MUST use the same population of token-bearing session
  observations and together account for that complete population.
- **FR-025**: Models MUST distinguish empty states for no session activity, session
  activity without reliable model identity, and a selected range with no matching
  model observations.
- **FR-025a**: If the active scope contains session activity but lacks reliable
  model evidence, Models MUST show the no-model-evidence state. It MUST show the
  filter-empty state only when the active scope contains no session activity but
  matching activity exists outside the filters, and MUST show the global
  no-session state only when no retained session activity exists. These final
  empty claims MUST appear only after every root and discovered source completed
  without failure; otherwise Models MUST label the scope provisional/incomplete.
- **FR-026**: Newly processed model observations MUST become visible through the
  normal analytics refresh behavior in summaries, history, model rows, selected
  model session pages, and expanded chain history without requiring the window or
  application to restart.
- **FR-027**: Raw model identifiers MUST remain legible and copyable; provider
  identity MUST use Quill's established provider vocabulary while model identity
  itself remains visually neutral rather than receiving status colors.
- **FR-028**: Model aliases and concrete version identifiers MUST remain separate
  observed identities unless the provider itself reports them as the same raw
  identifier; Quill MUST NOT rewrite historical identity.
- **FR-029**: Any supported source record that satisfies the reliable-evidence and
  accepted-identifier definitions MUST populate Models without identifier-specific
  product rules.
- **FR-030**: Models MUST disclose history-reprocessing status as pending, running,
  complete, partial, or failed. Partial results MUST remain visible and be labeled
  incomplete. Status MUST distinguish a completely enumerated inventory with
  unreadable sources from provider-root discovery that did not complete.
- **FR-031**: A partial or failed history pass MUST report how much retained
  history could not be processed, including incomplete provider roots and failed
  sources, and MUST allow retry without discarding valid recovered results.
- **FR-032**: Reprocessing appended, rewritten, or removed transcripts MUST add,
  replace, or remove corresponding model observations so Models matches retained
  source evidence; unchanged source evidence MUST remain a no-op.
- **FR-033**: Models MUST provide distinct loading and error states. A loading,
  partial, or failed history state MUST be able to coexist with already available
  model results rather than replacing them. Each independently failed aggregate,
  history, session-page, or chain-history request MUST retain unaffected results
  and offer a retry for that failed region. Results from an older range, provider,
  model, or session scope MUST NOT be presented as belonging to a newer scope.

### Scope Boundaries

**In scope**:

- Prospective model capture from normal local coding-agent sessions.
- Full recoverable backfill from retained local transcripts.
- Provider-qualified raw model identity and honest unattributed coverage.
- Token, turn, session, cache-read-share, first/last-seen, and model-switch
  analytics.
- A range-aware, provider-filterable Models tab with model and session detail.

**Out of scope**:

- Per-model prices, spend estimates, or cost comparisons.
- Per-model active runtime or latency attribution.
- Model capability catalogs, context-window metadata, lifecycle/deprecation data,
  recommendations, or "best model" judgments.
- Friendly-name mappings, alias resolution, identifier normalization, or merging
  model versions under a family name.
- Selecting, configuring, or changing the model used by a provider.
- Provider quota/entitlement rows; Models reports observed session usage rather
  than which models an account may use.
- Cloud synchronization or reconstruction from session data that is no longer
  available locally.

### Key Entities

- **Observed Model Identity**: A provider-qualified raw identifier exactly as it
  appeared in reliable session evidence after the accepted identifier's sole
  surrounding-whitespace removal. Identity is the pair of provider and raw model
  identifier; descriptive metadata is not required.
- **Model Usage Observation**: One reliable point in a session where a model was
  observed. It belongs to a provider, session, and ordered parent-or-subagent
  chain; carries a timestamp and raw model identity; and may carry input, output,
  cache-creation, and cache-read token values.
- **Attribution Coverage**: The percentage defined under Requirements. It uses one
  population for both numerator and denominator, while tokenless observations
  remain available to the non-token measures they support.
- **Model Usage Summary**: A range-scoped aggregate for one observed model,
  including token dimensions, turns, sessions, cache-read share, and first/last
  seen.
- **Model Session Summary**: A model-focused view of one session, including its
  attributed usage, primary model, last activity, parent/subagent chain context,
  and whether it contains within-chain model changes.
- **Model Switch**: A transition between two different provider-and-identifier
  pairs observed consecutively within the same ordered parent or subagent chain.
  Missing or unreliable model evidence breaks the sequence rather than allowing a
  switch to be inferred across the gap.

## Success Criteria *(mandatory)*

### Measurable Outcomes

Performance evidence uses this fixed protocol:

- The benchmark desktop profile provides four logical CPU cores to Quill, 8 GiB
  of system memory, and local solid-state storage. Evidence from more capable
  hardware qualifies only when CPU and memory are constrained to that profile;
  the exact CPU, storage, and operating system are recorded. All measurements use
  one release artifact built from one recorded commit.
- A clean history run starts from fresh model-analytics state with the same
  isolated Quill data directory and the same fixture-corpus hash. The full corpus
  is read once immediately before each launch to establish the same warm
  filesystem-cache policy. Timing starts when history status first becomes
  `running` and stops when its terminal status is committed and visible.
- A Models opening starts when the user activates Models and stops when the
  complete summary and model rows render. One unmeasured warm-up precedes 100
  measured openings in the same process with unchanged data.
- Live-visibility timing starts after a complete retained record is flushed and
  its transcript notification is accepted, and stops when its provider-qualified
  model row renders.
- Other Analytics views remain usable during backfill when tab and range changes
  render within two seconds without input lock or application failure.

- **SC-001**: After processing, 100% of accepted raw model identifiers present as
  reliable model evidence in supported session records are represented by the
  same accepted identifier and provider in Models.
- **SC-002**: A previously unseen accepted raw model identifier appears in Models
  within five seconds of its observation becoming available to Analytics, with no
  Quill update or configuration change.
- **SC-003**: Users can identify the highest-token model, its provider, and its
  share of attributed usage for any supported time range in under 10 seconds.
- **SC-004**: Across at least three clean history-reprocessing runs under the fixed
  benchmark protocol, histories containing 10,000
  readable retained sessions reach the complete state within five minutes without
  users losing access to other Analytics views. Histories containing unreadable
  transcripts reach a labeled partial or failed state within the same period.
- **SC-005**: 100% of token-bearing session observations in the selected range are
  accounted for as either model-attributed or unattributed; no observation in the
  coverage population is silently omitted.
- **SC-006**: Reprocessing unchanged history produces zero change in model token
  totals, turn counts, session counts, and switch counts.
- **SC-007**: Across 100 measured Models-tab openings under the fixed benchmark
  protocol with a history containing 100,000 model observations, at least 95 show
  complete summary and row data within two seconds.
- **SC-008**: Adding or backfilling model attribution causes zero change to
  existing provider, session, and total-token values for unchanged source data.
- **SC-009**: In a consistent usability walkthrough with at least 10 representative
  Quill users covering a parent model change and a differently modeled subagent,
  at least 90% identify the real within-chain switch without reporting
  parent/subagent interleaving as a switch.
- **SC-010**: Every empty or partial-data state states whether no sessions exist,
  no model identity was available, or the active filters selected no matching
  observations.

## Dependencies

- Quill must retain access to the local session transcripts selected by existing
  retention settings; deleted or unavailable transcripts cannot be backfilled.
- Session sources must provide explicit model identity and token evidence for the
  dimensions they expect Quill to attribute. Quill cannot recover facts the source
  never recorded.
- The existing local session-processing lifecycle must continue to identify
  provider, session, parent/subagent chain, ordering, and deletion so model history
  can follow the same ownership boundaries.
- Models relies on the existing Analytics time-range, provider-selection, and live
  refresh vocabulary so its controls remain consistent with other Analytics tabs.
- Existing provider, session, and token totals remain the source of truth for their
  current views; model attribution must reconcile with them without rewriting them.

## Assumptions

- Quill remains local-first: model observations and analytics use locally retained
  session evidence and are not sent to a cloud service.
- Provider session formats may expose model identity and token dimensions with
  different completeness. Only source-supported facts are presented as exact.
- Attribution coverage applies to token-bearing session observations that can be
  evaluated consistently; it does not claim to classify unrelated live-only quota
  or token snapshots that carry no session-model evidence.
- Existing session retention and deletion choices govern model history retention.
- Retained transcripts are the sole source for historical backfill. Aggregate-only
  history is not reverse-attributed to models.
- Model identifiers are opaque provider values. Their spelling does not reliably
  encode family, capability, price, lifecycle, or equivalence.
- The Models tab follows the existing Analytics range and provider vocabulary but
  does not depend on live rate-limit polling.
- Session sources that currently lack model identity contribute no model rows until
  they expose reliable evidence; this is a source-data limitation, not a permanent
  exclusion.
- Cost and per-model runtime can be specified later if trustworthy pricing and
  interval attribution become available without weakening historical accuracy.
