# Retention Corpus Study Evidence Report

This is the only commit-eligible shape for a privacy-reviewed aggregate report; it is rendered from a schema-valid private manifest and excludes source evidence.

## Scope and approvals

State corpus independence and approval status without paths, labels, custodians,
operators, reviewers, identifiers, or dates that fingerprint the source. Corpus
approval does not authorize Git, Beads, or external transfer.

## Baselines

Keep the 2026-07-24 real source inventory, later `VACUUM INTO` index-drop copy,
and frozen synthetic timing fixture separate. Do not blend denominators.

## Controlled matrix

Publish three controlled-warm observations per mode, median, and maximum. Label
cold observations best-effort diagnostic data. Missing, suppressed, failed, or
incomparable members yield `insufficient evidence`; never drop outliers or
replace a warm observation with cold or extra data.

| Mode | Warm 1 | Warm 2 | Warm 3 | Median | Maximum | Classification |
| --- | ---: | ---: | ---: | ---: | ---: | --- |
| Archive off | | | | | | insufficient evidence |
| Archive on | | | | | | insufficient evidence |

## Offline dbstat footprint gate

This section is eligible only after timing-sensitive replay and uses separate
post-replay scratch snapshots, never a replay copy. Publish exactly three
controlled warm paired repetitions; the cancellation probe is separate and is
not a fourth repetition. Whole-object totals and before/after deltas may remain
exact because they contain no category or calendar-month cells.

| Repetition | Before walk | After walk | Max RSS delta | Reconciliation |
| --- | ---: | ---: | ---: | --- |
| 1 | | | | |
| 2 | | | | |
| 3 | | | | |

Record main-database, object-btree, freelist, unattributed/non-btree, WAL, and
SHM bytes separately. The zero-tolerance equation is `main + WAL + SHM =
object b-tree + freelist + unattributed + WAL + SHM`. State cancellation
latency, the fixed offline placement, retain or reject disposition, and reasons.
Do not create or imply product work unless retained accounting explains at least
90% of measured database-byte delta and an operator-recorded actionable object
occupies at least 5% of the before file.

## Privacy and rounding

Suppress category and UTC-calendar-month cells below 10 as `suppressed (<10)`.
Whole-table totals remain exact. Round milliseconds to whole values, bytes to
whole bytes, rates to two decimals, and percentages to one decimal. Remove
paths, hostnames, projects, sessions, agents, payloads, archive rows, raw
errors, and run/source identifiers.

## Decision registry

The 90-day recommendation remains `insufficient evidence` until a separately
approved product threshold exists. A ceiling is `confirm` only when all three
controlled observations meet it; `revise` needs at least two over it by more
than 10%; every other outcome is `insufficient evidence`. Non-conforming
timestamps stay separate counts and never become retention-eligible rows.
