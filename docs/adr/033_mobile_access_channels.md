# ADR 033: Mobile access — tailnet web first, gateway and IM channels second

Status: Accepted (operator decision 2026-10-08). Supporting research and track designs live in
[docs/mobile_access_design.md](../mobile_access_design.md). Track A implementation and verification
are recorded separately in `PROGRESS.md`; Tracks B/C/D remain unimplemented.

Track A implementation decisions (2026-10-08): CI builds the untracked frontend dist with
Node 22.22.0; plain Vite, Svelte 5 and TypeScript; Web Push sends only entity IDs, kinds and
state names. All HTTP routes require the instance bearer, including enrollment redemption,
HTML, scripts and service-worker assets. Ordinary browser navigation and QR-only mobile
enrollment cannot bootstrap this policy; the operator must supply an authenticated browser
mechanism that attaches Authorization to all navigation and subresource requests. No public
shell or anonymous redemption exception is introduced. Enrollment still requires a short-lived
single-use token in the POST body in addition to the bearer.

Operator decision (2026-10-22): an opt-in `local_page_without_token` setting may serve the page
and its static assets without the bearer, for testing in an ordinary local browser. It applies
only to GET/HEAD requests whose Host names the loopback listener literally (`127.0.0.1:<port>`,
`localhost:<port>`, or `[::1]:<port>`); every JSON route, including enrollment and OpenAPI,
still requires the bearer, and the page carries no daemon data. This narrows the 2026-10-08
"no public shell" rule for that opt-in case only; it is off by default, and a proxy that rewrites
Host to the loopback address would publish the page, so the mode is not for a tailnet-published
port. The same change adds the ADR 042 console sections (`GET /api/v1/console`) as further
content-free reads.

Dispatch and workflow status queries require explicit IDs in web-local `task_links` configuration,
because the existing control protocol has no enumeration/by-task operation for those resources.
Absent links render as untracked; no IDs or completion states are inferred.


## Context

VibeMux runs are supervised only from the machine that hosts the daemon:
`vibemuxctl`, the egui Supervisor Chat, and the terminal UIs all attach
through the authenticated local control IPC, and the A2A gateway is
loopback-only by decision. Once a dispatch, DSH session, or dual-track
workflow is running, an operator away from the desk has no way to observe or
query its state.

The building blocks for a mobile surface already exist inside the control
protocol: bounded read-only `FrontendTasks`/`FrontendTask` queries on a
`TaskId` cursor, `HarnessDispatchOutput` and `DshSessionPoll` with
`after_sequence` paging, `WorkflowStatus`, and a polling-client precedent
(the GUI's 5 s `POLL_INTERVAL`). What does not exist is any web, IM,
notification, or bot surface, any server-push channel on the control IPC, or
any non-loopback endpoint. The engineering rules keep non-loopback exposure
fail-closed and unadvertised (ADR 024), defer the web/Tauri frontend and
multi-user authentication (`PROGRESS.md` M5 notes), and defer remote nodes
and Tailnet integration to M10.

Two external facts reshape the original proposal:

1. **The operator runs a tailnet.** `tailscale serve` publishes a
   loopback-bound service to the operator's own devices with automatic
   HTTPS, so a responsive web UI can reach the phone with no new
   VibeMux-operated listener. Tailscale authenticates devices, not requests,
   so an app-layer token stays mandatory.
2. **OpenClaw already ships the mobile apps.** Its gateway has maintained
   official Android (Google Play) and iOS companion apps, an established
   shared-secret/pairing model, webhook-based external integrations, and an
   agent that can call external tools. Feeding that gateway gives VibeMux
   notifications and chat control on both phone platforms through someone
   else's signed, maintained apps.

A further alignment pass (October 2026) confirmed the seams behind the
priority: Claude Code's official Remote Control validates the outbound-only,
QR-paired, approval-forwarding UX while relying on a vendor relay and
transcript storage VibeMux does not want; self-hosted Web Push works for
installed PWAs behind valid TLS (which a tailnet URL provides), so Track A
gets real push without a relay; OpenClaw's gateway documents a
token-protected webhook endpoint and registers configured MCP servers as
agent tools, fixing Track B's seam; Feishu's long connection carries
interactive-card callbacks (`card.action.trigger`), fixing Track C's
interaction path without a public callback URL.

The operator has set the surface priority accordingly (2026-10-06):
tailnet-exposed mobile web first, OpenClaw second, direct Feishu third, an
independent iOS and Android app last.

## Goals and non-goals

Goals:

- Run/task state on the phone — a real board UI plus notifications and
  read-only queries — with no new non-loopback listener operated by VibeMux.
- A connector seam so external gateways and IM platforms are reversible
  choices behind one process.
- Content-free payload defaults that keep prompts, diffs, secrets, and
  environments off external platforms permanently.
- A growth path to an installable PWA now and a packaged iOS/Android app
  later that reuses the same frontend.

Non-goals:

- A cloud relay, account system, or bridge service operated for VibeMux.
- Any non-loopback bind on the control plane or the A2A gateway; multi-user
  or public remote-browser authentication.
- Terminal streaming or any proxying of native TUI text (ADR 016's boundary
  stands).
- Git mutations, prompt relay, or harness tool-permission auto-approval from
  any phone surface.

## Decision

1. **Track order is fixed as A, B, C, D.** A: loopback-bound read-only
   mobile PWA published through the operator's tailnet. B: OpenClaw gateway
   connector. C: direct Feishu adapter. D: independent iOS/Android app via a
   Tauri 2 shell reusing A's frontend. Each track after A delivers through
   surfaces that already exist on the phone (browser, OpenClaw apps, Feishu)
   before VibeMux ships its own.

2. **Track A — `crates/vibemux_web`, a local client that also serves HTTP.**
   It authenticates to the daemon with the runtime descriptor's bearer
   token, polls the existing read-only operations (5 s default), and serves
   a static responsive PWA plus a JSON API bound to `127.0.0.1` with a
   per-instance bearer token on every route. Write actions are excluded. It
   never opens SQLite, never writes canonical state, and never proxies
   terminal text. Remote reachability is the documented operator pattern
   (`tailscale serve`); documentation states that the tailnet is the
   operator's trust boundary and makes no remote-readiness claim beyond it.
   Track A also owns phone-facing conveniences: self-hosted Web Push
   (VAPID keys generated and stored by `vibemux_web`, push messages POSTed
   directly to browser push endpoints over outbound HTTPS; iOS requires
   the installed-PWA form and valid TLS), a one-time QR enrollment flow
   for the per-instance token, and an OpenAPI-described JSON API. Bearer
   auth stays mandatory; Tailscale identity headers are optional signal,
   honored only when the request arrives directly from tailscaled.
   This partially reopens the M5 deferrals of a web/Tauri frontend and of
   remote browser access — by explicit operator decision, for a read-only,
   token-gated board published through the operator's own tailnet; the
   multi-user-authentication deferral stays fully in force.

3. **Track B — an OpenClaw connector inside `crates/vibemux_bot`.**
   Notifications are delivered to the operator's OpenClaw gateway (loopback
   HTTP when co-located, tailnet address otherwise); delivery is
   agent-mediated — the gateway's LLM reformats the summary into its
   configured channels — and direct native push of webhook events inside
   OpenClaw's own iOS/Android apps is not a documented guarantee, so the
   relay payload carries minimal verbosity only (IDs and states, never
   titles; design §6/B.1). The connector exposes a small read-only MCP
   tool surface on loopback (`list_tasks`, `get_task`, `dispatch_status`,
   `dsh_status`, `workflow_status`, `health`) for the gateway's agent,
   authenticated with the connector's bearer token (constant-time
   comparison) and bounded concurrency — an unauthenticated loopback
   listener is prohibited. Mutating tools do not exist. Gateway
   credentials follow OpenClaw's shared-secret/pairing model; VibeMux
   never stores chat-platform credentials.

4. **Track C — direct Feishu adapter in the same connector process.** The
   connector is a control-IPC client (daemon bearer token, poll loop over
   the existing cursors) that implements one in-crate
   `notification_channel` trait with two families: push webhooks
   (`feishu_custom_bot` first; DingTalk and WeCom robots are cheap
   follow-ons) and bidirectional outbound sessions (`feishu_long_connection`
   app-bot, whose outbound WebSocket carries both events and interactive
   card callbacks, `card.action.trigger`, so approval-style cards need no
   public callback URL; Telegram and DingTalk Stream are listed
   extensions). The connector never opens SQLite and never writes
   canonical state. Channel traffic is outbound-only, and the connector
   process opens no listening socket unless the Track B MCP surface is
   enabled. Read-only chat commands map 1:1 onto
   existing `ControlOperation`s.

5. **Track D — a Tauri 2 mobile shell around A's frontend, own ADR.**
   Packaging, signing, and distribution (self-hosted versus stores) and the
   app's local storage rules are decided there. Tauri 2 mobile is stable and
   actively maintained; the track is deliberately last because A–C deliver
   value without a mobile toolchain. Remote push in the shell is a known
   gap (Tauri's official notification plugin is local-only; third-party
   push plugins exist) — a Track D ADR decision, since Track A's web push
   already covers notification needs.

6. **Content policy is a contract, not a courtesy (Tracks B and C).**
   Payloads carry bounded, content-free projections: entity IDs and kinds,
   state names, harness kind, elapsed time, sequence numbers. Task titles
   appear only at `standard` verbosity or higher; branch and worktree names
   only at `verbose`; both are unredacted operator-authored text (length-
   bounded), and the default verbosity is an acceptance decision (design
   §9). Prompt content, diffs, file contents, environment
   values, and tokens are never sent at any setting.

7. **Deduplication is a protocol requirement (all delivery paths).**
   Notification dedupe keys are `(entity kind, entity id, observed state,
   control sequence)` and include the daemon generation, so neither a
   connector restart nor a daemon restart (which can reset control
   sequences) can suppress or double-send; clients replay from persisted
   cursors and re-read the descriptor on generation change. Notifications
   report canonical daemon states
   only; completion is never inferred from terminal text, pane exit, or idle
   time.

8. **Fail-closed configuration.** One external validated JSON file with
   `snake_case` keys per connector; disabled unless the file exists and
   validates. Bidirectional channels require non-empty chat/user
   allowlists. Token material is referenced by environment-variable name or
   a separate secret file, never stored in the config JSON, never logged,
   and never committed. Webhook URLs are https-only, validated at startup,
   and never followed through redirects.

9. **Degradation is local and bounded.** Bounded retries with backoff per
   delivery; a channel exceeding its threshold is marked unhealthy and
   skipped; malformed inbound data fails one message, never the process; the
   connector's crash or absence never affects the daemon, and no state is
   lost because cursors catch up on restart.

10. **Mutating phone actions stay out of scope here.** A later decision may
    add confirmation-gated cancel/pause-style operations (chat
    confirmations, or forms behind Track A's token); git-mutation commands,
    prompt relay, and harness tool-permission auto-approval do not exist at
    any phase. All inputs from phones and chats are parsed as data and never
    reach a shell or `.cmd` shim command string.

## Compatibility and rollback

- No control protocol, store schema, plugin protocol, A2A, or CLI change is
  required; Tracks A–C consume only existing operations and cursors. The
  dependency set does change: new exact-pinned crates (`web-push` ≥ 0.10.3
  — the CVE-2025-53604 floor —, a QR renderer, `include_dir`), a tokio
  `signal` feature addition, and the `web/` npm tree enter through the §16
  review, `cargo deny`, and `cargo audit`. Rolling back is deleting the
  crates, the config files, and (for Track A) stopping the local process,
  plus reverting the dependency lines.
- Track A's JSON API is a new, unpromised surface: it is versioned in its
  own right when it ships, and any later compatibility promise follows the
  §17 process with its own ADR note.

## Security and privacy

- Track A's only listener is loopback and token-gated; publishing it to the
  tailnet is the operator's action, and the tailnet is the documented trust
  boundary. The app-layer token remains mandatory because Tailscale
  authenticates devices, not requests.
- Tracks B and C push to external platforms — a deliberate data egress
  bounded by the content policy and allowlists; disabling a track is a
  config deletion or process stop.
- Read-only tool and command surfaces bound prompt-injection impact on
  VibeMux's own tools: an injected instruction can read status, nothing
  else. Because the Track B relay path hands the summary to the gateway's
  LLM — which holds the operator's other tools — that payload carries
  minimal verbosity only and never includes titles.
- Push subscriptions and VAPID keys are local secrets with bounded
  retries; push payloads are content-free; the VAPID private-key file
  carries an explicit user-only ACL; push endpoints are validated like
  webhooks (https-only, host policy, no redirects). Enrollment QRs carry
  one-time, short-TTL tokens redeemed by POST body; enrollment attempts
  (time and outcome, never token material) are logged. Enrollment does
  not replace the per-instance token. Public unauthenticated share links
  (the OpenCode-share footgun) are out of scope by construction: every
  route, including enrollment redemption, requires the token.
- Tokens and webhook secrets are never logged, serialized into events, or
  committed; diagnostic logs carry counters and correlation IDs only.
- No track grants data access beyond what a same-logon control-IPC client
  already holds. Track A does extend reachability of that read-only slice
  to a device outside the ADR 020 logon-SID boundary — a deliberate,
  bounded extension of the trust model that rests on the per-instance
  token, the content-free payloads, and the operator's tailnet policy.

## Verification required before acceptance

- Unit tests for cursor replay, dedupe-key stability across restarts,
  content-policy redaction at every verbosity level, allowlist enforcement,
  config validation failures, and (Track A) route authentication.
- Contract tests per channel/connector against recorded fixtures (message
  shape, size bounds, rate limits, backoff), plus failure-path tests
  (malformed inbound data, unhealthy channel, reconnect storms).
- Security tests: webhook and push-endpoint URL validation and redirect
  refusal, secret redaction in logs, constant-time bearer comparison, a
  demonstration that the connector opens no listening socket unless the
  Track B MCP surface is enabled (and that the MCP surface authenticates
  every request), and that every Track A route rejects unauthenticated
  requests; frontend checks that the delivered security headers hold (CSP
  with no inline scripts and non-inline service-worker registration) and
  that cached responses are never displayed as current.
- Track A platform checks: the PWA renders and functions at phone and
  desktop widths, and the documented `tailscale serve` path is exercised on
  a real tailnet before any documentation example ships.
- Track A push and enrollment checks: web-push delivery exercised against
  a real installed PWA on iOS and Android, stale-subscription cleanup, and
  one-time enrollment with expired and reused token rejection.
- The M9 security review covers the whole mobile stack before any release
  claim. This ADR's non-loopback wording is a bounded interpretation of
  ADR 024 for read-only, content-free surfaces published by the operator,
  to be re-examined there.
- The `AGENTS.md` §14 gates for touched code (`cargo fmt`, `clippy -D
  warnings`, `cargo test`/`nextest`, `cargo deny`/`audit`) on Windows.

## Alternatives

- **Native app with a relay bridge (Happy Coder style):** richest UX;
  rejected — requires always-on relay infrastructure, accounts, and an
  encryption design that conflict with the local-first, fail-closed posture
  of a pre-alpha release line.
- **First-party IM adapters before any web surface (the original proposal
  order):** cheapest notification path, but it delivers chat-density
  information only and builds interaction surfaces by hand while a
  responsive board over an existing tailnet provides more value sooner;
  the adapters remain Track C, not cancelled.
- **Wait for M10 remote nodes/Tailnet:** defers phone access behind a
  deferred milestone; none of the tracks here requires the gates that
  VibeMux-operated remote exposure requires, because VibeMux itself keeps
  binding loopback only — stated as a bounded interpretation of ADR 024
  for this surface, re-examined by the M9 security review.
