# VibeMux Read Only Mobile Surface

Track A is an operator-started `vibemux_web` process. It polls the existing
authenticated daemon control IPC and serves a bounded status board on loopback.
The web process does not start the daemon, open SQLite, write canonical state,
attach terminals, cancel runs, approve tools, or change Git state.

The implementation is pre-alpha. Local automated checks and live-device,
tailnet, and release qualification are separate gates; see `PROGRESS.md`.

## Build and start

Build the frontend using Node **22.22.0**, before building the Rust crate:

```text
cd crates/vibemux_web/web
npm ci
npm run typecheck
npm test
npm audit --audit-level=low
npm run build
cd ../../..
cargo build -p vibemux_web --locked
cargo run -p vibemux_web --locked -- <project_root>
```

`web/dist` and `node_modules` are ignored and never committed. The compile-time
embedding guard rejects absent shell, manifest, service worker, or JS bundle.
`web-push` uses the ECE OpenSSL backend; CI provisions Perl for its vendored
build, which also requires the normal platform C toolchain.

The process is disabled when `<project_root>/.vibemux/web_config.json` is absent.
Invalid config or secrets fail closed. A validated config has this shape:

```json
{
  "schema_version": 1,
  "bind_address": "127.0.0.1:8420",
  "poll_seconds": 5,
  "instance_token": { "env_var": "VIBEMUX_WEB_TOKEN" },
  "enrollment_url": "https://operator.example/",
  "push_allowed_hosts": ["fcm.googleapis.com", "updates.push.services.mozilla.com"],
  "vapid_subject": "mailto:operator@example.com",
  "task_links": []
}
```

The example domains are placeholders, not exercised tailnet or push examples.
Use a cryptographically random token with at least 256 bits of entropy,
encoded as base64url without padding. Supply it through the named environment
variable or replace the reference with an absolute `secret_file` path whose
access controls are restricted. Never place the token itself in config,
command arguments, source, screenshots, or logs. The config allows exactly one
secret reference. Unknown fields, non-loopback binds, invalid endpoint hosts,
and unsafe file links are rejected.

The config has a 32 KiB cap. `poll_seconds` defaults to 5 and accepts 1–300;
`bind_address` defaults to `127.0.0.1:8420`. Explicit config values take
precedence over defaults; environment reads are limited to the named token
reference. The token is read once at startup. Replace it and restart to rotate
credentials; **rotation logs out every enrolled device**, invalidates all prior push subscriptions,
and keeps the permanent VAPID key pair. Devices must authenticate and register again.

One file lock in `.vibemux/web` admits one web process per project, even when
different HTTP ports are configured. The process owns only its web-local
VAPID/subscription/dedupe files. On Windows these use the existing restricted
ACL policy (current user, SYSTEM, and Administrators; no other principals); on
POSIX private files and directories require current-user ownership and modes
600/700. Existing insecure directories fail closed rather than being adopted.

## Authentication and enrollment

**Every HTTP route requires the instance bearer**, including HTML, CSS, JS,
manifest, service-worker assets, OpenAPI, and enrollment redemption. Browser
navigation cannot attach a bearer from JavaScript before loading those assets.
Consequently ordinary mobile browsing and QR-only enrollment are unavailable
without an operator-provided authenticated browser/bootstrap mechanism that
injects `Authorization` on navigation and all subresource requests. No public
shell, anonymous enrollment endpoint, query-string secret, or cookie fallback
is provided. Testing header injection in a browser fixture does not qualify an
operator's actual bootstrap mechanism.

An already authenticated browser can issue a QR invitation. The QR encodes
JSON containing the configured root URL and a one-time token; it is not a URL
with a token query. Redemption additionally requires the instance bearer and
submits the enrollment token only in a POST JSON body. Invitations expire after
120 seconds, are single-use, and are bounded to eight outstanding tokens.
Enrollment and authentication have bounded failed-attempt windows. Logged
enrollment records contain only time and accepted/rejected outcome.

The operator's own tailnet exposure is an external trust boundary. VibeMux
continues binding loopback and does not edit tailnet policy, start a tunnel,
or trust identity headers. No runnable `tailscale serve` example or remote
readiness claim is shipped without a live test on the operator's tailnet.

## Status API and freshness

The authenticated `/openapi.json` uses OpenAPI 3.1 and describes API version 0.1.0. GET operations
expose daemon health, task pages, and task details with runs and optional linked
dispatch/workflow status. The task snapshot is capped at 256 tasks, pages at
32 tasks, and refresh at 30 seconds. `truncated` exposes the snapshot cap.
Config `task_links` explicitly maps task UUIDs to optional dispatch/workflow
UUIDs; the existing protocol cannot enumerate those resources by task, so
missing links remain untracked. DSH status is an optional global projection.

Daemon generation changes rebuild the snapshot. A failed or raced refresh
marks prior data stale and hides task titles. Cached browser API responses
carry an explicit stale marker, expire quickly, and never render as current.
Browser cache partitions depend on a one-way bearer digest and are cleared
on sign-out/token change. Service-worker updates require an explicit prompt
action. A terminal exit or idle state is never interpreted as completion.

POST endpoints store web-local push subscriptions and issue/redeem enrollment
tokens only. No operation mutates daemon state. No stable compatibility promise
is made for this pre-alpha API; future changes follow ADR 033 and the repository
compatibility process. No store/control migration is needed. To roll back,
stop the web process and remove its config; retain or separately remove only
known web-local state if subscriptions are intentionally abandoned.

## Push and security boundaries

VAPID P-256 keys persist across process restarts. Push subscriptions are bounded
to 32, expire/prune locally, and are removed on 404/410; repeated permanent
failure quarantines delivery. Allowed endpoint hosts are exact operator-configured
names. Sending requires HTTPS on port 443, no userinfo/query/fragment/IP literal,
public DNS addresses pinned for the request, no proxy or redirect following,
bounded response bodies, deadlines, and bounded retries.

The encrypted push payload has exactly `kind`, `id`, and `state`. It carries no
title, branch, prompt, path, terminal output, free text, token, generation, or
control sequence. Generation and sequence remain in the local dedupe record.
The first observation establishes a baseline. A known entity changing state
can notify even after a daemon generation/sequence reset; unchanged state does
not notify. Claims are persisted before external delivery. Delivery is
best-effort: an interrupted/uncertain send may be lost and is not blindly
replayed after restart. Bounded in-call retries can produce vendor-level
duplicates; there is no exactly-once delivery claim. Notification clicks use
the fixed application root, never a payload-derived URL.

The accepted threat model is in `mobile_access_design.md` §7. The remaining
risks are compromised same-logon processes, a stolen shared instance token,
frontend XSS/dependency issues, denial of service through the deliberately
global bounded auth limiter, and vendor push delivery behavior. Static assets
receive strict CSP and security headers; scripts/workbox are bundled locally.
Live installed-PWA delivery on iOS and Android, real tailnet/bootstrap access,
Linux/WSL execution, and M9 release review remain separate unverified gates.

## Dependency review and changelog

Track A adds exact-pinned `include_dir` (MIT) for embedded assets, `qrcode`
(MIT/Apache-2.0, image backend disabled) for SVG QR generation, `fs4`
(MIT/Apache-2.0, synchronous file locks only), and `web-push` 0.11.0
(Apache-2.0, built-in clients disabled) for VAPID/ECE request construction.
The latter reuses the existing reqwest transport policy and brings ECE
(MPL-2.0), OpenSSL (Apache-2.0), and JWT crypto (ISC). JWT and its pure-Rust
transitive implementation are constrained in the lockfile for Rust 1.85.
This expands build/crypto dependencies; automated deny/audit outcomes are
recorded in `PROGRESS.md` without ignored vulnerabilities.

Version 0.1.0 adds the read-only mobile API/PWA, strict bearer routing,
enrollment, and minimal self-hosted push. It introduces no daemon operation,
canonical schema, A2A, terminal, or Python change. ADR 033 is Accepted;
Track A verification remains separate from Tracks B/C/D and release readiness.
