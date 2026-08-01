## What's New

**The main window is now a widget**

The split Live/Analytics dashboard has been replaced by a single 360px
always-on-top monitoring widget: a compact LIMITS band with one row per
provider, and a switchable view below it — Usage, Trends, Charts, Models, and
Context — all in one window. Everything the old dashboard showed is still
here, adapted to the narrower surface. Management surfaces (Sessions,
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
