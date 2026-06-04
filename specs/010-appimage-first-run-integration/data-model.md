# Phase 1 Data Model: AppImage First-Run Self-Integration

The feature has no database schema change. It persists two small settings-table
rows and writes one desktop launcher file.

## Entity: Integration decision (settings rows)

Stored via `storage.get_setting`/`set_setting` (no migration needed).

| Key | Value | Meaning |
|-----|-------|---------|
| `appimage.integration` | `done` \| `declined` \| (unset) | Whether the user integrated, declined, or has not been asked. |
| `appimage.integration_path` | absolute path | The integrated AppImage path (e.g. `~/Applications/Quill.AppImage`); set when state is `done`. |

### State transitions

```text
(unset) --prompt: Add-------> done
(unset) --prompt: Not now---> declined
(unset/declined) --Settings button--> done
done --Settings button (re-run)--> done        # idempotent re-integration
*  --integration error--> (state unchanged / left unset)   # retryable
```

- The first-run prompt fires only when state is `(unset)` **and** running as an AppImage.
- `declined` suppresses the prompt permanently but never disables the Settings control.
- A failed integration leaves the state unrecorded so a later launch or click retries.

## Entity: Launcher entry (filesystem, not DB)

Written to `~/.local/share/applications/quill.desktop`.

| Field | Value |
|-------|-------|
| `Type` | `Application` |
| `Name` | `Quill` |
| `Exec` | `<integration_path> %U` |
| `Icon` | `quill` (resolves to the installed `hicolor` PNG) |
| `Categories` | `Utility;Development;` |
| `Terminal` | `false` |
| `StartupWMClass` | matches the app's window class |

Companion artifact: the icon PNG at
`~/.local/share/icons/hicolor/256x256/apps/quill.png`.

## Derived status (not stored)

`is_appimage` is computed at runtime from the `APPIMAGE` env var; `integrated` is
derived from `appimage.integration == done`. These feed the
`get_appimage_integration_status` contract (see `contracts/ipc-commands.md`) and
the Settings control's rendering.
