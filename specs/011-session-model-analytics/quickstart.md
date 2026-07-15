# Quickstart: Session Model Analytics

Use isolated demo-mode directories to validate model ingestion, history recovery,
and UI behavior without touching normal Quill data.

## Prerequisites

- Existing project toolchain and dependencies.
- Sanitized Claude and Codex transcript fixtures placed in isolated directories.
- No new automated test code is part of this plan; adding tests requires separate
  explicit user authorization.

## Start with isolated data

```bash
npm install
RUN_ROOT="$(mktemp -d /tmp/quill-models.XXXXXX)"
mkdir -p \
  "$RUN_ROOT/home" \
  "$RUN_ROOT/xdg/config" \
  "$RUN_ROOT/xdg/data" \
  "$RUN_ROOT/xdg/cache" \
  "$RUN_ROOT/xdg/runtime" \
  "$RUN_ROOT/data" \
  "$RUN_ROOT/rules" \
  "$RUN_ROOT/claude" \
  "$RUN_ROOT/codex"
chmod 0700 "$RUN_ROOT/xdg/runtime"

env -u AT_SPI_BUS_ADDRESS \
HOME="$RUN_ROOT/home" \
XDG_CONFIG_HOME="$RUN_ROOT/xdg/config" \
XDG_DATA_HOME="$RUN_ROOT/xdg/data" \
XDG_CACHE_HOME="$RUN_ROOT/xdg/cache" \
XDG_RUNTIME_DIR="$RUN_ROOT/xdg/runtime" \
QUILL_DEMO_MODE=1 \
QUILL_DATA_DIR="$RUN_ROOT/data" \
QUILL_RULES_DIR="$RUN_ROOT/rules" \
QUILL_CLAUDE_PROJECTS_DIR="$RUN_ROOT/claude" \
QUILL_CODEX_SESSIONS_DIR="$RUN_ROOT/codex" \
npm run tauri -- dev
```

Place only sanitized fixtures under `$RUN_ROOT/claude` and `$RUN_ROOT/codex`.
Keep the same `RUN_ROOT` and environment for every restart in one validation
run. After stopping every process from the run, remove only its guarded root:

```bash
case "$RUN_ROOT" in
  /tmp/quill-models.*) rm -rf -- "$RUN_ROOT" ;;
  *) echo "Refusing to remove unexpected RUN_ROOT: $RUN_ROOT" >&2 ;;
esac
```

## Fixture matrix

Prepare retained transcript sources with these evidence patterns. Model strings
are examples of opaque input data, not supported-model constants.

| Fixture | Required evidence | Expected outcome |
|---|---|---|
| Claude attributed | Assistant records with explicit model and usage fields | Raw ID, turns, token dimensions, and session appear |
| Unseen identifier | Accepted string not previously stored, such as `future/model.snapshot-2099` | Appears without code/config change |
| Missing identity | Token-bearing assistant record without model | Tokens count as unattributed; no `unknown` row |
| Invalid identity | Empty, 257-Unicode-scalar, control-character, and non-text model values | Identity rejected; other valid record data survives |
| Malformed record isolation | Claude and Codex malformed JSON, unsupported shape, and missing timestamp placed between valid records | Only affected records are skipped; later valid records appear in both adapters |
| Invalid token sibling | One absent/invalid token dimension beside valid dimensions | Valid sibling dimensions survive independently |
| Claude model change | Same parent chain: model A, then model B | One within-chain switch |
| Gap between models | Model A, null-model turn, model B | No inferred A-to-B switch |
| Claude subagent | Parent model A with interleaved subagent model B | Separate chains; no parent/subagent switch |
| Codex model context | `turn_context` with an explicit model | Model turn/session appears with no invented tokens |
| Codex token count | First, monotonic, missing-dimension, identical, and per-dimension decrease cumulative events without model foreign key | Exact per-dimension deltas count once, missing baselines persist, resets count from zero, and tokens remain unattributed |
| All-unattributed provider | Provider activity with tokens but no accepted model | Provider filter remains available with 0% coverage |
| Unbounded session models | At least 1,001 accepted distinct IDs in one session | All identities remain in complete table/detail with no session-model cap |
| Bracketing activity | One session observation before and one after a selected range, with none inside it | Session/provider do not enter active scope; filter-empty state appears |

Use at least two providers with the same raw model string to confirm provider
qualification keeps their rows separate.

## First-run backfill

1. Start Quill with the fixture directories and open Analytics → Models.
2. Confirm status moves from pending to running and then complete, partial, or
   failed without hiding recovered rows or blocking another Analytics tab.
3. Confirm summary coverage accounts for all token-bearing observations as
   attributed plus unattributed.
4. Verify range and provider changes update summary, chart, table, and detail
   together.
5. Restart with unchanged fixtures. Totals, turns, sessions, and switches must
   remain identical; unchanged sources should be skipped.

For one intentionally unreadable source, valid rows from other sources remain
visible, status becomes partial or failed as appropriate, failed/remaining counts
show `failedSources > 0` and `remainingSources === 0` after terminal processing,
and Retry does not discard valid results. Nonzero remaining count is reserved for
interrupted or not-yet-attempted work.

Also make one provider root unreadable. Confirm failed-root and failed-source
counts remain distinct and `inventoryComplete` is false for root failure but can
be true for a partial run caused only by individually unreadable sources.

## Prospective reconciliation

With the app running:

1. Append and flush an assistant or turn-context record with a new accepted raw
   model ID, then send its transcript notification. Start timing when notification
   acceptance returns and confirm the row renders within five seconds even while
   additional notifications continue; later events must not extend the first
   event's fixed one-second refresh deadline.
2. Rapidly notify a parent transcript and subagent transcript that share one
   analytics session inside the same search-coalescing window. Confirm both source
   paths reconcile while Session Search retains its existing session-keyed
   coalescing behavior.
3. Rewrite one source so its model evidence changes. Confirm only that source's
   model rows change and sibling parent/subagent sources remain intact.
4. Remove one source, then run a complete retry. Confirm its stale observations
   disappear.
5. Trigger the existing session deletion workflow. Confirm model analytics vanish,
   then retry unchanged retained history and verify they do not resurrect.
6. Change the suppressed transcript content. Confirm current evidence can return
   only after its replacement commits; failed parsing must leave it suppressed.
7. With more than one selected-model session page loaded and one row expanded,
   append matching evidence. Confirm refresh replays the loaded page count,
   refetches expanded history, invalidates collapsed history, and preserves old
   results with page/row Retry if refresh fails.
8. Force page replay to fail while one expanded history can still refresh, then
   reverse the failures. Confirm each request proceeds independently and only its
   own Retry/error region is affected.

## Models UI walkthrough

- Models remains visible when live usage snapshots are empty or all providers are
  disabled.
- Summary rail shows coverage, distinct provider-qualified models, and multi-model
  sessions.
- History keeps aggregate attributed and unattributed series visible while one
  selected model is emphasized.
- Every table column sorts, with default token-descending/provider/model ordering.
- Raw IDs remain complete, copyable, monospace, and visually neutral.
- Selecting a model loads 20 recent sessions; Load more reaches all results.
- Expanding a session loads chain history lazily, shows primary model, and keeps
  parent/subagent chains distinct.
- Empty states distinguish no global sessions, filter mismatch, and session
  activity without reliable model identity.
- Aggregate, history, backfill, page, and row errors coexist independently with
  last successful data and expose scope-specific Retry actions.
- Narrow panes scroll the semantic table horizontally without dropping columns.
- Keyboard tab navigation, labeled range/provider button groups with
  `aria-pressed`, `aria-sort`, native model inspection, chart data-table access,
  disclosure `aria-expanded`/`aria-controls`, status announcements, specific
  Load more/Retry names, visible focus, and reduced motion remain usable.

## US1 implementation evidence — 2026-07-14

This run kept sanitized transcripts and the analytics database under
`/tmp/quill-t027-*`. It exercised the browser-demo UI and the real Claude/Codex
notification adapters; it did not add automated test code or read normal
retained transcripts. One initial launch omitted the rules-directory override,
as noted under the isolation incident. All follow-up native UI, range, and timing
checks isolated `HOME`, XDG directories, rules, data, and provider roots.

### Browser-demo scope, coverage, and identity

The UI run used `npm run dev -- --host 127.0.0.1`, an isolated Chrome profile,
X11 keyboard input, and Chrome DevTools Protocol DOM inspection. Screenshots and
CDP output were retained only under `/tmp` for this run.

| Check | Observed result |
|---|---|
| `1h`, all providers | 5,690 attributed + 7,600 unattributed = 13,290 tokens; 42.81414597441685% coverage; 3 distinct provider-qualified rows; 12 history buckets |
| `24h`, all providers | 5,690 attributed + 18,800 unattributed = 24,490 tokens; 23.23397305022458% coverage; 4 distinct rows; 24 history buckets |
| Accepted fixture identities | `1h`: 3/3 represented; `24h`: 4/4 represented. `future/model.snapshot-2099` remained exact and appeared without a catalog change. |
| Equal raw IDs | `claude/shared/model.snapshot` and `codex/shared/model.snapshot` rendered as separate rows. |
| Aggregate/history consistency | History sums were exactly `[5,690, 7,600]` for `1h` and `[5,690, 18,800]` for `24h`; parallel `1h` and `24h` calls retained their own bucket counts and scopes. |
| Bracketed interval | MiniMax observations at 70 minutes before and 5 minutes after the fixture snapshot produced `scopedSessionCount = 0`, `scopedEvidenceCount = 0`, zero rows, and unavailable coverage for `1h`; they did not enter scope from source-bound overlap. In `24h`, the earlier observation produced one scoped session, 5,200 unattributed tokens, 0% coverage, and no fabricated model row. |

The hidden history table exposed caption `Model token history by time bucket`, 12
rows for `1h`, and all five required columns: bucket start, bucket end,
attributed tokens, unattributed tokens, and selected-model tokens.

Every one of the 13 visible model columns was activated twice through its native
sort button. `aria-sort` changed between `ascending` and `descending` for
Provider, Model ID, attributed tokens/share, all four token dimensions, turns,
sessions, cache-read share, first seen, and last seen. Row order changed by the
chosen value with provider/raw-ID stable ties; unavailable cache share stayed
last. The default was attributed-token descending.

Keyboard input exercised `Home` (Now), `ArrowRight` (Trends then Charts), `End`
(Context), and `ArrowLeft` (Models). Each command moved visible focus and selected
the matching panel. Models remained the fourth tab in the five-tab set.

### Independent requests and Retry

A browser-only scoped failure wrapper rejected one command at a time while
leaving the argument-aware fixtures unchanged:

- On `24h`, `get_model_history` failed while aggregate data remained visible.
  `Retry history` issued only `get_model_history`, cleared its error, and restored
  24 semantic-table buckets.
- On `7d`, `get_model_analytics` failed while successful 28-bucket history
  remained visible. `Retry summary and table` issued only
  `get_model_analytics`, cleared its error, and restored five model rows.

These results verify command-, scope-, and Retry-state independence in the
frontend. They do not claim a concurrent SQLite mutation test; each real backend
command independently opens and commits its own deferred read snapshot.

### Native Claude and Codex ingest

The initial real adapter run used an isolated D-Bus session with provider flags
disabled and these overrides:

```bash
QUILL_DEMO_MODE=1 \
QUILL_DATA_DIR=/tmp/quill-t027-data \
QUILL_CLAUDE_PROJECTS_DIR=/tmp/quill-t027-claude \
QUILL_CODEX_SESSIONS_DIR=/tmp/quill-t027-codex \
QUILL_PORT=19927 \
dbus-run-session -- src-tauri/target/debug/quill
```

This exact successful command omitted `QUILL_RULES_DIR`. Startup warned that it
was using the production default rules directory; the watcher then reported
`Rule watcher: initial reconciliation — no changes`. No rule mutation was
observed. The follow-up native UI and timing run used the same sanitized data and
provider roots with complete process-level isolation:

```bash
HOME=/tmp/quill-t027-native/home \
XDG_CONFIG_HOME=/tmp/quill-t027-native/xdg/config \
XDG_DATA_HOME=/tmp/quill-t027-native/xdg/data \
XDG_CACHE_HOME=/tmp/quill-t027-native/xdg/cache \
XDG_RUNTIME_DIR=/tmp/quill-t027-native/xdg/runtime \
DISPLAY=:0 \
QUILL_DEMO_MODE=1 \
QUILL_DATA_DIR=/tmp/quill-t027-data \
QUILL_RULES_DIR=/tmp/quill-t027-native/rules \
QUILL_CLAUDE_PROJECTS_DIR=/tmp/quill-t027-claude \
QUILL_CODEX_SESSIONS_DIR=/tmp/quill-t027-codex \
QUILL_PORT=19927 \
WEBKIT_INSPECTOR_HTTP_SERVER=127.0.0.1:9234 \
dbus-run-session -- src-tauri/target/debug/quill
```

The runtime directory had mode `0700`. Claude, Codex, and MiniMax remained
disabled for the whole follow-up.

One authenticated Python `urllib` POST per fixture returned `202 queued`:
Claude in 0.026 seconds and Codex in 0.028 seconds. The requests were sequential.
A read-only Python stdlib `sqlite3` poll started only after the second response
and observed both source rows as `ok` plus all committed observations in 0.002
seconds. Therefore the observed database bounds were at most 0.002 seconds from
the final accepted response and 0.056 seconds from the first request start
(`0.026 + 0.028 + 0.002`). These are commit-visibility bounds, not rendered-UI
timings.

| Provider/model identity | Observations | Input | Output | Cache creation | Cache read |
|---|---:|---:|---:|---:|---:|
| Claude / unattributed | 4 | 109 | 72 | 0 | 0 |
| Claude / `Mixed.Case/v9` | 1 | 40 | 0 | 0 | 10 |
| Claude / exact 256-scalar `a…a` | 1 | 60 | 20 | 0 | 0 |
| Claude / `future/model.snapshot-2099` | 1 | 80 | 40 | 5 | 25 |
| Claude / `shared/model.snapshot` | 1 | 100 | 50 | 20 | 30 |
| Codex / unattributed | 3 | 1,000 | 200 | 150 | 500 |
| Codex / `codex/new.model-Ω` | 1 | 0 | 0 | 0 | 0 |
| Codex / `shared/model.snapshot` | 1 | 0 | 0 | 0 | 0 |

The accepted identity set was represented 6/6 as provider-qualified rows. The
exact 256-scalar identifier was present, and the 257-scalar candidate was absent
from both providers. Sole surrounding whitespace was removed from
`Mixed.Case/v9` without case or punctuation rewriting. Equal Claude/Codex
`shared/model.snapshot` identities remained separate. Malformed JSON, non-object
records, unsupported shapes, and missing timestamps placed between valid records
did not prevent later accepted records from committing. Negative/non-numeric
token siblings became null while the valid dimensions in the same records
survived.

Across this native fixture, attributed tokens were 480 and unattributed tokens
were 2,031, accounting for all 2,511 token units and yielding
19.11589008363202% coverage. Tokenless Codex model turns remained represented
without invented tokens.

### Native Models reachability and complete representation

WebKit remote inspection opened the real Tauri window after the fully isolated
launch. Now showed `0 lines (1h)`, the live pane still said `No provider is
enabled`, and every provider flag remained disabled. Despite that zero-snapshot
state, Analytics → Models was present, had `aria-selected="true"`, and its panel
was visible.

Before adding timing-only live sources, both the real
`get_model_analytics({ range: "1h", provider: null })` IPC response and rendered
model table contained all 6/6 accepted provider-qualified identities from the
fixture table above. The native UI showed 480 of 2,511 tokens attributed (19.1%)
and six model rows. Inspector output retained the complete 256-`a` identifier,
confirmed its Unicode-scalar length was 256, and contained no 257-scalar row.
Both provider-qualified `shared/model.snapshot` rows and the unseen
`future/model.snapshot-2099` row were present. This was real aggregate/UI output,
not the browser mock.

### Native timestamp boundaries

A separate fully isolated native launch used a sanitized Claude source with one
session observation at `2026-07-15T00:12:24.609Z` and another at
`2026-07-15T01:27:24.611Z`. Its authenticated notification returned `202 queued`
in 0.0215705079 seconds. The real Tauri IPC aggregate generated at
`2026-07-15T01:25:43.589Z` queried the half-open range
`[2026-07-15T00:25:43.589Z, 2026-07-15T01:25:43.589Z)` and returned:

```text
globalSessionCount=1
scopedSessionCount=0
scopedEvidenceCount=0
representedProviders=[]
attributedTokens=0
unattributedTokens=0
totalTokens=0
attributedCoveragePercent=null
distinctModels=0
models=[]
```

The retained session existed globally, but neither bracketing observation entered
the selected interval. This is the real backend filter-empty aggregate rather
than the browser MiniMax fixture result.

### Native fixed-window visibility

The timing rerun used fresh `/tmp/quill-t027-fixed-window-2` storage and provider
roots. `HOME`, `XDG_CONFIG_HOME`, `XDG_DATA_HOME`, `XDG_CACHE_HOME`, the mode-0700
`XDG_RUNTIME_DIR`, `QUILL_DATA_DIR`, `QUILL_RULES_DIR`, and both provider roots
were isolated under that directory. Before the run, the configured Claude root
had zero files, the database had zero sources and observations, and the real
provider-disabled Tauri window already had Models selected with `0 models`.

Four sanitized files were staged outside the configured provider root. One
Python process moved source 1 into the root and waited for its `202 queued`
response, waited about 250 milliseconds, then repeated that move/notify sequence
for sources 2, 3, and 4. No later source path existed under the provider root
before its own step.

| Source | Accepted response (epoch ms) | Accepted offset | Committed `lastSuccessAtMs` | Commit offset |
|---|---:|---:|---:|---:|
| `fresh-event-1` | 1784079404895 | 0 ms | 1784079404896 | 0 ms |
| `fresh-event-2` | 1784079405147 | 252 ms | 1784079405153 | 257 ms |
| `fresh-event-3` | 1784079405399 | 504 ms | 1784079405402 | 506 ms |
| `fresh-event-4` | 1784079405651 | 756 ms | 1784079405659 | 763 ms |

Each source independently reached `processing_status = ok` with one observation.
Because each successful source transaction emits its update only after commit,
all four commit-backed events occurred before render. A WebKit mutation observer
found `live/fixed-window-fresh-first` in the real Models DOM at
`1784079406002`; the same render contained all four fresh provider-qualified
rows.

The first row rendered 1,107 milliseconds after the first accepted response and
1,106 milliseconds after its commit, while the fourth source had already
committed 343 milliseconds before render. If the fourth event had reset the
one-second timer, the earliest refresh would instead have been
`1784079406659`, 657 milliseconds later than the observed render. The continued
event burst therefore did not extend the first event's deadline, and the native
notification-to-render result remained well within five seconds. Database commit
and rendered-DOM boundaries are recorded separately.

### Isolation incident and remediation

An abandoned setup attempt temporarily changed the isolated database's provider
flags to mount Session Search. That caused startup repair to write user-level
Claude integration files before the process was stopped. Exact log evidence:

```text
[2026-07-15][00:47:47][quill_lib::claude_setup][INFO] Refreshed secret in existing local config.json
[2026-07-15][00:47:47][quill_lib::claude_setup][INFO] Registered quill MCP server in .claude.json
[2026-07-15][00:47:47][quill_lib::claude_setup][INFO] Registered Quill hooks in settings.json
[2026-07-15][00:47:47][quill_lib::claude_setup][INFO] Updated Quill MCP section in ~/.claude/CLAUDE.md
[2026-07-15][00:47:50][quill_lib::claude_setup][INFO] MCP server verification passed
[2026-07-15][00:47:54][quill_lib::integrations::manager][INFO] Integration startup repair passed for provider=Claude
```

Affected paths were `~/.config/quill/config.json`, `~/.claude.json`,
`~/.claude/settings.json`, and `~/.claude/CLAUDE.md`. No Codex repair completion
was logged. The process was stopped, isolated provider flags were restored to
disabled, and the successful ingest run used direct authenticated notifications
without enabling either provider.

Remediation completed without restarting production Quill; its existing process
remained alive. `~/.config/quill/config.json` was restored from its exact backup
and validated against the production secret, port, and mode. Production-compatible
managed blocks in `~/.claude/CLAUDE.md`, `~/.claude/settings.json`, and
`~/.claude.json` were retained. The audit removed four broken debug-only wrapper
entries, three debug-only MCP modules, and their Python bytecode cache, while
retaining the production-valid qbuild integration.

Root cause: `QUILL_DATA_DIR` isolates Quill's database but does not relocate
global provider-integration paths under the user's home and XDG directories.
Future full-app fixture runs must also isolate `HOME`, `XDG_CONFIG_HOME`,
`XDG_DATA_HOME`, and `QUILL_RULES_DIR`; database-only overrides are insufficient
when startup integration repair can run.

## US2 backfill and reconciliation evidence — 2026-07-14

This walkthrough used only generated transcripts and isolated runtime state. No
normal `~/.config/quill`, `~/.claude`, `~/.codex`, or retained user session path
was read or written. Before launch, a realpath assertion required every configured
path to begin with `/tmp/quill-t037-` and rejected each normal path and descendant.

The native fixture root was
`/tmp/quill-t037-native-20260714.iqBrBZ`. Its runtime directory had mode `0700`.
The launch that exposed both the real Tauri IPC surface and HTTP notification
adapter was:

```bash
env -u AT_SPI_BUS_ADDRESS \
HOME=/tmp/quill-t037-native-20260714.iqBrBZ/home \
XDG_CONFIG_HOME=/tmp/quill-t037-native-20260714.iqBrBZ/xdg/config \
XDG_DATA_HOME=/tmp/quill-t037-native-20260714.iqBrBZ/xdg/data \
XDG_CACHE_HOME=/tmp/quill-t037-native-20260714.iqBrBZ/xdg/cache \
XDG_RUNTIME_DIR=/tmp/quill-t037-native-20260714.iqBrBZ/xdg/runtime \
DISPLAY=:0 \
QUILL_DEMO_MODE=1 \
QUILL_DATA_DIR=/tmp/quill-t037-native-20260714.iqBrBZ/data \
QUILL_RULES_DIR=/tmp/quill-t037-native-20260714.iqBrBZ/rules \
QUILL_CLAUDE_PROJECTS_DIR=/tmp/quill-t037-native-20260714.iqBrBZ/claude \
QUILL_CODEX_SESSIONS_DIR=/tmp/quill-t037-native-20260714.iqBrBZ/codex \
QUILL_PORT=19937 \
WEBKIT_INSPECTOR_HTTP_SERVER=127.0.0.1:9237 \
RUST_LOG=info \
dbus-run-session -- npm run tauri -- dev
```

The first generated inventory contained 1,206 Claude files and one Codex file.
The first migration completed 1,207/1,207 sources with 1,208 observations. A
further 20,000 Claude files expanded the retry inventory to 21,207 sources. Real
IPC calls were issued through WebKit's isolated
`ws://127.0.0.1:9237/socket/1/1/WebPage` target. HTTP notifications used the
isolated `data/auth_secret` against port `19937`.

### Interruption, restart, and unchanged retry

The real `retry_model_history_backfill` command returned generation 1 pending at
epoch `1784087803899`. The worker entered running at `1784087803904`. A read-only
SQLite poll captured 647 processed, 185 skipped, and 20,375 remaining before the
process was interrupted. The durable row after shutdown had 996 processed, 220
skipped, 19,991 remaining, 996 observations written, and update timestamp
`1784087813077`; status remained running and `finished_at_ms` remained null.

Restarting with the exact same environment created generation 2 with trigger
`startup_resume`. The first captured resumed row had 867 processed, 1,309 skipped,
and 19,031 remaining. It committed complete at `1784087873953` with both roots
complete, inventory complete, 18,998 processed, 2,209 skipped, zero failed, zero
remaining, and 18,998 observations written. The resulting tables held 21,207
sources and 21,208 observations.

An immediate unchanged retry returned generation 3 pending at
`2026-07-15T03:58:18.001Z` and committed complete at `1784087924238`:

```text
totalSources=21207
processedSources=0
failedSources=0
skippedSources=21207
remainingSources=0
observationsWritten=0
```

Source and observation counts remained exactly 21,207 and 21,208. Removing the
20,000 expansion files plus the original 1,200 bulk files and retrying pruned
their rows under a complete Claude-root proof, leaving the seven targeted
sources plus their eight observations before the remaining scenarios.

### Failed source versus failed root

For the source-only run, `unreadable-session.jsonl` was changed and set to mode
`000` while both provider roots remained readable. Generation 5 committed partial
at `1784087979078` with roots 2/2 complete, `inventoryComplete=true`, seven total
sources, one failed source, six skipped, and zero remaining. Its last-good
`unreadable/model-v1` observation remained visible and source status became
`stale`.

For the root-only run, that file was restored to mode `0600` with valid changed
content and the isolated Codex root alone was set to mode `000`. Generation 6
committed partial at `1784088006749` with one completed root, one failed root,
`inventoryComplete=false`, six discovered sources, one processed, zero failed
sources, five skipped, and zero remaining. The persisted Codex source stayed
`ok` with its two last-good observations. The restored Claude source atomically
changed to `unreadable/model-v2` with 700 input and 70 output tokens. The Codex
root was then restored to mode `0700`.

These runs distinguish individually unreadable sources from incomplete root
enumeration: only the former can finish with a complete inventory.

### Append, rewrite, remove, and sibling isolation

Live source notifications exercised changed-source replacement independently of
the backfill:

| Mutation | Accepted | Committed result |
|---|---:|---|
| Append one valid record | `1784088044431`–`1784088044454` (`202 queued`) | `lastSuccessAtMs=1784088044463`; source held both `rewrite/model-v1` and `append/model-v2` |
| Rewrite parent source | `1784088069059`–`1784088069082` (`202 queued`) | `lastSuccessAtMs=1784088069089`; parent became `parent/model-v3` with 300 input/30 output |
| Remove retained source | generation 7 terminal `1784088105917` | Complete inventory pruning left zero source and observation rows for `remove-session` |

Before and after the parent rewrite, its sibling subagent retained source hash
`974149c6db3384319499cb878430320d8703c468209f1c453f516e9596699b85`,
chain `agent-one`, and the single `sibling/model-v1` observation with 50 input and
5 output tokens. Only the notified parent source changed.

### Deletion suppression and changed-fingerprint recovery

Real `delete_session_data({ provider: "claude", sessionId: "delete-session" })`
returned zero because no legacy token row matched, while its committed model
mutation still removed the observation and changed the retained source to
`suppressed` at `1784088139515`. The suppression fingerprint exactly matched
content fingerprint
`ae85768ae2c649d7cf54c73542c896943480e1df15fc59bb10d9a43d278a008b`.

The post-delete aggregate excluded the source from inventory scope and model
rows: `globalSessionCount=4`, Claude `scopedSessionCount=3`, five Claude models,
and no `delete/model-v1`. Generation 8 then completed with six of six sources
skipped, zero processed, zero observations written, and the deleted source still
suppressed with zero observation rows. Unchanged retained history therefore did
not resurrect the deletion.

A changed invalid-UTF-8 replacement notification was accepted from
`1784088203412` to `1784088203435`. At `1784088203437` the source still had the
same suppression baseline, zero observations, and user-safe diagnostic
`A model history source could not be parsed.` Suppression did not clear on the
failed attempt.

For the next valid changed fingerprint, a read-only sampler requested
0.5-millisecond intervals across notification and commit. Scheduling and database
work yielded these observed snapshots:

```text
1784088253791 suppressed, baseline hash present, 0 observations
1784088253831 ok, suppression null, new hash present, 1 observation
```

The HTTP request was accepted from `1784088253801` to `1784088253824`; the
replacement transaction stamped `lastSuccessAtMs=1784088253825` and installed
`delete/model-v2` with 600 input and 60 output tokens. Sampling did not expose an
intermediate unsuppressed-without-replacement state, but sampling alone cannot
prove its absence. Atomicity comes from the SQLite transaction that replaces the
observations and clears suppression in the same successful commit; the observed
before/after snapshots are consistent with that guarantee.

### Range, request, and existing-total independence

Concurrent native aggregate/history calls shared generated timestamp
`2026-07-15T04:04:49.793Z`. Both `1h` and `24h` aggregates reported five scoped
sessions, seven evidence rows, seven provider-qualified models, 1,883 attributed
plus 575 unattributed tokens, and 2,458 total. Their history sums were exactly
1,883 and 575; `1h` had 12 five-minute buckets over
`[2026-07-15T03:04:49.793Z, 2026-07-15T04:04:49.793Z)`, while `24h` had 24
hourly buckets over
`[2026-07-14T04:04:49.793Z, 2026-07-15T04:04:49.793Z)`.

Browser request-state checks used the separate isolated profile
`/tmp/quill-t037-browser-20260714.rvVTG0/profile`, Vite port `51937`, and Chrome
DevTools port `9238`. The launch was:

```bash
npm run dev -- --host 127.0.0.1 --port 51937
/opt/google/chrome/chrome --headless=new --disable-gpu --no-sandbox \
  --hide-scrollbars --remote-debugging-port=9238 \
  --user-data-dir=/tmp/quill-t037-browser-20260714.rvVTG0/profile \
  --noerrdialogs --no-first-run --ozone-platform=headless \
  --window-size=1440,1000 \
  'http://127.0.0.1:51937/?modelFixture=partial-sources&modelFailure=history'
```

With history alone failed, backfill Retry changed the visible retained-history
state from partial to pending while the history error and all three aggregate
model rows remained. With Retry alone failed, its error appeared inside the
backfill region while the successful history table and three model rows remained.
After changing only the browser control to an aggregate failure and selecting
`24H`, the aggregate error coexisted with 24 successful history rows; backfill
Retry still changed partial to pending without clearing either request-local
state. This complements the US1 aggregate/history Retry evidence above.

Before model mutations, two isolated legacy token snapshots contained 333 input,
66 output, 9 cache-creation, 12 cache-read, and 420 total tokens. Every checkpoint
retained exactly two rows and 420 tokens. Final real
`get_session_stats({ days: 30 })` returned one session, `avg_tokens=420`, and
`total_tokens=420`. Model backfill, pruning, failures, retries, suppression, and
replacement did not change existing token/session totals or create a
`token_hourly` row.

This was a debug-build manual integration walkthrough, not the release-artifact
performance run below. The 21,207-source corpus was generated for interruption
timing and does not claim the fixed 10,000/100,000-row benchmark profile.

After evidence capture, process and listener checks confirmed no T037 Tauri,
Vite, or Chrome process and no listener on ports `19937`, `51937`, `9237`, or
`9238`. Both generated fixture roots were then removed and verified absent:

```bash
rm -rf -- \
  /tmp/quill-t037-native-20260714.iqBrBZ \
  /tmp/quill-t037-browser-20260714.rvVTG0
```

## US3 session investigation evidence — 2026-07-15

This walkthrough drove the real React hooks and Models UI against the
argument-aware browser IPC fixtures. It used an isolated Chrome profile under
`/tmp`; it did not launch Tauri, open a SQLite database, or read normal session,
config, rules, Claude, or Codex paths.

### Reproducible browser controls

The run used Vite port `51947`, Chrome DevTools port `9247`, and fixture root
`/tmp/quill-t047-browser-20260715`. A coordinator terminal can launch both
processes in dedicated process groups and persist their exact group-leader PIDs:

```bash
RUN_ROOT=/tmp/quill-t047-browser-20260715
case "$RUN_ROOT" in
  /tmp/quill-t047-browser-*) ;;
  *) echo "Unsafe RUN_ROOT: $RUN_ROOT" >&2; exit 1 ;;
esac

occupied=$(ss -H -ltnp | rg ':(51947|9247)\b' || true)
test -z "$occupied" || {
  echo "Refusing to reuse occupied T047 ports:" >&2
  printf '%s\n' "$occupied" >&2
  exit 1
}
test ! -e "$RUN_ROOT" || {
  echo "RUN_ROOT already exists: $RUN_ROOT" >&2
  exit 1
}
mkdir -p "$RUN_ROOT/profile"

stop_recorded_group() {
  local pid_file="$1"
  local attempt=0
  local pgid
  while test ! -s "$pid_file" && test "$attempt" -lt 20; do
    attempt=$((attempt + 1))
    sleep 0.05
  done
  test -s "$pid_file" || return 0
  read -r pgid <"$pid_file"
  case "$pgid" in
    ''|*[!0-9]*) return 1 ;;
  esac
  kill -- "-$pgid" 2>/dev/null || true
}

cleanup_partial_launch() {
  trap - EXIT
  stop_recorded_group "$RUN_ROOT/chrome.pgid" || true
  stop_recorded_group "$RUN_ROOT/vite.pgid" || true
  attempt=0
  while ss -H -ltnp | rg -q ':(51947|9247)\b'; do
    attempt=$((attempt + 1))
    test "$attempt" -lt 100 || break
    sleep 0.1
  done
  case "$RUN_ROOT" in
    /tmp/quill-t047-browser-*) rm -rf -- "$RUN_ROOT" ;;
  esac
  echo "Stopped owned T047 groups and removed root after launch failure." >&2
}
trap 'status=$?; test "$status" -eq 0 || cleanup_partial_launch' EXIT

listener_pid() {
  local line
  line=$(ss -H -ltnp "sport = :$1")
  [[ "$line" =~ pid=([0-9]+) ]] || return 1
  printf '%s\n' "${BASH_REMATCH[1]}"
}

wait_for_owned_listener() {
  local port="$1"
  local pid_file="$2"
  local label="$3"
  local attempt=0
  local pid
  local expected_pgid
  local actual_pgid

  while test ! -s "$pid_file" && test "$attempt" -lt 100; do
    attempt=$((attempt + 1))
    sleep 0.1
  done
  test -s "$pid_file" || {
    echo "$label did not record its session leader within 10 seconds." >&2
    return 1
  }
  read -r expected_pgid <"$pid_file"
  case "$expected_pgid" in
    ''|*[!0-9]*) return 1 ;;
  esac

  attempt=0
  while test "$attempt" -lt 100; do
    kill -0 "$expected_pgid" 2>/dev/null || {
      echo "$label session leader $expected_pgid exited before readiness." >&2
      return 1
    }
    if pid=$(listener_pid "$port"); then
      actual_pgid=$(ps -o pgid= -p "$pid" | tr -d '[:space:]')
      test "$actual_pgid" = "$expected_pgid" || {
        echo "Port $port belongs to PGID $actual_pgid, not $expected_pgid." >&2
        return 1
      }
      printf '%s listener PID=%s PGID=%s\n' \
        "$label" "$pid" "$actual_pgid"
      return 0
    fi
    attempt=$((attempt + 1))
    sleep 0.1
  done
  echo "$label did not open port $port within 10 seconds." >&2
  return 1
}

setsid --fork bash -c '
  set -e
  pid_file=$1
  shift
  self_pid=$$
  self_pgid=$(ps -o pgid= -p "$self_pid" | tr -d "[:space:]")
  test "$self_pgid" = "$self_pid"
  tmp_file="${pid_file}.tmp.${self_pid}"
  printf "%s\n" "$self_pid" >"$tmp_file"
  mv -- "$tmp_file" "$pid_file"
  exec "$@"
' t047-vite "$RUN_ROOT/vite.pgid" \
  npm run dev -- --host 127.0.0.1 --port 51947 --strictPort \
  >"$RUN_ROOT/vite.log" 2>&1 &
VITE_SETSID_HELPER_PID=$!
printf 'Ignoring short-lived Vite setsid helper PID=%s\n' \
  "$VITE_SETSID_HELPER_PID"
wait_for_owned_listener 51947 "$RUN_ROOT/vite.pgid" Vite || exit 1
if test "${T047_FAIL_AFTER_VITE:-0}" = 1; then
  echo "Forced failure after Vite readiness." >&2
  exit 1
fi

setsid --fork bash -c '
  set -e
  pid_file=$1
  shift
  self_pid=$$
  self_pgid=$(ps -o pgid= -p "$self_pid" | tr -d "[:space:]")
  test "$self_pgid" = "$self_pid"
  tmp_file="${pid_file}.tmp.${self_pid}"
  printf "%s\n" "$self_pid" >"$tmp_file"
  mv -- "$tmp_file" "$pid_file"
  exec "$@"
' t047-chrome "$RUN_ROOT/chrome.pgid" \
  /opt/google/chrome/chrome \
  --headless=new --disable-gpu --no-sandbox \
  --hide-scrollbars --remote-debugging-port=9247 \
  --user-data-dir="$RUN_ROOT/profile" \
  --noerrdialogs --no-first-run --ozone-platform=headless \
  --window-size=1440,1200 \
  'http://127.0.0.1:51947/?modelFixture=complete' \
  >"$RUN_ROOT/chrome.log" 2>&1 &
CHROME_SETSID_HELPER_PID=$!
printf 'Ignoring short-lived Chrome setsid helper PID=%s\n' \
  "$CHROME_SETSID_HELPER_PID"
wait_for_owned_listener 9247 "$RUN_ROOT/chrome.pgid" Chrome || exit 1
read -r VITE_PGID <"$RUN_ROOT/vite.pgid"
read -r CHROME_PGID <"$RUN_ROOT/chrome.pgid"
printf 'Vite PGID=%s Chrome PGID=%s\n' "$VITE_PGID" "$CHROME_PGID"
```

Interactive Bash may assign a background `setsid` helper its own job-control
group, and `setsid --fork` makes `$!` identify only that short-lived helper.
Nothing above treats `$!` as an application PID. Each child wrapper verifies its
new session has `$$ == PGID`, atomically persists that value, then `exec`s the
application so the recorded leader identity remains stable.

`T047_FAIL_AFTER_VITE=1` is a validation-only control for the documented failure
trap. It exits after Vite ownership is proven but before Chrome starts; the trap
must stop Vite, clear both ports, and remove the guarded root.

The exact coordinator block was rerun under an interactive PTY with Bash flags
`himBHc` and monitor mode enabled. In the forced partial run, `$!` reported helper
PID `1468424`, while the wrapper recorded PGID `1468426` and Vite listened from
PID `1468549` in that group. The forced exit printed the trap cleanup message;
both ports and the root were absent immediately afterward.

Keep that coordinator terminal open. In a second terminal, define `cdp_eval`.
It discovers the Quill page from Chrome's `/json/list`, connects to that page's
exact `webSocketDebuggerUrl`, prints the chosen target ID/URL, evaluates the
supplied expression, waits for promises, and prints the returned value:

The recorded helper run used Node `v25.8.2`. It depends on global `fetch` and
`WebSocket`, so it is not a general Node 18 repo-baseline command. The feature
check fails before target discovery when either global is unavailable:

```bash
printf 'CDP helper Node: %s\n' "$(node --version)"
node -e '
  if (typeof fetch !== "function" || typeof WebSocket !== "function") {
    process.exit(1);
  }
' || {
  echo "CDP helper needs global fetch and WebSocket; use DevTools UI." >&2
  exit 1
}

cdp_eval() {
  CDP_EXPR="$1" node --input-type=module -e '
    const targets = await (
      await fetch("http://127.0.0.1:9247/json/list")
    ).json();
    const target = targets.find(
      ({ type, url }) =>
        type === "page" &&
        url.startsWith("http://127.0.0.1:51947/")
    );
    if (!target) throw new Error("Quill CDP page target was not found.");

    const socket = new WebSocket(target.webSocketDebuggerUrl);
    await new Promise((resolve, reject) => {
      socket.onopen = resolve;
      socket.onerror = reject;
    });
    const response = await new Promise((resolve, reject) => {
      socket.onmessage = ({ data }) => {
        const message = JSON.parse(data);
        if (message.id !== 1) return;
        if (message.error) reject(new Error(JSON.stringify(message.error)));
        else resolve(message);
      };
      socket.send(JSON.stringify({
        id: 1,
        method: "Runtime.evaluate",
        params: {
          expression: process.env.CDP_EXPR,
          awaitPromise: true,
          returnByValue: true
        }
      }));
    });
    if (response.result.exceptionDetails) {
      throw new Error(response.result.exceptionDetails.text);
    }
    console.error(`CDP target ${target.id}: ${target.url}`);
    console.log(JSON.stringify(
      response.result.result.value ?? response.result.result.description ?? null,
      null,
      2
    ));
    socket.close();
  '
}

cdp_eval '({ title: document.title, url: location.href })'
```

This run submitted each bounded Node command through Quill
`quill_execute`, with the function definition and desired `cdp_eval` call in the
same command payload. That is the required route when the context hook blocks a
raw local HTTP fetch; an ordinary terminal without that hook can run the function
directly. On Node 18, open `chrome://inspect/#devices` in a separate graphical
Chrome, configure `localhost:9247`, choose Inspect for the Quill page whose URL
starts `http://127.0.0.1:51947/`, and paste each expression inside the outer
`cdp_eval '…'` quotes into its Console. This DevTools path uses the same page
target and `Runtime.evaluate` behavior without the Node helper.

Navigate to Analytics, then install instrumentation before Models mounts. The
instrumentation records every IPC call and captures the 60-second fallback
refresh callback registered as Models mounts. Calling that captured callback
exercises the real poll-refresh path without an unbounded wait:

```bash
cdp_eval '(async () => {
  const analytics = Array.from(document.querySelectorAll("button"))
    .find((node) => node.textContent?.trim() === "Analytics");
  if (!analytics) throw new Error("Analytics navigation was not found.");
  analytics.click();
  await new Promise((resolve) => setTimeout(resolve, 500));
  return Array.from(document.querySelectorAll("[role=tab]"))
    .map((node) => node.textContent?.trim());
})()'

cdp_eval '(() => {
  window.__t047Calls = [];
  const originalInvoke = window.__TAURI_INTERNALS__.invoke.bind(
    window.__TAURI_INTERNALS__
  );
  window.__TAURI_INTERNALS__.invoke = (...args) => {
    window.__t047Calls.push({
      at: Date.now(),
      cmd: args[0],
      args: args[1]
    });
    return originalInvoke(...args);
  };

  const originalSetInterval = window.setInterval.bind(window);
  window.setInterval = (callback, delay, ...args) => {
    if (delay === 60000) {
      window.__t047ModelPoll = () => callback(...args);
      return 947047;
    }
    return originalSetInterval(callback, delay, ...args);
  };
  window.__t047SetModelFailure = (value) => {
    const url = new URL(location.href);
    if (value === null) url.searchParams.delete("modelFailure");
    else url.searchParams.set("modelFailure", value);
    history.replaceState(null, "", url);
  };
  return { instrumented: true };
})()'

cdp_eval '(async () => {
  const models = Array.from(document.querySelectorAll("[role=tab]"))
    .find((node) => node.textContent?.trim() === "Models");
  if (!models) throw new Error("Models tab was not found.");
  models.click();
  await new Promise((resolve) => setTimeout(resolve, 800));
  return {
    selected: models.getAttribute("aria-selected"),
    commands: window.__t047Calls.map(({ cmd }) => cmd)
  };
})()'
```

Select the Claude row, expand the chain-rich session, then load page two with
the actual table/disclosure buttons:

```bash
cdp_eval '(async () => {
  const row = Array.from(
    document.querySelectorAll(".model-usage-table__row")
  ).find((node) =>
    node.innerText.startsWith("CLAUDE") &&
    node.innerText.includes("shared/model.snapshot")
  );
  if (!row) throw new Error("Claude shared-model row was not found.");
  row.querySelector(".model-usage-table__inspect")?.click();
  await new Promise((resolve) => setTimeout(resolve, 400));
  return document.querySelector(".model-detail-panel__count")?.textContent;
})()'

cdp_eval '(async () => {
  const session = Array.from(
    document.querySelectorAll(".model-detail-panel__session")
  ).find((node) => node.innerText.includes("model-session-mixed"));
  if (!session) throw new Error("Chain-rich session was not found.");
  session.querySelector(".model-detail-panel__disclosure")?.click();
  await new Promise((resolve) => setTimeout(resolve, 400));
  return session.innerText;
})()'

cdp_eval '(async () => {
  const loadMore = Array.from(
    document.querySelectorAll(".model-detail-panel button")
  ).find((node) => node.textContent?.trim() === "Load more");
  if (!loadMore) throw new Error("Load more was not found.");
  loadMore.click();
  await new Promise((resolve) => setTimeout(resolve, 400));
  return document.querySelector(".model-detail-panel__count")?.textContent;
})()'
```

Failure controls change the URL without reloading, so retained state remains
observable. Run one control, trigger the captured poll, inspect
`window.__t047Calls` plus the rendered error, clear the failure, then activate
the specifically named Retry button:

```bash
cdp_eval '(async () => {
  window.__t047SetModelFailure("sessions");
  const callStart = window.__t047Calls.length;
  window.__t047ModelPoll();
  await new Promise((resolve) => setTimeout(resolve, 800));
  return {
    calls: window.__t047Calls.slice(callStart),
    count: document.querySelector(".model-detail-panel__count")?.textContent,
    errors: Array.from(document.querySelectorAll(".model-detail-panel__error"))
      .map((node) => node.innerText)
  };
})()'

cdp_eval '(async () => {
  window.__t047SetModelFailure(null);
  const retry = Array.from(
    document.querySelectorAll(".model-detail-panel button")
  ).find((node) => node.textContent?.trim() === "Retry refreshing sessions");
  if (!retry) throw new Error("Page Retry was not found.");
  retry.click();
  await new Promise((resolve) => setTimeout(resolve, 600));
  return window.__t047Calls.slice(-2);
})()'

cdp_eval '(async () => {
  window.__t047SetModelFailure("detail");
  window.__t047ModelPoll();
  await new Promise((resolve) => setTimeout(resolve, 800));
  const session = Array.from(
    document.querySelectorAll(".model-detail-panel__session")
  ).find((node) => node.innerText.includes("model-session-mixed"));
  return session?.querySelector(".model-detail-panel__row-error")?.innerText;
})()'

cdp_eval '(async () => {
  window.__t047SetModelFailure(null);
  const session = Array.from(
    document.querySelectorAll(".model-detail-panel__session")
  ).find((node) => node.innerText.includes("model-session-mixed"));
  const retry = Array.from(session?.querySelectorAll("button") ?? [])
    .find((node) => node.textContent?.trim() === "Retry session history");
  if (!retry) throw new Error("Row Retry was not found.");
  retry.click();
  await new Promise((resolve) => setTimeout(resolve, 400));
  return window.__t047Calls.at(-1);
})()'
```

The collapsed-cache and stale-row actions used these exact expressions:

```bash
cdp_eval '(async () => {
  const id = "model-detail-session-02";
  const row = Array.from(
    document.querySelectorAll(".model-detail-panel__session")
  ).find((node) => node.innerText.includes(id));
  const disclosure = row?.querySelector(".model-detail-panel__disclosure");
  if (!disclosure) throw new Error("Session 02 disclosure was not found.");
  disclosure.click();
  await new Promise((resolve) => setTimeout(resolve, 300));
  disclosure.click();
  const collapsedBeforeRefresh = disclosure.getAttribute("aria-expanded");
  const callStart = window.__t047Calls.length;
  window.__t047ModelPoll();
  await new Promise((resolve) => setTimeout(resolve, 600));
  const refreshCalls = window.__t047Calls.slice(callStart);
  disclosure.click();
  await new Promise((resolve) => setTimeout(resolve, 300));
  return {
    collapsedBeforeRefresh,
    refreshCalls,
    reopenCall: window.__t047Calls.at(-1),
    expandedAfter: disclosure.getAttribute("aria-expanded")
  };
})()'

cdp_eval '(async () => {
  const id = "model-detail-session-23";
  const beforeCount = document.querySelectorAll(
    ".model-detail-panel__session"
  ).length;
  const row = Array.from(
    document.querySelectorAll(".model-detail-panel__session")
  ).find((node) => node.innerText.includes(id));
  const disclosure = row?.querySelector(".model-detail-panel__disclosure");
  if (!disclosure) throw new Error("Session 23 disclosure was not found.");
  disclosure.click();
  await new Promise((resolve) => setTimeout(resolve, 300));
  const staleNotice = row.querySelector(
    ".model-detail-panel__stale"
  )?.textContent;
  await new Promise((resolve) => setTimeout(resolve, 3300));
  return {
    beforeCount,
    staleNotice,
    afterCount: document.querySelectorAll(
      ".model-detail-panel__session"
    ).length,
    header: document.querySelector(".model-detail-panel__count")?.textContent,
    rowStillPresent: Array.from(
      document.querySelectorAll(".model-detail-panel__session")
    ).some((node) => node.innerText.includes(id))
  };
})()'
```

Direct payload capture used the same instrumented invoke surface, not imported
fixture helpers:

```bash
cdp_eval '(async () => {
  const first = await window.__TAURI_INTERNALS__.invoke(
    "get_model_sessions",
    {
      range: "1h",
      modelProvider: "claude",
      modelId: "shared/model.snapshot",
      cursor: null,
      limit: 20
    }
  );
  const second = await window.__TAURI_INTERNALS__.invoke(
    "get_model_sessions",
    {
      range: "1h",
      modelProvider: "claude",
      modelId: "shared/model.snapshot",
      cursor: first.nextCursor,
      limit: 20
    }
  );
  const history = await window.__TAURI_INTERNALS__.invoke(
    "get_session_model_history",
    {
      provider: "claude",
      sessionId: "model-session-mixed",
      range: "1h"
    }
  );
  return { first, second, history };
})()'
```

Every observation below was read from returned payloads,
`window.__t047Calls`, `aria-expanded`, named error regions, session counts, and
rendered disclosure text after those exact actions.

### Paging and complete chain history

Selecting Claude / `shared/model.snapshot` issued
`get_model_sessions({ range: "1h", cursor: null, limit: 20 })`. The panel showed
`20 of 25 sessions`. Load more issued one request whose opaque cursor began
`qmf1.`, then showed `25 of 25 sessions` and removed Load more. Direct invocation
of the same two argument sets returned 20 and 5 unique session IDs, the same
total on both pages, and a null final cursor.

Expanding `model-session-mixed` issued exactly one row request and rendered the
same parent-first response returned by direct IPC:

| Chain | Chronological rendered segments | Switches | Token totals |
|---|---|---:|---|
| Parent `model-session-mixed` | `shared/model.snapshot` (2 repeated turns, 2,450 tokens) → model gap (1 turn) → `future/model.snapshot-2099` (1,800) → `tie/😀` (3,100) → `tie/Ω` (3,100) | 2 | 10,450 attributed; 90 unattributed |
| Subagent `agent-routing-a` | `future/model.snapshot-2099` (2 repeated turns, 330 tokens) → `shared/model.snapshot` (200) → model gap (1 turn) → `shared/model.snapshot` (220) | 1 | 750 attributed; 300 unattributed |

The exact rendered UTC windows were parent
`06:30:06.614–06:32:06.614`, `06:42:06.614`, `06:58:06.614`,
`07:04:06.614`, `07:05:06.614`; subagent
`06:35:06.614–06:43:06.614`, `06:53:06.614`, `06:54:06.614`, and
`06:55:06.614`, all on `2026-07-15`. This also exposes the real interleaving
while preserving chain-local order.

Parent and subagent activity timestamps interleaved, but the session total was
the sum of the independent chains: three switches, not an extra transition at a
chain boundary. Consecutive repeated models compressed into one segment. The
parent null-model turn and subagent null-model turn rendered as gaps and reset
adjacency; the subagent's separate token-only unattributed observation increased
coverage totals but created no segment and did not reset adjacency.

The response and disclosure both showed four distinct models, 11,200 attributed
tokens, 390 unattributed tokens, and `tie/Ω` as primary. `tie/Ω` and `tie/😀`
each had 3,100 attributed tokens and one turn. The lower Unicode scalar value
`Ω` therefore won the final raw-ID tie deterministically; no fixture-specific
model rank was involved. Parent segments appeared in timestamp order, followed
by the subagent chain and its own timestamp-ordered segments, so every observed
within-chain change remained available chronologically.

### Refresh replay, cache invalidation, and independent Retry

Two pages and `model-session-mixed` remained loaded for this matrix:

| Action | Observed IPC and UI result |
|---|---|
| Set `modelFailure=sessions`; trigger captured poll | Aggregate and history refreshed. Page replay called page one and failed. Expanded `model-session-mixed` still issued and completed its row request. All 25 sessions and full chain history remained visible beside `Retry refreshing sessions`. |
| Clear failure; activate page Retry | Exactly two `get_model_sessions` calls replayed null cursor then `qmf1.`. Panel returned to `25 of 25`; no row request was issued by page Retry. |
| Set `modelFailure=detail`; trigger captured poll | Both session pages replayed successfully. Expanded row request failed independently. All 25 sessions and last successful chain history remained visible beside `Retry session history`; no page error appeared. |
| Clear failure; activate row Retry | Exactly one `get_session_model_history` call for `model-session-mixed` succeeded; no page request was issued. |
| Expand then collapse `model-detail-session-02`; trigger poll; reopen | Initial expansion issued one row request. The disclosure was `aria-expanded="false"` during refresh and issued no row request then. Reopening issued one new row request, proving shared refresh invalidated its collapsed cache. Other expanded rows refreshed normally. |
| Expand `model-detail-session-23` | Its deterministic `not_found` response first rendered the stale-row notice. After the specified three-second removal window, observed count changed from 25 to 24, header read `24 of 25 sessions`, and that provider/session row was absent. |

These observations came from actual call arguments and post-render DOM state,
not inferred hook behavior. Both request families were observed after one
captured poll callback, with independent retained data, error regions, and Retry
controls. The implementation connects that callback to both hooks through one
`refreshGeneration`, but this walkthrough did not instrument or directly observe
the internal React state value itself.

### Scope and limitations

The browser mock's event plugin registers listeners but does not emit native
Tauri events, so this run invoked the captured fallback-poll callback. That is
the actual callback used by `useModelAnalytics`, but it does not validate native
event delivery or one-second event coalescing. Deterministic fixture responses
validate query arguments, React lifecycle, paging, cache, disclosure, and Retry
behavior; they do not replace the isolated SQLite query evidence gathered while
implementing the native commands. This run makes no release-build performance,
100,000-observation, or external-user usability claim.

The normal interactive run ignored Vite helper PID `1470970` and Chrome helper
PID `1471209`. It observed Vite listener PID `1471073` in persisted PGID
`1470972` and Chrome listener PID/PGID `1471213`; both ownership checks matched
the wrapper-recorded groups. Cleanup then used those persisted PGIDs, verified
each command line before signaling its process group, waited for both listeners
to disappear, and removed only the guarded fixture root. Clearing the
launch-failure trap first prevents it from racing the explicit stop sequence:

```bash
RUN_ROOT=/tmp/quill-t047-browser-20260715
case "$RUN_ROOT" in
  /tmp/quill-t047-browser-*) ;;
  *) echo "Unsafe RUN_ROOT: $RUN_ROOT" >&2; exit 1 ;;
esac
trap - EXIT

read -r VITE_PGID <"$RUN_ROOT/vite.pgid"
read -r CHROME_PGID <"$RUN_ROOT/chrome.pgid"
case "$VITE_PGID" in ''|*[!0-9]*) exit 1 ;; esac
case "$CHROME_PGID" in ''|*[!0-9]*) exit 1 ;; esac

stop_group() {
  local pgid="$1"
  local expected="$2"
  local command_line
  test -r "/proc/$pgid/cmdline" || return 0
  command_line=$(tr '\0' ' ' <"/proc/$pgid/cmdline")
  case "$command_line" in
    *"$expected"*) kill -- "-$pgid" ;;
    *)
      echo "Refusing to stop unexpected PGID $pgid: $command_line" >&2
      return 1
      ;;
  esac
}

stop_status=0
stop_group "$CHROME_PGID" "$RUN_ROOT/profile" || stop_status=1
stop_group "$VITE_PGID" "--port 51947" || stop_status=1
test "$stop_status" -eq 0 || exit 1

attempt=0
while ss -ltnp | rg -q ':(51947|9247)\b'; do
  attempt=$((attempt + 1))
  test "$attempt" -lt 100 || {
    ss -ltnp | rg ':(51947|9247)\b' >&2
    exit 1
  }
  sleep 0.1
done

case "$RUN_ROOT" in
  /tmp/quill-t047-browser-*) rm -rf -- "$RUN_ROOT" ;;
esac
test ! -e "$RUN_ROOT"
printf 'Stopped Vite PGID=%s and Chrome PGID=%s; ports clear; root removed\n' \
  "$VITE_PGID" "$CHROME_PGID"
```

Observed cleanup output was `Stopped Vite PGID=1470972 and Chrome
PGID=1471213; ports clear; root removed`. A final independent
`ss -ltnp | rg ':(51947|9247)\b'` returned no match, and the guarded root was
absent.

## Full isolated regression evidence — 2026-07-15

This run exercised the schema-v28 seeder, current debug native binary, real
Tauri IPC commands, and rendered Models DOM. Every data, rules, home, XDG,
Claude, and Codex path was beneath one guarded `/tmp` root. No normal Quill
configuration or retained transcript directory was read or written.

### Fixture, schema, and launch

The exact fixture root was
`/tmp/quill-t051-native-20260715.CLGVx4`. Setup rejected any resolved path
outside that root and any overlap with `~/.config/quill`, `~/.claude`, or
`~/.codex`:

```bash
RUN_ROOT=$(mktemp -d /tmp/quill-t051-native-20260715.XXXXXX)
case "$RUN_ROOT" in
  /tmp/quill-t051-native-20260715.*) ;;
  *) exit 1 ;;
esac
mkdir -p \
  "$RUN_ROOT/home" \
  "$RUN_ROOT/xdg/config" \
  "$RUN_ROOT/xdg/data" \
  "$RUN_ROOT/xdg/cache" \
  "$RUN_ROOT/xdg/runtime" \
  "$RUN_ROOT/data" \
  "$RUN_ROOT/rules" \
  "$RUN_ROOT/claude" \
  "$RUN_ROOT/codex"
chmod 0700 "$RUN_ROOT/xdg/runtime"

python3 scripts/populate_dummy_data.py \
  --data-dir "$RUN_ROOT/data" \
  --rules-dir "$RUN_ROOT/rules" \
  --projects-dir "$RUN_ROOT/claude" \
  --codex-sessions-dir "$RUN_ROOT/codex" \
  --no-backup \
  --seed 42
```

Seeder output reported schema versions `1–28`, 13 retained JSONL sources,
1,054 model observations, and a complete `2/2` provider-root inventory. The
13-file, 502,637-byte Claude-plus-Codex corpus hash was
`646bf81d721cbb93de70486b1dd8bf70e802176a8eb05915bee97788c16977db`.
The scale session contained 1,001 observations, 1,001 distinct accepted IDs,
and 14,011 attributed tokens before native launch.

The current source was rebuilt, then Vite and the native app ran in separate
terminals. Ports `8181`, `19951`, and `9251` were verified unused first:

```bash
cargo build --manifest-path src-tauri/Cargo.toml
npm run dev -- --host 127.0.0.1 --port 8181 --strictPort

env -u AT_SPI_BUS_ADDRESS \
HOME=/tmp/quill-t051-native-20260715.CLGVx4/home \
XDG_CONFIG_HOME=/tmp/quill-t051-native-20260715.CLGVx4/xdg/config \
XDG_DATA_HOME=/tmp/quill-t051-native-20260715.CLGVx4/xdg/data \
XDG_CACHE_HOME=/tmp/quill-t051-native-20260715.CLGVx4/xdg/cache \
XDG_RUNTIME_DIR=/tmp/quill-t051-native-20260715.CLGVx4/xdg/runtime \
DISPLAY=:0 \
QUILL_DEMO_MODE=1 \
QUILL_DATA_DIR=/tmp/quill-t051-native-20260715.CLGVx4/data \
QUILL_RULES_DIR=/tmp/quill-t051-native-20260715.CLGVx4/rules \
QUILL_CLAUDE_PROJECTS_DIR=/tmp/quill-t051-native-20260715.CLGVx4/claude \
QUILL_CODEX_SESSIONS_DIR=/tmp/quill-t051-native-20260715.CLGVx4/codex \
QUILL_PORT=19951 \
WEBKIT_INSPECTOR_HTTP_SERVER=127.0.0.1:9251 \
RUST_LOG=info \
dbus-run-session -- src-tauri/target/debug/quill
```

The native process logged the guarded data and rules paths. Its HTTP adapter
listened on `19951`, WebKit inspection on `9251`, and its XDG runtime directory
remained mode `0700`.

### Complete aggregate and detail representation

WebKit's outer target protocol forwarded JSON-encoded `Runtime.evaluate`
messages to `page-7` over
`ws://127.0.0.1:9251/socket/1/1/WebPage`. The evaluated page called the real
native surface, not browser fixtures:

```js
const aggregate = await window.__TAURI_INTERNALS__.invoke(
  "get_model_analytics",
  { range: "1h", provider: null },
);
const detail = await window.__TAURI_INTERNALS__.invoke(
  "get_session_model_history",
  {
    provider: "claude",
    sessionId: "demo-claude-model-scale-session",
    range: "1h",
  },
);
```

The result was final and complete: both `claude` and `codex` were represented,
`inventoryComplete=true`, `scopeFinal=true`, 1,014 aggregate model rows, 16,340
attributed plus 1,148 unattributed tokens, and 93.43549862763038% coverage.

For the provider-qualified scale identity, the aggregate table contained all
1,001 generated IDs from `demo/generated/model-0000` through
`demo/generated/model-1000`. Sorting by case-sensitive raw ID matched the
generated sequence at every index. SHA-256 of those IDs joined with newline was
`e056a2eee459bfe9369b2e787a7b993062c66d8621e69665fd10fc1c92030294`.

The session-detail response independently returned one parent chain, 1,001
distinct models, 1,001 chronological model segments, 1,000 within-chain
switches, 14,011 attributed tokens, and zero unattributed tokens. Its ordered
ID sequence matched all 1,001 expected values and produced the same SHA-256.
Thus neither complete aggregate table nor complete detail applied a model-count
cap or catalog filter.

### Debug-only highest-token diagnostics across every range

A second isolated native run used
`/tmp/quill-t051-ranges-20260715.joCBnE`, ports `19952` and `9252`, and the same
schema-v28 seeder inputs. It again contained 13 retained sources and 1,054 model
observations under isolated home, XDG, data, rules, Claude, and Codex roots.

Each range used the same diagnostic procedure. The UI first selected the range
without timing and returned to Now. One fresh real aggregate then established
that range's expected complete row count and token-descending first row. Timing
began immediately before activating `#analytics-tab-models` and stopped at the
first DOM sample where all four conditions held:

```text
#analytics-tab-models aria-selected = true
requested range button aria-pressed = true
.model-usage-table__data aria-busy = false
.model-usage-table__row count = fresh IPC model count
```

The visible first row matched the fresh IPC identity in every run:

| Range | Epoch start–end | Diagnostic duration | Complete rows | Visible highest-token row | Visible share | IPC share |
|---|---|---:|---:|---|---:|---:|
| `1h` | `1784106202548–1784106203512` | 965 ms | 1,014 / 1,014 | Claude / `demo-claude-opaque-3-0-1`, 497 tokens | 3% | 3.0416156670746632% |
| `24h` | `1784106205576–1784106206440` | 864 ms | 1,020 / 1,020 | Claude / `demo-claude-opaque-3-0-1`, 497 tokens | 2.8% | 2.77591599642538% |
| `7d` | `1784106207640–1784106208516` | 875 ms | 1,032 / 1,032 | Claude / `demo-claude-opaque-2-0-1`, 497 tokens | 2.3% | 2.2919068480516485% |
| `30d` | `1784106209672–1784106210551` | 879 ms | 1,032 / 1,032 | Claude / `demo-claude-opaque-2-0-1`, 497 tokens | 2.3% | 2.2919068480516485% |

All four diagnostic durations were below ten seconds, and the 1,001-ID scale
session remained inside every range. These are debug-build observations only.
They do not establish or qualify as formal SC-003 performance evidence. Formal
acceptance remains blocked until the fixed performance protocol can use one
unchanged release artifact built from one recorded commit and record its required
artifact and host metadata.

The earlier single-range debug observation measured 942 ms after a fresh count
sample. It is superseded by the four-range matrix above and likewise carries no
formal SC-003 claim. Its first attempt had reused a stale row count across a
moving `1h` boundary, so it was discarded rather than reported as success.

### Reconciliation and invariance

Before retry, read-only SQLite queries captured stable content rather than
mutable backfill timestamps. The canonical hash inputs were these ordered
projections:

```sql
SELECT id, timestamp, provider, bucket_key, bucket_label, utilization,
       resets_at, created_at
FROM usage_snapshots ORDER BY id;

SELECT id, session_id, hostname, timestamp, input_tokens, output_tokens,
       cache_creation_input_tokens, cache_read_input_tokens, cwd, created_at,
       provider
FROM token_snapshots ORDER BY id;

SELECT provider, session_id, agent_id, is_sidechain, timestamp, kind, uuid,
       parent_uuid
FROM session_events
ORDER BY provider, session_id, timestamp, kind, coalesce(uuid, ''),
         coalesce(agent_id, '');

SELECT provider, source_key, source_record_key, source_ordinal,
       observation_kind, source_session_id, analytics_session_id, chain_id,
       parent_chain_id, agent_id, turn_id, raw_model_id, cwd, hostname,
       is_sidechain, observed_at_ms, input_tokens, output_tokens,
       cache_creation_tokens, cache_read_tokens, model_evidence, token_evidence
FROM model_usage_observations
ORDER BY provider, source_key, source_record_key;
```

Each row was compact JSON encoded, prefixed by its 8-byte big-endian length,
and fed to SHA-256. Generation 2's real
`retry_model_history_backfill` invocation returned pending in 7 ms. The worker
committed complete from epoch `1784105452736` to `1784105452757` (21 ms), with
13/13 sources skipped, zero failed or remaining, and zero observations written.

All pre/post comparisons were equal:

| Invariant | Before and after |
|---|---|
| Schema | 28 rows, versions 1 through 28 |
| Provider snapshots | Claude 12,102; Codex 4,034; SHA-256 `81281ddd0c7abbb18586168fc2e5fd40fe2c6424982403e52620e7d37a41f66a` |
| Session events | 1,829 rows; Claude 1,136 events / 36 sessions; Codex 693 / 21; SHA-256 `6f74b896d50b126bc3941779b0c8d12e0bbe95de8600320518181a5f4dc11225` |
| Token snapshots | 678 rows; 2,899,930 input + 1,104,560 output + 678,933 cache creation + 1,665,924 cache read = 6,349,347; SHA-256 `005ff21d21056c624ad806235a45967ac7aa7123ea4ea8e12d4ee857d1933e19` |
| Model facts | 1,054 rows; SHA-256 `4cf3c402e6e0ccdee90a1214df8b34ae545e439a1aaa994ca6845bd1cc42f5ac` |
| Claude model dimensions | 1,038 observations / 10 sessions; 9,419 input, 4,363 output, 1,454 cache creation, 6,737 cache read |
| Codex model dimensions | 16 observations / 1 session; 476 input, 156 output, 44 cache creation, 184 cache read |

A second real retry checked query-level invariance under a stable `30d` scope.
Generation 3 committed complete from `1784105535533` to `1784105535572` with
13 skipped sources and zero writes. Pre/post aggregate payloads, excluding only
generated/backfill timestamps, were exactly equal: 1,032 model rows, 21,685
attributed plus 1,148 unattributed tokens, 22,833 total, 94.97218937502737%
coverage, 1,032 distinct provider-qualified models, and 11 multi-model sessions.
The complete scale detail payload was also exactly equal before and after:
1,001 accepted IDs, 14,011 tokens, and 1,000 switches.

These results establish unchanged-history idempotence and zero change to the
existing provider, session, and token datasets during backfill reconciliation.
They do not compare sliding-window aggregates across different timestamps; the
stable DB facts and same-scope generation-3 query comparison avoid that invalid
assumption.

### Cleanup and limits

The first native process group received Ctrl-C. Its separately recorded Vite group
was validated as this repository's `--port 8181` process before
`kill -- -1928537`. Ports `19951`, `9251`, and `8181` were then clear. Cleanup
accepted only the guarded prefix, rejected any surviving matching process or
listener, removed the fixture root, and verified it absent:

```bash
RUN_ROOT=/tmp/quill-t051-native-20260715.CLGVx4
case "$RUN_ROOT" in
  /tmp/quill-t051-native-20260715.*) ;;
  *) exit 1 ;;
esac
rm -rf -- "$RUN_ROOT"
test ! -e "$RUN_ROOT"
```

The four-range diagnostic repeated those checks for ports `19952`, `9252`, and
`8181`, then removed only
`/tmp/quill-t051-ranges-20260715.joCBnE` after verifying its native and Vite
process groups had stopped. Both guarded roots were absent after their runs.

This was a current debug-build regression, not T052's one-commit release
artifact or fixed 10,000/100,000-observation benchmark. Inspector-driven native
button activation supplies diagnostic functional and range coverage only; it
does not establish SC-003 and is not an external user study, so it also makes no
SC-009 claim. The app attempted enabled-provider usage refresh inside each
isolated home; Codex returned its expected unauthenticated warning, and every
integration/config write remained under the guarded roots.

## Existing verification commands

Run from repository root unless a command changes directory:

```bash
npm run lint
npm run typecheck
npm run build
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
cargo test --manifest-path src-tauri/Cargo.toml
lat check
```

## Performance validation

Use the fixed benchmark profile: four logical CPU cores available to Quill,
8 GiB system memory, and local solid-state storage. A stronger machine qualifies
only when CPU and memory are constrained to that profile. Build one release
artifact from one recorded commit and use that unchanged artifact for every run.
Record exact CPU model/logical-core limit, RAM limit, storage model/type, operating
system, commit, artifact hash, and fixture-corpus hash/counts.

Use these timer boundaries and runs:

- For each clean backfill, start with a fresh isolated Quill data directory and
  identical fixture hash. Read the full fixture corpus once immediately before
  each launch and do not clear the filesystem cache afterward. Start at committed
  `running` status and stop at committed terminal status. Run 10,000 readable
  sessions three times; each must complete within five minutes.
- During every backfill, switch Analytics tabs and change range every 30 seconds.
  Each interaction must render within two seconds without input lock or failure.
- Include separate unreadable-source and unreadable-root runs. Each must reach a
  correctly labeled terminal partial/failed result within five minutes and report
  source/root incompleteness separately.
- After one unmeasured warm-up in a single process with unchanged 100,000-row
  data, measure 100 Models activations. Start at tab activation and stop when the
  complete summary and rows render; at least 95 must finish within two seconds.
  Record p50, p95, pass count, and every failed duration.
- For live visibility, start after the complete record is flushed and transcript
  notification is accepted; stop when its provider-qualified row renders. Repeat
  during an event burst and require both runs to finish within five seconds.

Record raw start/end timestamps, terminal status, interaction latency, query/UI
latency, and fixture counts with the implementation evidence.

## Formal release benchmark evidence

This evidence applies the fixed protocol to one recorded commit and one unchanged
release artifact. Every process used real Tauri IPC, release WebKit, isolated
temporary roots, and a transient user-systemd cgroup. No browser mock or
production session/config root participated.

### Formal artifact and discarded diagnostics

Formal commit is
`568d77d06e24f98730059b608003a8436f27fc6c`. It was built once with
`pnpm tauri build --no-bundle --ci`; the artifact is
`.worktrees/model-analytics-benchmark/src-tauri/target/release/quill`.
It is 49,415,064 bytes, timestamped
`2026-07-15 14:10:10.775994352 -0700`, with SHA-256
`8b7afc78b82441ed0eba30d1fcb06194c65c94b13afdae3381bc9e099ec242aa`.
The worktree and artifact were rechecked after capture and still matched the
commit, size, and hash.

Earlier artifacts and candidate runs are diagnostics only:

- Commit `aaf6b510f1e84cd424f44f4560459df1702d8edc` exposed an actual
  persistence defect: 0 of 100 measured Models openings met two seconds
  (p50 3,657 ms; p95 3,752 ms). Its artifact and results are not formal claims.
- Commit `35b5aa8ba9839cf019c2b0553d1d1e3ea22e0b6d`, artifact SHA-256
  `ef756059c047e67e8bf7ead4a6b7880cf28873c9ced199538befd1b2efc048cb`,
  fixed tab persistence. A stronger semantic harness then measured a 2,144 ms
  range render and traced serialized reads plus refresh supersession. Its
  earlier benchmark tables are superseded.
- Unrecorded candidate artifacts, including SHA-256
  `bd1bc8e184b92d72cd2e02c7c085ff631ecc756b3b63c295c2d85a7d7e484720`,
  were root-cause probes only. Representative discarded raw files are
  `/tmp/quill-t052-v3-r1-range-threshold-failure.json`,
  `/tmp/quill-t052-perf-fix-r6-threshold-failure.json`, and
  `/tmp/quill-t052-perf-fix-r7-diagnostic.json`.

Two additional attempts were discarded for instrumentation defects, not product
failures. An initial run-2 evaluator required the UI to remain non-busy after
the first exact successful payload-to-DOM render; a valid later refresh could
start immediately, so that overconstraint threw and produced no accepted raw
result. The replacement is
`/tmp/quill-t052-formal-r2-monitor.json`. The first source-failure evaluator
compared an active 1H UI range with a direct 24H history response. Its raw
diagnostic is
`/tmp/quill-t052-formal-source-failure-diagnostic.json`; the matching-scope
rerun below replaced it.

### Fixed profile and fresh corpus

Profile: Ubuntu 24.04.4 LTS, Linux 6.17.0-29-generic, AMD Ryzen Threadripper
3970X 32-Core Processor, and `/tmp` ext4 on `/dev/nvme1n1p2`, a
non-rotating Sabrent Rocket 4.0 2TB NVMe device. Every service used CPUs
`0-3`, `memory.max=8589934592`, and `memory.swap.max=0`; each child
profile confirmed those limits.

The fixed readable corpus at `/tmp/quill-t052-formal-corpus` used
`baseMs=1784150545137`. It contained 10,000 Claude JSONL files/sessions,
100,000 observations, 256 calculated opaque model IDs, and 40,500,391 bytes.
Path-plus-content SHA-256 was
`7894303cf8df1474dfebefa2108af61c2bddfcd9a950524b96cfd56fc3d90c1c`.
Expected attributed tokens were 1,064,863; highest identity was Claude /
`bench/generated/model-0000` with 19,941 tokens. Every readable run used this
unchanged fresh corpus and a fresh data directory. Each run read the fixture
once immediately before launch without clearing filesystem cache.

### SC-004 readable and incomplete backfills

All three readable runs reached committed and rendered `complete` state with
two of two roots complete, 10,000 of 10,000 sources processed, zero failures or
remaining sources, 100,000 observations, 10,000 scoped sessions, 256 models,
1,064,863 attributed tokens, and 100% coverage.

| Run | Warm read | Commit duration | Start-to-visible bound | 24H range | Now | Models |
|---|---:|---:|---:|---:|---:|---:|
| 1 | 220 ms | 19,270 ms | 20,646 ms | 768 ms | 110 ms | 242 ms |
| 2 | 212 ms | 19,072 ms | 20,658 ms | 426 ms | 88 ms | 282 ms |
| 3 | 209 ms | 18,591 ms | 19,946 ms | 464 ms | 104 ms | 299 ms |

All commit and visible bounds were below five minutes. All nine interactions
were below two seconds. Range timing ended after successful range-specific model
rows and history matched their payloads through two paint frames, with summary
data present and non-busy at the initial match. Tab timing ended at substantive
Now content or retained Models data. The retained data may enter a later refresh
immediately after that match; no claim is made about a post-measurement busy
state.

The unreadable-source corpus contained 1,001 files, 4,054,091 bytes, and SHA-256
`4ac79e178ede0de14ea789b0e8a7538297ed8819683e5e8aeebbbcb7e3d413b5`.
Its 24 ms warm-read attempt exited 123 as intended. Commit took 1,751 ms; the
conservative visible bound was 36,470 ms. Final state was `partial`: two of
two roots complete, `inventoryComplete=true`, 1,000 of 1,001 sources
processed, one failed source, zero remaining, 10,000 observations, and 256
visible model rows. Diagnostic:
`A model history source could not be read.` Now/Models rendered in 79/262 ms.

The unreadable-root corpus contained 1,000 files, 4,050,040 bytes, and SHA-256
`2c357bc6845f182289dbf12b09a2b45cf076ac24c21c9fcc8722e3feb21a8cf6`.
Its 23 ms warm-read attempt exited 1 as intended. Commit took 1,732 ms; visible
bound was 4,707 ms. Final state was `partial`: one of two roots complete, one
failed root, `inventoryComplete=false`, 1,000 of 1,000 sources processed,
zero failed sources or remaining sources, 10,000 observations, and 256 visible
model rows. Diagnostic:
`codex transcript inventory could not read all filesystem entries.`
Now/Models rendered in 63/240 ms. Both cases preserved the source/root
distinction and reached visible terminal state below five minutes.

### SC-003 range walkthrough

Run 3 stayed unchanged after backfill. Each range first established native
expected data, then measured range selection through the matching successful
aggregate/history payload and exact DOM. Identification measured return from Now
through visible provider, raw ID, tokens, share, and complete rows.

| Range | Range load | Identify highest model | Evidence/history rows |
|---|---:|---:|---:|
| 1H | 1,315 ms | 193 ms | 100,000 / 256 / 12 |
| 24H | 1,380 ms | 111 ms | 100,000 / 256 / 24 |
| 7D | 1,333 ms | 181 ms | 100,000 / 256 / 28 |
| 30D | 1,345 ms | 164 ms | 100,000 / 256 / 30 |

Every range matched 1,064,863 attributed tokens, zero unattributed tokens, 100%
coverage, 256 distinct models, and 10,000 multi-model sessions. Highest identity
matched Claude / `bench/generated/model-0000`, 19,941 tokens, and 1.9% UI
share (1.8726352591835758% native). Every identification was below ten seconds.

### SC-007 persistent Models openings

On unchanged run-3 data, one unmeasured warm-up rendered 256 rows in 162 ms.
Then 100 Now-to-Models activations were timed through complete retained Models
data: 100 passed, zero failed, minimum 117 ms, p50 242 ms, p95 268 ms, and
maximum 275 ms. Runtime instrumentation recorded zero `get_model_*` fetches
during measured reopens.

### SC-002 fixed-window live visibility

The live run started with empty retained history, reached complete backfill, and
opened Models with its real `model-analytics-updated` listener active. Records
were written, flushed, and fsynced before authenticated notify requests; every
request returned HTTP 202 `queued`. Timing began at target acceptance and
stopped at the first exact provider-qualified ID and 23-token row render.

| Case | Accepted epoch | Rendered epoch | Duration |
|---|---:|---:|---:|
| Normal | `1784150675793` | `1784150676862` | 1,069 ms |
| Sustained target | `1784150688375` | `1784150689417` | 1,042 ms |

The sustained case accepted 12 peer notifications at 252–257 ms intervals.
Final peer acceptance was `1784150691437`: 3,062 ms after target acceptance
and 2,810 ms after first peer acceptance. Target rendered 2,020 ms before final
acceptance, proving later events did not reset the first event's window.
Post-run native data contained all 14 provider-qualified IDs. Both target IDs
rendered below five seconds without a model catalog, configuration change, or
new build. As above, a later refresh may begin after the recorded exact render;
no post-paint busy-state claim is included.

### Raw evidence, assertions, and cleanup

Raw readable evidence is
`/tmp/quill-t052-formal-r{1,2,3}-{monitor,warm,profile}.json`. Supporting
evidence is in `/tmp/quill-t052-formal-corpus-metadata.json`,
`/tmp/quill-t052-formal-profile.json`,
`/tmp/quill-t052-formal-sc003.json`,
`/tmp/quill-t052-formal-sc007.json`,
`/tmp/quill-t052-formal-{source,root}-failure*.json`, and
`/tmp/quill-t052-formal-live-{normal,sustained}-{arm,notify,render,verify}.json`.
Live baseline/profile metadata is retained under the same
`/tmp/quill-t052-formal-live-*` prefix.

Harness sources remain in `/tmp/quill-t052-harness/`, including
`generate_corpus.py`, `hash_corpus.py`, `launch_run.sh`,
`backfill_monitor.js`, `build_failure_corpora.sh`, `failure_monitor.js`,
`sc003_ranges_v2.js`, `sc007_openings_v2.js`, `live_notify.py`,
`live_notify_sustained.py`, `live_wait.js`, `live_verify.js`,
`stop_run.sh`, and `assert_formal.py`.
`/tmp/quill-t052-formal-assertions.json` records `"status": "pass"` and the
same artifact/corpus hashes. A fresh artifact SHA-256 check after measurement
also matched
`8b7afc78b82441ed0eba30d1fcb06194c65c94b13afdae3381bc9e099ec242aa`.

All formal application services, inspectors, private D-Bus sessions, and
isolated per-run data/config roots were stopped and removed with guarded
`/tmp/quill-t052-*` checks. Production roots were untouched. The formal
corpus, raw JSON, harness scripts, and exact release artifact remain only for
evidence review.
