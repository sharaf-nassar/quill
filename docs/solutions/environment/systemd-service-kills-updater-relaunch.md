---
title: Diagnostic systemd service kills the updater relaunch child
date: 2026-09-14
component: Linux AppImage updater / diagnostic launch environment
tags: [systemd, cgroup, updater, relaunch, AppImage]
problem_type: environment
---

## Symptom

Quill installed 0.4.2, closed, and did not relaunch. A subsequent desktop
launch succeeded. Installation itself was not broken.

## Evidence

Local journal on 2026-09-14, UTC-07:00:

- 21:10:52: old PID 2778494 logged `Update 0.4.2 installed, relaunching...`.
- 21:10:53: systemd finished `quill-protected.service`.
- 21:11:10: GNOME started PID 616181 in `app-gnome-quill-616181.scope`.

Quill history records PID 2778494 as the main process of the temporary
`quill-protected.service` used during the transcript-memory investigation.
That service used memory limits and `MALLOC_ARENA_MAX=2`, without overriding
systemd's service exit/kill defaults. The transient unit was already unloaded
when investigated; a later `LoadState=not-found` response is not historical
unit-configuration evidence.

An isolated subprocess probe reproduced the failure without running Quill:
a parent service spawned a child with `start_new_session=True`, equivalent
to the updater's `setsid()`, waited for child readiness, then exited normally.
Both processes remained in the service cgroup. The child recorded SIGTERM
rather than reaching its post-parent-exit continuation. The service finished
successfully in about 99 ms, so a successful service exit alone did not prove
successful relaunch. The original relaunch child's signal was not separately
logged; the matching launch history, service-stop timing, and probe identify
the cleanup mechanism.

Repeating the probe with `ExitType=cgroup` allowed the child to continue
normally after its parent exited. The service finished after the child,
about 1.1 seconds later. Both transient probes were collected automatically.

## Cause

`spawn_delayed_relaunch` in `src-tauri/src/lib.rs` detaches the child from the
Unix session, not from its systemd cgroup. With the default service lifecycle,
main-process exit stops the service; `KillMode=control-group` terminates any
remaining child processes. This kills the relaunch child while it waits for
the predecessor to exit, before the child's logger initializes.

The updater/relaunch implementation predates commits `39361c7` and `108735e`.
Neither changed the relaunch helper, PID handshake, updater dependencies, or
shutdown policy. The diagnostic service environment exposed this failure.

## Prevention

Do not leave Quill running under a main-process-lifetime diagnostic service
when testing updates. Use a normal desktop launch, or a transient service
with `ExitType=cgroup` on a systemd version supporting it. This keeps the
service and memory limits alive while the replacement process runs. A user
scope can also follow the lifetime of the whole process group.

Do not switch to `KillMode=process` or `none` merely to leak the replacement
out of cleanup. Do not remove memory limits or restart a user's live app
without permission. Ordinary `setsid()` alone cannot solve cgroup cleanup.

The user's replacement Quill was already desktop-launched when investigated,
so no production restart, application replacement, or service-policy mutation
was needed or performed. This verifies the service mechanism, not a new
end-to-end production update cycle.

## References

- [systemd.kill](https://man.archlinux.org/man/systemd.kill.5.en): default
  `KillMode=control-group` and remaining-process cleanup.
- [systemd.service](https://man.archlinux.org/man/systemd.service.5.en):
  `ExitType=main` versus `ExitType=cgroup`.
- `docs/solutions/runtime-errors/transcript-reparsing-retains-glibc-arenas.md`
  records the original protected launch and its memory limits.
