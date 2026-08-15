## What's New

**Pi integration**

Quill now indexes and searches Pi sessions, follows live Pi activity, attributes
token and model usage, and gives Pi agents Quill's history and working-context
tools through Pi's extension API. Pi remains absent from LIMITS because Quill
has no supported Pi rate-limit source.

Pi sessions that were already active before this upgrade stay dark in Live until
Pi restarts and loads the new extension. Ephemeral Pi sessions now appear as
persistent Sessions rows with an EPHEMERAL badge. Quill stores Pi's per-message
cost data, but does not display it yet.

Enabling Pi installs the executable managed file `quill.ts` at
`~/.pi/agent/extensions/quill.ts`, or
`$PI_CODING_AGENT_DIR/extensions/quill.ts` when configured. Quill repairs and
self-updates that file. Disabling Pi removes it while preserving every other Pi
file.

Settings → Integrations reports the extension's connection, protocol, and
offline-spool health. Failed tracking waits in a bounded private spool and
replays after Quill becomes available.

Downgrade note: older Quill builds that do not understand provider `pi` drop
its saved enablement entry. If you downgrade and later return to a current
build, re-enable Pi.

**Session continuity removed**

Quill no longer captures or injects cross-session task hints. The next update
automatically removes its old hooks, local files, tables, and feature telemetry
while preserving working-context data and third-party hooks.

**The main window is now a widget**

The split Live/Analytics dashboard has been replaced by a single 360px
always-on-top monitoring widget: a compact LIMITS band with one row per
provider, and a switchable view below it — Usage, Models, and Context — all in
one window. Management surfaces (Sessions,
Learning, Instances, Settings) are unchanged and still open from the titlebar.

Because the old layout is gone, its preferences are gone with it. The saved
main-window size is discarded once (the widget owns its own geometry; your
window *position* is kept), and the layout, time-visualization, and
show-Live/show-Analytics settings have been removed from Settings → General
along with their entries in the config summary and Reset to defaults. On a
fresh install the widget starts pinned always-on-top; if you had already set
that preference either way, your choice is preserved.

**Database cleanup**

Quill removes an unused legacy analytics archive when upgrading your local
database. This change cannot be reversed: older Quill versions cannot open the
upgraded database. Use Compact database after upgrading to reclaim disk space.

If `usage.db.pre-model-wipe.bak` exists beside your current database, it is an
optional, user-controlled backup. Quill does not delete it automatically. You
may delete it to reclaim disk space only after confirming the current database
works and you no longer need the backup.

**Faster analytics groundwork**

Quill adds hourly rollups for faster Models and runtime analytics. Existing
model history is indexed in resumable background chunks after upgrade; Models
keeps using raw evidence and shows build progress until the index is complete.
Settings → Performance can rebuild the model index without replacing history
whose raw evidence was already pruned. The database-format upgrade is one-way:
older Quill versions cannot open it, but existing raw analytics remain intact.
