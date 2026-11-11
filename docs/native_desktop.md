# Native desktop workspace

The Tauri desktop and optional browser chat use the same Svelte workspace,
components, typography, and styles. They do **not** share execution authority,
credentials, or a browser profile. The desktop is a pre-alpha local UI and
configuration surface, not a live orchestration client.

## Build and open

Use Node 22.22.0 and the repository's pinned Rust toolchain. From the repository
root, in PowerShell:

```powershell
npm --prefix crates/vibemux_desktop/ui ci
npm --prefix crates/vibemux_desktop/ui run build
cargo run -p vibemux_desktop
```

The desktop has its own npm lockfile and renderer dependencies. It imports the
browser workspace's source, not its `node_modules` or `dist`. A normal Cargo
build uses the bundled `crates/vibemux_desktop/ui/dist` assets. For development,
run the UI's `npm run dev` and start Rust with `--no-default-features`.

This path opens Tauri, not the older egui demo. egui remains available until the
separate native project/Ask/task/console parity work is complete. See
[PROGRESS.md](../PROGRESS.md) for current qualification, and
[ADR 043](adr/043_tauri_desktop_app.md) for the boundary.

## Provider profiles

Open **Settings → Provider profiles**. Add a profile and choose:

- A stable profile ID and a display name.
- Anthropic Messages or OpenAI-compatible Chat Completions.
- A base URL, retaining any provider-specific versioned path.
- The provider's model ID or operator-defined model alias.
- API-key, bearer-token, or no authentication. Authenticated profiles contain
  only an environment-variable **name**, never a key value.

Choose an **Orchestrator profile**, then **Save profiles**. You can edit or
remove profiles and save the resulting configuration. Up to 16 profiles fit in
one 64 KiB, schema-1 `provider_profiles.json` file in the native app's configuration
directory. The file is separate from `desktop_settings.json`, browser history,
CC Switch, and both VibeMux databases.

Remote URLs require HTTPS. HTTP is accepted only for numeric loopback addresses,
such as `http://127.0.0.1:11434/v1` or `http://[::1]:8080/v1`. URLs cannot contain
user information, queries, fragments, or backslashes. Model IDs in these profiles
do not change the browser conversation record's model vocabulary.

**Saved means configuration only, not connected.** Saving validates syntax and
publishes local metadata. It does not read the referenced variable, discover
credentials, query a model list, test a connection, run a harness, or send a
prompt. There is no hidden provider request behind a Save action.

The main coding-workflow orchestrator still consumes its separately configured,
startup-pinned local gateway and role bindings. Its live-attestation and owned
fixture-permit gates remain in place. A selected desktop profile does not change
an active route, restart the daemon, grant execution, or replace those gates.
Direct upstream routing from these profiles is not implemented.

## Appearance and local drafts

Theme, font size, density, reduced motion, and sidebar preferences use the native
settings file. The renderer receives only those preferences and the start-page
value, not private project-folder or Ask-consent paths.

Optional local conversation records belong to the WebView's separate origin and
are never imported automatically from a browser. Web Locks and stale-record
checks remain required for writing them. When the WebView lacks Web Locks, the
workspace reports that local changes are unsaved; native appearance and provider
settings remain available independently. No fallback weakens history ownership.

Native chat submission, unported project/Ask/task/console actions, attachments,
exports, external links, and executable artifact previews remain unavailable.
The composer can retain an unsent draft; a disabled Send does not create a
canonical Task or produce a synthetic provider reply. Provider profiles are not
included in browser conversation exports.

## Failure and privacy behavior

- A missing provider file starts as an empty profile list. Malformed,
  unknown-version, oversized, linked, or non-regular provider files are not
  silently reset or overwritten. Preserve a copy and repair the file outside
  the app before retrying; upgrading may be necessary for a newer schema.
- Saves use an exclusive sibling staging file and atomic replacement. Independent
  app instances publish complete snapshots with last-writer-wins semantics.
- Settings errors return fixed messages rather than file paths, environment
  values, or fragments of invalid configuration.
- The new provider commands are granted only to the main window. The renderer
  retains IPC-only networking, bundled assets, navigation restrictions, and no
  generic filesystem, HTTP, shell, or opener plugin.

## Isolated native UI walkthrough

Build the desktop UI first. The optional Cargo example uses the production
command and window builders, a nonpersistent WebView, and a **new** output
folder for all native settings. Windows/Linux WebView data also stays under that
owned folder; incognito mode stays enabled on every platform. A runtime preflight
refuses missing WebView support without opening an installation dialog. Startup
and walkthrough deadlines bound the harness, and a watchdog timeout is a failed,
unverified shutdown, never passing evidence. The harness evaluates a trusted
local test plan; no JavaScript-evaluation command is added to production IPC.

```powershell
cargo run --locked --offline -p vibemux_desktop --example native_ui_smoke -- --script crates/vibemux_desktop/tests/fixtures/shared_workspace_smoke.json --output target/native-ui-smoke-check
```

The output directory must not already exist. Use a new name for each run. The
walkthrough exercises appearance, both presentations, disabled-send draft
retention, provider CRUD/selection, native validation refusals, settings privacy,
and page-reload persistence. `report.json` contains the actual result and
inspected WebView properties. `--hold-seconds 30` keeps the owned window visible
briefly after the result for a screenshot or manual inspection.

This is a local native-UI check, not signing/notarization, installer qualification,
Windows/WebView2 acceptance, or live model/harness verification. Browser-only
checks and a native build do not replace running this walkthrough on the target
platform.
