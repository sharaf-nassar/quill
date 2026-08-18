# Quill for Pi

Quill's Pi extension reports local session activity and exposes Quill's local working-context tools. It sends no message bodies and refuses non-loopback Quill URLs.

## Support

- Package: `@sharaf-nassar/quill-pi`
- Pi: `>=0.84.0 <1`
- Node.js: `>=22.19.0`
- Entry point: `quill.ts`, exported as the package root

Package releases use independent SemVer. Protocol 2 requires the exact reporter and Quill build pair; an incompatible server makes live push inert while persisted Pi evidence remains available for later reconciliation.

## Install and reload

Install globally through Pi, then run `/reload` in an open Pi process:

```bash
pi install npm:@sharaf-nassar/quill-pi
```

Quill desktop still owns `~/.config/quill/config.json`, its authentication secret, the loopback servers, feature settings, and indexed data. The package never creates credentials, tracking spools, extension logs, or remote fallbacks.

Pi packages run with user permissions. Review `quill.ts` and npm provenance before installation.

## Tracking persistence

For persistent sessions, lifecycle and direct-lineage evidence is appended as compact `quill-tracking` custom entries before the matching protocol-v2 live request. Pi buffers early entries and flushes them with its first assistant entry. Native Pi entries remain the only owner of prompts, messages, tool output, model, and usage content.

`--no-session` is intentionally untracked: no custom entry, live request, health subject, spool, or log is created. Transient delivery retries once, authentication reloads once, an unknown session reannounces once, and exact-pair mismatch stops later live pushes without removing persisted evidence.

When `PI_SUBAGENT_CHILD=1`, the extension registers tracking only. Root processes, including `--no-session`, retain the eight `quill_` tools and context router; children expose neither.

Child launchers such as pi-subagents may load Quill through their generic explicit `extensions` or `subagentOnlyExtensions` configuration and may report a generic runtime acknowledgement. Quill does not edit launcher settings, auto-inject the broker-selected reporter path, or pin a Quill-specific pi-subagents release. An ambient-disabled child without an explicitly configured Quill extension remains outside the supported tracking guarantee.

## Ownership and coexistence

Pi 0.84 resolves project and user extensions before package extensions. The first compatible Quill copy claims the reporter for that load. Later copies register no tools or handlers. Session shutdown releases the claim so `/reload`, `/new`, `/resume`, and `/fork` can elect again.

| Installed copies          | Active copy                   | Owner and result                                                                                                    |
| ------------------------- | ----------------------------- | ------------------------------------------------------------------------------------------------------------------- |
| Quill-managed only        | Managed `extensions/quill.ts` | Quill installs, repairs, updates, disables, and removes it.                                                         |
| npm only                  | npm package                   | Pi and the user own package state; Quill owns local config and ingestion.                                           |
| Managed and npm           | Managed copy                  | npm copy stays inert. Quill repair never edits Pi's npm store or settings.                                          |
| User `quill.ts` and npm   | User file first               | A compatible Quill file claims once. An unrelated file owns its own behavior; npm supplies the sole Quill reporter. |
| Project, managed, and npm | Project file first            | Project trust and Pi's native precedence apply, then the singleton claim prevents a second compatible reporter.     |

A user-owned global `extensions/quill.ts` blocks Quill's managed installation because both need the same path. Quill does not rename or delete it. Remove or rename that file before enabling the managed copy.

Copies older than `0.1.0` lack singleton election. Disable or remove them before mixing install channels.

## Lifecycle commands

Quill desktop owns only files carrying its managed marker, its AGENTS.md block, integration state, and deployment stamp. It never edits `settings.json`, `.pi/settings.json`, Pi's npm directories, project extensions, or unrelated files.

Pi owns npm operations:

```bash
pi update --extension npm:@sharaf-nassar/quill-pi
pi remove npm:@sharaf-nassar/quill-pi
```

Use `pi config` to disable or re-enable the npm resource without uninstalling it. Run `/reload` after install, repair, update, disable, or removal.

An unversioned install follows the registry's `latest` tag. Pin an exact version for a rollback or controlled downgrade:

```bash
pi install npm:@sharaf-nassar/quill-pi@0.1.0
```

Pi skips exact-version pins during bulk package updates. To return to current releases, install the unversioned package again. Quill desktop rollback affects only the managed copy; npm pins remain untouched. A newer package beside a managed copy stays inert until Pi reloads without the managed copy.

If Quill is disabled while another local provider keeps shared config, disable the npm package separately. Quill will not silently modify a user-installed package.

## Release contract

The repository's `publish-pi-extension.yml` workflow publishes `pi-vX.Y.Z` tags only when the tag, package version, and exported reporter version match. It runs the package and extension suites, inspects the npm tarball, then publishes from a GitHub-hosted runner.

The npm package must configure that exact workflow as its trusted publisher. The job uses GitHub OIDC with `id-token: write` and `npm publish --provenance --access public`. npm creates Sigstore provenance and registry publish attestations. No long-lived npm token or separate handwritten signature belongs in CI.

Before publishing, inspect the tag diff and `npm pack --dry-run --json`. Never republish a version. Fix forward with a new SemVer release, or move the npm `latest` tag back only during an incident.
