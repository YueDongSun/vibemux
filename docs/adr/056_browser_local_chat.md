# ADR 056: Optional browser-local conversational workspace

Status: Accepted for the optional browser UI integration, 2026-10-29. Originally proposed on 2026-10-25; renumbered from the unpublished ADR 044 to avoid the concurrently owned worker-tools ADR.

## Context

The user requested a complete Claude-style conversational web UI and explicitly selected a separate web worktree. The current ADR 033 web surface is a content-free read-only monitor. The independently owned Tauri desktop workstream has different credential and command boundaries. Extending either surface silently would blur those contracts.

## Decision

Add an optional static Svelte workspace under `crates/vibemux_chat/web`, with its own package lock, local commands, namespaced version-1 browser conversation record, and token stylesheet. It does not alter the existing monitor, daemon, control protocol, SQLite schema, canonical Task/Run/Event model, or desktop webview.

The user configures a direct Anthropic browser API connection. The destination is fixed HTTPS `api.anthropic.com`; the app does not discover private provider credentials or accept an arbitrary network proxy. The API key is held only in page memory. The UI sends conversation context and selected attachments only on explicit submission, and validates a key with a Models API query. There is no production demo inference path. API-account billing and limits remain provider-owned.

Local records contain conversation messages, prepared attachments, feedback, model/mode metadata, and artifact content. They are local UI data and confer no execution authority. Corrupt saved records are preserved until explicit recovery/reset. Export and deletion apply only to this namespaced browser history. Deleted conversations offer a bounded undo action.

Amendment, 2026-11-06: a page acquires one exclusive Web Lock for its writable lifetime, using `ifAvailable` so a second tab fails visibly rather than waiting indefinitely. Every save and explicit reset checks that the stored record still equals the page's last observed/saved record while holding that lock. A stale page retains its local content for export and cannot overwrite or reset newer history. `pagehide` flushes synchronously before releasing ownership; a restored page must reacquire and revalidate. This requires Web Locks in a secure context (loopback/localhost or HTTPS); unsupported browsers keep unsaved content in memory with an explicit warning. Old app tabs must be reloaded on upgrade because they do not participate in this coordination. The version-1 record, storage key, and secret-excluding serialization remain unchanged.

The page saves a submitted turn before dispatch when storage is available, checkpoints active streaming after at most one second of continuous changes, and flushes completion, cancellation, visibility loss, and page exit. The idle delay remains 400 ms. Timer and event ownership belongs to one autosave controller with teardown; no asynchronous lock acquisition is relied upon during page exit. Crashes and browser scheduling/storage failures can still lose changes since the last successful checkpoint.

Markdown raw HTML is escaped; rendered output is sanitized. Generated HTML previews use an opaque-origin sandbox without same-origin permission and a CSP restricting subresource loads, scripted fetches, and form submission. Inline preview scripts may run. This is not complete network isolation: frame self-navigation is not fenced. The parent controls copy and download. Preview scripts have no daemon API, provider key, filesystem, or canonical state handle.

## Alternatives

- Reuse the existing read-only monitor: rejected for this slice because conversational submission changes its explicit boundary.
- Build inside the Tauri workstream: rejected for this slice because the user chose separate web work and another owner controls that implementation.
- Add a daemon/provider proxy now: deferred; that would require an independently reviewed authenticated transport, secret storage, cancellation, and deployment contract. This static client does not advertise those capabilities.

## Security and consequences

Any same-origin script compromise can access an in-memory key. Use the UI only from a trusted local origin or controlled HTTPS deployment, retain a strict dependency lock, and avoid third-party runtime scripts. Memory-only retention reduces persistence exposure but does not provide encrypted secret storage. Browser-local chat data and attachments are readable by code on that origin and by users of the browser profile.

Sanitization and preview isolation require failure-case tests. Network errors, provider rejections, truncation, cancellation, storage failure, and page interruption must remain explicit states. Provider replies never constitute canonical Run success. Publishing the optional app, adding tools, remote listeners, arbitrary endpoints, durable credentials, or daemon mutations requires separate authority and review.

## Validation

Require a production build, controlled service tests, narrow desktop/mobile interaction verification, keyboard/dialog checks, token contrast checks, and independent source review. Keep live provider verification, Windows desktop/native harness validation, hosted CI, and deployment claims separate from those checks.
