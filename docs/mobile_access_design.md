# Mobile Access Design — Tailnet Web First, Gateway and IM Channels Second

Status: design accepted by the operator on 2026-10-08; implementation evidence is recorded separately. The accompanying decision record
is [ADR 033](adr/033_mobile_access_channels.md) (`Accepted`). This document
records the research behind the proposal and the phased design. Nothing here
changes code, protocols, or schemas; acceptance is decided through the ADR.

Research status: converged after five review passes (October 2026) covering
competitor surfaces, OpenClaw seams, tailnet exposure, PWA/Tauri stack,
Feishu/DingTalk capabilities, and OSS implementations. The remaining open
questions in §9 are implementation-phase decisions, not research gaps.

Operator priority decision (2026-10-06): the preferred order is

1. mobile web exposed through the operator's own tailnet (Tailscale serve),
2. the OpenClaw assistant gateway as the second surface,
3. a direct Feishu adapter as the third,
4. and, eventually, an independent mobile app/GUI for both iOS and Android.

---

## 1. Problem and goals

VibeMux runs are supervised at a desk through `vibemuxctl`, the egui Supervisor
Chat, and the terminal UIs. Once a dispatch, DSH session, or dual-track
workflow is running, the operator has no way to see or query its state away
from the machine. The goal of mobile access is a phone-side companion surface
that:

- presents run/task state on the phone, with state-change notifications and
  read-only queries;
- adds no new non-loopback listener, public endpoint, or trust boundary
  operated by VibeMux itself;
- stays read-only until a later, explicitly accepted decision adds commands;
- preserves every product invariant in `AGENTS.md` §2 and the accepted ADRs;
- grows toward an installable app experience (PWA first, a packaged iOS and
  Android app later) without discarding earlier work.

Non-goals for this proposal:

- A cloud relay, an account system, or an end-to-end encrypted bridge service
  operated for VibeMux.
- Any non-loopback bind on the control plane or the A2A gateway.
- Multi-user or public remote-browser authentication.
- Replacing the desktop GUI, TUI, or DSH terminal surfaces, or proxying
  terminal text to a phone (ADR 016's rule that native TUI text is never
  parsed, emulated, or proxied remains in force).

---

## 2. Current state inventory

Verified against the code at the time of writing.

What already exists and is reusable:

- **Control IPC.** `crates/vibemuxd/src/control.rs` serves protocol v7
  (`CONTROL_PROTOCOL_V7`) over a Windows named pipe with
  `PIPE_REJECT_REMOTE_CLIENTS` and a 256-bit bearer token from the protected
  runtime descriptor. The `ControlOperation` enum defines the full operation
  table, including `FrontendTasks`/`FrontendTask` (bounded read-only pages on
  a `TaskId` cursor), `HarnessDispatchStatus`/`Output`, `DshSessionPoll`,
  and `WorkflowStatus`. Each connection carries one request; there is no
  server-push channel, so clients poll.
- **Polling-client precedent.** `crates/vibemux_frontend/src/supervisor_client.rs`
  polls the control pipe every 5 s (`POLL_INTERVAL`) and never opens the
  database — the exact client class a web/bot surface would join.
- **Content-free responses.** Frontend query responses are bounded and omit
  prompt bodies, matching the "no full prompts or secrets in default logs or
  events" invariant.
- **Paging cursors.** `HarnessDispatchOutput` and `DshSessionPoll` accept an
  `after_sequence` cursor; `FrontendTasks` pages by `TaskId`. Incremental
  delivery can therefore be built on existing request/response primitives
  without protocol changes.
- **Loopback A2A gateway.** `crates/vibemux_a2a` runs an axum HTTP+JSON /
  JSON-RPC / gRPC server bound to loopback with per-peer bearer credentials
  and SSE subscription streams. Push notifications are explicitly
  unsupported (`push_notification_not_supported`). Remote (non-loopback)
  deployment is fail-closed and unadvertised (ADR 024).
- **Event store.** Append-only envelopes with idempotency keys
  (`crates/vibemux_events`, `crates/vibemux_store`), but no subscription or
  webhook path exists anywhere in the repository.

What is missing for mobile access:

- no web, IM, notification, or bot surface of any kind (grep over crates,
  `src/`, docs, and `PROGRESS.md` finds no Telegram/Slack/webhook/QR/pairing
  code);
- no server-push channel on the control IPC (poll-only by design, ADR 032);
- no non-loopback endpoint (by design), and the M5 notes in `PROGRESS.md`
  explicitly defer a graphical web/Tauri frontend, remote browser access, and
  multi-user authentication.

---

## 3. Constraints from the engineering rules

The design must respect, and is shaped by:

| Rule | Source | Consequence for mobile access |
|---|---|---|
| Single writer; clients only send validated requests | `AGENTS.md` §2.9, ADR 015 | A mobile/bot client is a control-IPC client; it never opens SQLite and never writes canonical state. |
| No unauthenticated TCP listener anywhere | `docs/architecture.md` | Every HTTP surface added here authenticates with a bearer token and binds loopback only. (Track A's opt-in `local_page_without_token`, 2026-10-22, serves only its static page without the token, to requests that name the loopback listener; every data route still requires it.) |
| Non-loopback bind is fail-closed and unadvertised | ADR 024, `PROGRESS.md` M7.0 | Remote reachability comes from operator-side tailnet publishing, not from a VibeMux listener; documentation states the operator trust boundary explicitly. |
| No secrets, tokens, full prompts, or environments in messages or logs | `AGENTS.md` §2.14, §13 | Web and IM payloads carry bounded, content-free projections only. |
| User messages are data, never interpolated into command strings | `AGENTS.md` §2.11, §12.1 | Chat and web input is parsed as data; nothing from a phone reaches a shell or a `.cmd` shim. |
| No automatic stash/reset/commit/merge/push | `AGENTS.md` §2.12, ADR 009 | No phase offers git-mutation commands. |
| Plugins never get the database; omitted permissions are denied | `AGENTS.md` §2.17, §9, ADR 014/021 | Connector surfaces get only declared and granted permissions. |
| Terminal text, pane exit, and idle do not prove completion | `AGENTS.md` §2.5 | Screens and notifications report canonical states read from the daemon, never terminal heuristics. |
| Docs must not describe planned code as implemented or advertise remote readiness | `AGENTS.md` §19 | This document is marked as a proposal; tailnet exposure is documented as an operator pattern with an explicit trust-boundary statement. |

The tailnet pattern keeps the letter of ADR 024 intact: VibeMux binds
loopback only, and the operator's Tailscale installation performs the
publishing. What changes with the operator's decision is emphasis — a
loopback-bound read-only web surface is now the first thing to build, not a
deferred M5 item.

---

## 4. Industry survey

### 4.1 Happy Coder (slopus/happy)

An open-source mobile and web client for Claude Code and Codex. A local CLI
wrapper wraps the agent session and bridges it to a native phone app through
a relay: session sync, push notifications, approvals, file mentions, and
slash commands on the phone.

Lessons: the richest mobile UX, at the highest cost — an always-on relay
service, an account/pairing system, an encryption design, and app-store
distribution. Not proportionate for a pre-alpha local orchestrator, and it
conflicts with the local-first, fail-closed posture. Rejected.

### 4.2 OpenClaw gateway

A self-hosted assistant where one long-lived gateway process owns all
messaging surfaces (WhatsApp, Telegram, Slack, iMessage, WebChat, and
others). Control-plane clients connect over a loopback WebSocket with a
shared secret and device pairing; Tailscale-origin identity can substitute
for the shared secret; remote access is an SSH-tunnel or VPN decision, never
an open port. Side-effecting commands require idempotency keys with
server-side dedupe, and events are not replayed — clients refresh on gaps.

Community status (verified October 2026): the release line is v2026.7.x;
Android ships as an official Google Play app acting as a Gateway companion
node (chat, voice, approvals), iOS ships with offline per-gateway session and
transcript caches, and Apple Watch adds voice turns. Nodes can host
supervised session turns with Gateway-owned state.

Lessons adopted here:

- channel ownership belongs in one always-on local gateway process, not
  inside the core and not in per-chat scripts;
- side-effect dedupe is a protocol requirement, not an implementation detail;
- remote reachability is an operator tunnel decision; the product itself
  keeps loopback binds;
- **and the decisive one for the priority order:** OpenClaw already maintains
  mature iOS and Android apps. Integrating with the gateway gives VibeMux a
  working mobile app surface (notifications plus chat commands plus
  approvals UX) without writing, signing, and distributing one.

### 4.3 Lightweight Telegram control bots (CCGram, HeyAgent)

A category of small tools that notify and take a few commands for coding
agents over Telegram/Slack/WhatsApp. This is the dominant pattern for
personal use: minutes of setup, no infrastructure, notification-first. It
remains the model for the first-party direct-IM adapter (Feishu first, per
the operator's priority).

### 4.4 Vibe Kanban

A kanban orchestrator for coding agents whose mobile story is a responsive
web UI reached through a documented remote-access path — no native app. It
demonstrates the browser as the first-class mobile surface and documents the
same operator-tunnel shape proposed in Track A. (The project has announced
sunset, which is why it is a pattern reference, not a dependency candidate.)

### 4.5 Secure exposure pattern: Tailscale serve

The service keeps binding loopback; `tailscale serve` publishes it to the
operator's own tailnet with automatic HTTPS; mTLS client certificates are a
documented defense-in-depth option. The decisive property for VibeMux: the
code never opens a non-loopback bind, so the ADR 024 stance is untouched, and
the operator explicitly accepts the tailnet trust boundary. Community status
(October 2026): serve/Funnel remain stable and are the standard pattern for
reaching personal services from a phone; no breaking changes affect this
design.

### 4.6 Chinese IM bot APIs

All three major Chinese platforms support push without any inbound port:

- **Feishu (Lark):** custom bot webhook push with optional signature
  verification; an app bot adds interactive cards and a long-connection
  (outbound WebSocket) event subscription — the strongest card/interaction
  model of the three, and the operator's chosen first-party channel.
- **DingTalk:** custom robot webhook (keyword filter, signing, or IP
  allowlist security modes); Stream mode receives callback messages over an
  outbound WebSocket connection, so bidirectional traffic needs no public
  callback URL.
- **WeCom (enterprise WeChat):** group robot webhook push, rate-limited at
  about 20 messages per minute per robot per group; bidirectional use needs a
  WeCom app and a heavier setup.

### 4.7 Rust and frontend ecosystem

- `teloxide` (Bot API 9.1 framework) and `frankenstein` (thin Bot API
  client) are the maintained Rust Telegram options; the same review applies
  to whichever Feishu path is chosen (webhook POST needs only an HTTP
  client; long connection needs a WebSocket client).
- **Tauri 2** is stable with first-class iOS and Android support and is
  actively maintained (mobile support stable since v2.0; the JS/Rust API
  line continues regular releases into 2026). It packages an existing web
  frontend into a lightweight native shell, which makes the PWA-to-app
  progression concrete: build the responsive web UI once, wrap it later for
  iOS and Android.
- axum is already in the dependency tree (`vibemux_a2a`), so the loopback
  web surface adds no new framework.

### 4.8 Community landscape snapshot (October 2026)

The market has settled into three proven patterns for reaching coding-agent
runs from a phone; VibeMux's tracks map onto two of them without a relay:

- **Vendor relay + native app.** Claude Code ships official mobile apps and
  a web client; its Remote Control feature connects them to a session on
  the operator's machine over outbound-only TLS (the local session "never
  opens inbound ports"), forwards permission prompts to the phone as
  short-lived dialogs, and pairs via URL/QR — at the cost of transcripts
  stored on Anthropic servers while connected, a paid-plan requirement, and
  no support for gateways or custom base URLs. Cursor's web agents are
  managed from the browser or phone; Omnara and Terragon run agents behind
  hosted control planes (Omnara's SDK is open but its control plane is
  hosted-first). This pattern is validated but conflicts with VibeMux's
  local-first, no-vendor-cloud posture; the design borrows only its UX
  moves (QR pairing, forwarded approvals).
- **Local server + tunnel + responsive web.** The self-hosted WebUI family
  (for example sugyan/claude-code-webui: a Node backend spawning the CLI,
  WebSocket to the browser, loopback bind with no built-in auth) plus a
  tailnet is the DIY standard. Its recurring footgun is "no auth until
  port-forwarded"; VibeMux's token-gated Track A exists precisely to avoid
  it. OpenCode's headless `serve` mode (REST + OpenAPI spec, auth, CORS,
  mDNS discovery) is the best-in-class shape for the JSON API side.
- **IM gateway aggregation.** OpenClaw (v2026.7.x, official Android/iOS
  companion apps) is the maintained exemplar; Telegram controllers such as
  ccgram (inline approve/deny buttons) prove approvals work as chat
  interactions but die from surface churn — an argument for VibeMux owning
  one connector process instead of many one-off bridges.

Implementation lessons worth adopting: Happy's QR-code device pairing (keys
created on-device, relay sees ciphertext — the pairing flow, not the relay,
is what Track A should copy); OpenCode's OpenAPI-described control API; and
"approvals as typed, idempotent events" (what the Telegram bridges built ad
hoc). Anti-patterns to avoid, already excluded by ADR 032's direction:
agentapi-style terminal scraping, and OpenCode-share-style public
unauthenticated session links.

### 4.9 The decisive observations

1. **Tailnet exposure is free remote access — and now includes real push.**
   A loopback-bound PWA plus the operator's Tailscale gives the highest
   information density with zero new VibeMux-operated attack surface, and
   self-hosted Web Push (VAPID, sent directly by the local process to the
   browser vendors' push endpoints) works for installed PWAs on iOS 16.4+
   and Android, with the tailnet HTTPS satisfying the valid-TLS
   requirement. No relay or third-party push service is involved.
2. **OpenClaw is an app shortcut with a documented seam.** Its gateway
   exposes a token-protected webhook endpoint for external triggers and
   registers configured MCP servers as agent tools; a VibeMux connector
   POSTs state changes there (loopback or tailnet address) and serves
   read-only tools, and the operator gets both phone platforms through
   OpenClaw's maintained, signed apps.
3. **A direct IM adapter remains the no-dependency fallback — and Feishu's
   long connection carries card callbacks.** `card.action.trigger` arrives
   over the same outbound WebSocket, so approval-style cards need no public
   callback URL; there is no mature Rust SDK, so a hand-rolled client is
   the realistic path.

---

## 5. Track comparison

| Criterion | A: Tailnet PWA | B: OpenClaw gateway | C: Direct Feishu | D: Packaged app (Tauri) |
|---|---|---|---|---|
| New VibeMux-operated attack surface | none beyond loopback (tunnel is operator-side) | none beyond loopback (connector is a client) | none (outbound only) | none beyond A (same API) |
| Information density | high (full board) | low–medium (chat) | low–medium (chat/cards) | high (same as A) |
| Notification fit | good (self-hosted web push to the installed PWA) | excellent (native push via gateway apps) | good (webhook/card push) | good (as A, or plugin push) |
| Command fit (later phases) | forms/buttons | chat + approvals UX via gateway | chat/cards | forms/buttons |
| Implementation cost | medium (HTTP surface + ADR) | low–medium (connector + MCP tools) | low (webhook first) | medium (packaging, signing, store) |
| New dependencies | axum (already in tree) | MCP client/server types | one HTTP/WS client | Tauri 2 mobile toolchain |
| Independent app (iOS+Android) | installable PWA now; packaging later | yes — OpenClaw's existing apps | no | yes — own signed app |

Decision, following the operator's priority: **A first, B second, C third,
D last** — D reuses A's frontend so nothing is thrown away.

---

## 6. Phased design

### Track A — Tailnet-exposed read-only mobile PWA (first)

#### A.1 Process model

- New workspace member `crates/vibemux_web`, binary `vibemux_web`: a local
  control-IPC client in the ADR 015/028 class that also serves HTTP. It
  authenticates to the daemon with the runtime descriptor's bearer token,
  never opens SQLite, never writes canonical state, and owns nothing.
- HTTP listener binds `127.0.0.1` on a configurable port with a random
  per-instance bearer token; every route requires it, except that the opt-in
  `local_page_without_token` setting (2026-10-22) serves the static page and
  its assets without it to requests that name the loopback listener. No route
  proxies terminal text; no PTY/ConPTY attach is implied.
- The daemon is untouched: no new control operations, no schema change. The
  web client polls the same read-only operations the GUI uses (5 s default,
  configurable) and re-serves bounded JSON to the browser.
- Process model: one `vibemux_web` instance per project daemon (the daemon
  is per project root), mirroring the GUI; an operator with several
  projects runs several instances, and the phone reaches whichever instance
  its tailnet URL targets. The process is operator-started; when it exits,
  the board and push stop until it is started again — supervision and
  autostart are operator choices, and the daemon never spawns it.

#### A.2 Frontend

- Static, responsive single-page app (a small JS framework or pre-rendered
  assets — decided at implementation review) backed by a JSON API that
  mirrors the read-only control queries: task board, task detail
  (runs/dispatch/DSH/workflow status), daemon health.
- PWA manifest + service worker for an installable home-screen app with an
  offline shell; data is always fetched fresh (no stale state is ever shown
  as current, per the completion-authority rules). Caching strategy: the
  static shell is precached; JSON API responses use network-first with a
  short network timeout and a short cache expiration, never cache-first;
  when a response is served from cache the UI shows a stale-data notice.
  The update flow uses the
  standard service-worker mechanics (a `prompt`-style update with an
  explicit reload affordance is the presumptive default for a status
  board); background periodic update checks are Chromium-only and best
  treated as an optimization, never a data-freshness mechanism.
- Component structure keeps presentation decoupled from the transport layer
  so Track D can repackage the same frontend in a Tauri shell later.
- Write actions are excluded from Track A entirely.

#### A.3 Reaching the phone

- Documented operator pattern: `tailscale serve` publishes the loopback
  surface to the operator's tailnet with automatic HTTPS; the phone opens
  the tailnet URL. VibeMux documentation states explicitly that the tailnet
  is the operator's trust boundary, that VibeMux still binds loopback only,
  and that no remote-readiness claim is made beyond this pattern.
- App-layer bearer auth stays mandatory (Tailscale authenticates devices,
  not requests) — defense in depth, and it keeps the surface safe behind
  any other tunnel an operator might substitute.
- Scoping who may reach the surface is the operator's tailnet policy: the
  policy file is default-deny, and `acls`/`grants` rules with tags or
  autogroups can restrict reachability to specific devices (for example,
  only the operator's phone). VibeMux documents this as the operator-side
  control; it never edits the user's tailnet policy.

#### A.4 Web push notifications (self-hosted)

- The web client is an installable PWA (A.2); on top of that, `vibemux_web`
  acts as a self-hosted Web Push application server: it generates VAPID
  keys, stores push subscriptions in its own local state (never in the
  daemon), and POSTs push messages directly to the browser vendors' push
  endpoints over outbound HTTPS when tracked states change. No relay and
  no third-party push service are involved.
- Platform rules: iOS/iPadOS 16.4+ subscribes only after the site is
  installed to the Home Screen; a valid TLS certificate is required, which
  the tailnet URL provides; a service worker handles `push` events.
- Push payloads follow the Track C content policy: identifiers and state
  names only; the board itself stays the source of truth.
- Delivery is best-effort, deduped by the same
  `(entity kind, entity id, observed state, control sequence)` key as every
  other path; a stale or rejected subscription is dropped after bounded
  retries. Push endpoints are client-supplied URLs, so the sender applies
  the same webhook discipline as Track C (https-only, explicit host
  policy, no redirect following, size-bounded bodies) — a poisoned
  subscription must not turn the web process into a request pivot.
- The VAPID private-key file carries an explicit user-only ACL consistent
  with the runtime-directory rules; the service-worker push handler
  renders a fixed field whitelist and opens the fixed tailnet URL only,
  so a forged payload cannot become a notification lure. The subscription
  store is bounded, with periodic pruning of stale entries.

#### A.5 Phone enrollment and API contract

- Bearer-token enrollment is QR-based (Happy's pairing flow, minus the
  relay): `vibemux_web` renders a short-lived one-time enrollment QR
  encoding the tailnet URL and a single-use enrollment token (≥256 bits);
  the phone redeems it by POST body — never a GET query string, so the
  token never lands in browser history, screenshots, or access logs — for
  the per-instance token, which the PWA stores locally. Enrollment
  attempts (time and outcome only, never token material) are logged, and
  enrollment and auth routes carry a bounded failed-attempt limiter.
- All enrolled devices share the per-instance token; token rotation is
  logout-everything (re-enroll each device). A lost phone is handled by
  rotating the token — accepted as deliberately coarse until a later
  decision adds per-device credentials.
- The JSON API is described by a hand-written OpenAPI 3 document kept in
  sync by a route-coverage contract test (§10.2), so the future Tauri app
  and any third-party reader consume one contract.
- Tailscale identity headers (`Tailscale-User-Login` and friends) are
  optional additional signal, never a replacement for the token, and are
  only honored when the request arrives directly from tailscaled.

### Track B — OpenClaw gateway connector (second)

#### B.1 Integration shape

- The connector lives in the planned `crates/vibemux_bot` process (see
  C.1): a local client process that talks to the daemon over control IPC and
  to external gateways as a pure client.
- **Notifications out:** the gateway exposes a token-protected HTTP
  webhook endpoint for external triggers (enabled in its config with a
  shared secret and path). Its documented semantics are "wake the gateway
  and run the agent" — the connector POSTs a content-free summary at
  minimal verbosity only (IDs and states, never titles: task titles can
  embed attacker-influenced text, and the receiving LLM holds the
  operator's other tools), and the
  gateway's agent relays it into the operator's configured channels
  (Telegram, WhatsApp, and friends), which deliver native mobile push.
  Delivery through this path is therefore agent-mediated: what VibeMux
  sends is content-free, but the receiving channel sees the gateway's LLM
  formatting of it. Direct native push of third-party webhook events
  inside OpenClaw's own iOS/Android apps is not a documented guarantee —
  the apps' documented strengths are chat and approval UX. When the
  gateway is co-located the connector POSTs to loopback; otherwise to the
  tailnet address.
- **Queries in:** the connector exposes a small read-only MCP tool surface
  on loopback — authenticated like Track A: the connector's per-instance
  bearer token, constant-time comparison, and bounded concurrency, because
  an unauthenticated loopback listener would violate the
  no-unauthenticated-listener constraint and plain-HTTP loopback is
  reachable to browser-origin attacks such as DNS rebinding —
  (`list_tasks`, `get_task`, `dispatch_status`, `dsh_status`,
  `workflow_status`, `health`) registered in the gateway's configuration —
  configured MCP servers are exposed to its agent as plugin-owned tools —
  so the operator can ask questions in chat and get content-free answers.
  Mutating tools do not exist in this track, and a tricked read-loop
  cannot exhaust the connector because tool execution is concurrency-
  bounded.

#### B.2 Boundaries

- The connector authenticates to the OpenClaw gateway with its shared
  secret/pairing model; VibeMux never stores chat-platform credentials.
  The node protocol (the gateway's WebSocket hub with per-node capability
  registration, the same pairing used by its companion apps) is the deeper
  future seam — bidirectional presence and remote-control commands — and
  stays a listed extension; the read-only connector needs only webhook +
  MCP tools.
- OpenClaw's agent-facing tools are read-only and content-free (same policy
  as §6/C.3), which also bounds prompt-injection impact: an injected
  instruction can read status, nothing else.
- Dedupe of notifications uses the same
  `(entity kind, entity id, observed state, control sequence)` key as every
  other delivery path; OpenClaw's own "no replay, refresh on gaps" contract
  is respected rather than fought.

### Track C — Direct Feishu adapter (third)

#### C.1 Connector process

- New workspace member `crates/vibemux_bot`, binary `vibemux_bot`: the
  shared connector host for external channels (OpenClaw connector in Track
  B, Feishu adapter here). It is a control-IPC client: daemon bearer token,
  poll loop over `FrontendTasks`/`HarnessDispatchStatus`/`Output`/
  `DshSessionPoll`/`WorkflowStatus` with the existing cursors, 5 s default.
- It owns no canonical state; its crash or absence never affects the daemon,
  and cursors catch up on restart.

#### C.2 Adapter families

- One in-crate trait, `notification_channel`, with a bounded outbound
  message type, per-channel retry policy, and rate limiting:
  - **Family A — push webhooks (one-way):** `feishu_custom_bot` first;
    `dingtalk_custom_bot`, `wecom_custom_bot` are cheap follow-ons.
  - **Family B — bidirectional outbound:** `feishu_long_connection` (app
    bot, outbound WebSocket): events and interactive-card callbacks
    (`card.action.trigger`) both arrive over the same connection, so
    approval-style cards need no public callback URL. There is no mature
    Rust SDK, so the client is hand-rolled against the documented wire
    format. Telegram and DingTalk Stream are listed extensions, not
    initial scope; DingTalk additionally publishes a protocol-level access
    specification for custom clients (credential registration with
    AppKey/AppSecret, WebSocket connect with endpoint + ticket, topic
    subscription, ACK, heartbeat), so a hand-rolled Rust client there is
    officially supported — its multi-instance duplicate-consumption caveat
    is exactly what the shared dedupe keys handle. Feishu's long
    connection, by contrast, authenticates with an Event Ticket obtained
    via App ID/Secret and speaks JSON frames (headers plus event body,
    client ACK) whose exact wire format is SDK-defined rather than a
    published specification — a hand-rolled Rust client must track the
    official SDK's source for wire details, and private/self-hosted Feishu
    deployments do not support the mode at all.
- Outbound-only rule for every family: channel traffic is outbound-only,
  and the connector process opens no listening socket unless the Track B
  MCP surface is enabled (that surface is a loopback listener under the
  connector's bearer token — see B.1/B.2).
- Webhook URLs are token material (classified like the bot token: env var
  or secret file, never the config JSON) and are validated at startup
  (https scheme, explicit host policy), never followed through redirects,
  and size-bounded.

#### C.3 Message content policy (Tracks B and C share it)

- Default verbosity `minimal`: entity IDs and kinds, state names, harness
  kind, elapsed time, sequence numbers. `standard` adds short task titles;
  `verbose` adds branch/worktree names.
- Never sent under any setting: prompt or message content, diffs, file
  contents, environment variables, tokens, or anything not produced by the
  content-free control queries.
- Per-channel rate limits and a coalescing window (default a few seconds)
  collapse transition storms into one message with the final state.
- Read-only commands map 1:1 onto existing `ControlOperation`s. Mutating
  commands are excluded; a later decision may add confirmation-gated
  cancel/pause actions, and no git-mutation or prompt-relay command exists
  at any phase.

#### C.4 Configuration and failure behavior

- One external validated JSON file with `snake_case` keys per connector;
  disabled unless the file exists and validates. Bidirectional channels
  require non-empty chat/user allowlists. Token material is referenced by
  environment-variable name or a separate secret file, never stored in the
  config JSON, never logged, never committed; files live under `.vibemux`
  and are git-ignored.
- Bounded retries with backoff and jitter per delivery; unhealthy channels
  are skipped until backoff elapses; malformed inbound data fails one
  message, never the process; logs carry counters and correlation IDs only.

### Track D — Independent mobile app via Tauri 2 (last)

- Repackage the Track A frontend in a Tauri 2 mobile shell for iOS and
  Android, speaking the same loopback/tailnet JSON API. No second frontend
  codebase and no relay: the app is a client of the operator's own machine.
- Scope, signing, distribution (self-hosted IPA/APK versus stores), and the
  app's local storage rules get their own ADR. Tauri 2 mobile is stable and
  actively maintained (community status, §4.7), so the path is real — but it
  is deliberately last because Tracks A–C deliver value without a
  toolchain, signing, and release pipeline. Remote push in the shell is a
  known gap: Tauri's official notification plugin delivers local
  notifications only, and remote push needs a third-party plugin (several
  exist, including FCM/APNs/UnifiedPush variants) — a Track D ADR decision,
  not a blocker, since the PWA push path already covers notification needs.
- iOS specifics to carry into the Track D ADR: WKWebView does not
  guarantee localStorage persistence and backgrounding suspends
  JavaScript, so the shell must persist the instance token through a
  native plugin (Tauri Store or Stronghold) rather than web storage, must
  not rely on background execution for refresh, and must accept
  constrained automated testing on iOS (no WebDriver support).

---

## 7. Threat model notes

| Adversary / vector | Control | Residual risk |
|---|---|---|
| IM or assistant provider reads message plaintext | content-free defaults; no tokens in messages | task metadata (IDs, states, optionally titles) is visible to the receiving platform |
| Unauthorized chat participant | mandatory allowlists (Family B); webhook signing secrets where supported (Family A) | a mis-scoped allowlist leaks metadata |
| Tailnet device beyond the operator's own | operator's tailnet policy governs device admission; app-layer bearer token still required on every route (Tailscale authenticates devices, not requests) | a compromised tailnet device can reach the read-only board; content-free payloads bound the exposure |
| Forged commands / injected instructions | read-only tool and command surfaces; inputs parsed as data; mutating actions do not exist | an injected instruction can read status, nothing else |
| Misdirected webhook URL | https-only, explicit host policy, no redirect following, startup validation | operator error remains possible; validated at startup |
| Browser-side attacks on the PWA | bearer token per instance; no cookies; static assets only; JSON API mirrors content-free queries | XSS in frontend code is the main residual class — keep dependencies minimal and reviewed |
| Push subscription abuse / spam | subscriptions stored locally; VAPID keys guarded like secrets; content-free payloads; per-endpoint bounded retries | a compromised push endpoint receives only state metadata |
| Enrollment QR interception | one-time, short-TTL token redeemed by POST body (never a GET query string), tokens ≥256 bits; enrollment attempts (time and outcome only) are logged with a bounded failed-attempt limiter; the per-instance token is still required afterwards | a same-network observer must redeem the token within its TTL |
| Agent-mediated relay (Track B) | the gateway's LLM reformats the connector's summary before it reaches channels, and that LLM holds the operator's other tools — so the relay payload carries minimal verbosity only (IDs and states; titles never enter it, because task titles can embed attacker-influenced text from issues and repos) and nothing secret ever enters the payload | notification wording is outside VibeMux's control once handed to the gateway's agent |
| Local same-SID malware | inside the ADR 020 boundary by definition (same-SID processes are trusted local collaborators, and descriptor access is an accepted trust-model limitation); the tracks add no new privilege — the only new listeners are loopback and token-gated | unchanged from the existing trust model |
| Secret leakage | tokens and webhook secrets never logged, serialized into events, or committed; errors redacted | — |

Web and connector traffic to external platforms is deliberate data egress,
bounded by the content policy; disabling any track is a config deletion or
process stop.

---

## 8. Milestone mapping

- Tracks A–C are additive local-client work, the same class as the GUI and
  TUI precedents that ran ahead of the §7 execution order. They change no
  canonical state, store schema, control protocol, plugin contract, or A2A
  surface, so they do not reorder the immediate execution list. One
  caveat: Track A adds a Node/Vite frontend build to the repository's CI
  story (the repo has no Node build today) — a real infrastructure change
  under §7 item 2's CI-tooling umbrella; whether a pinned CI step builds
  `web/dist` or the dist is committed is decided at implementation review.
- Why read-only first: monitoring is the away-from-desk need, and it ships
  with zero canonical-surface change. The highest-value mutation, cancel,
  maps onto existing `HarnessDispatchCancel`/`DshSessionCancel` operations,
  so a later confirmation-gated Track A cancel needs only confirmation UX
  and a policy decision — not a new control operation; that decision is
  deliberately not bundled here.
- Track A partially reopens the M5 deferral of a "graphical web/Tauri
  frontend" — by explicit operator decision, for a read-only mobile board
  only. The deferral of multi-user authentication and of any
  VibeMux-operated remote access remains fully in force.
- The M9 security review still reviews the whole mobile stack before any
  release claim; Track D additionally requires its own ADR with signing and
  distribution decisions. Nothing here relaxes ADR 024, the M7.0 gates, or
  the M10 entry condition (M7 and M9 verified).

---

## 9. Open questions

1. **PWA frontend stack.** Research signal now points at plain Vite +
   Svelte 5 + TypeScript + `vite-plugin-pwa` (SolidJS runner-up; React 19
   rejected on payload/maintenance grounds) — final confirmation, and the
   SvelteKit-versus-plain-Vite call, happen at Track A implementation
   review with the dependency surface kept small enough for Track D's
   Tauri reuse.
2. **MCP tool typing.** The official Rust MCP SDK (`rmcp`, the
   modelcontextprotocol/rust-sdk) is the presumptive candidate for the
   loopback tool surface; the §16 dependency review must pin the current
   version and check against the known streamable-HTTP latency regression
   reported around v0.16.
3. **Feishu app-bot credentials.** Whether the interactive-card path (app
   bot, long connection) is worth the app setup over the zero-setup custom
   bot webhook at first delivery.
4. **Titles by default.** Whether `standard` verbosity (short task titles)
   is the default is a privacy trade-off to settle at ADR acceptance.
5. **Web-push key custody.** VAPID key generation and storage details for
   `vibemux_web` (per instance, file-backed, outside the daemon), settled
   in the Track A design review.

---

## 10. Implementation research (Track A — `vibemux_web`)

Status: converged after five passes (October 2026). Every named topic —
in-repo seams, dependencies, caching, embedding, key custody, platform
quirks — is resolved below or explicitly deferred to the implementation
review; remaining checklist items (exact pins for new crates, MSRV
verification, live device tests) are execution work, not research gaps.

### 10.1 In-repo seams (verified against code)

- **Control client is reusable — do not duplicate it.** `ControlClient`
  (`pub`, `crates/vibemuxd/src/control.rs`, `from_descriptor`) already
  reads the runtime descriptor, gates on the negotiated protocol version,
  and exposes typed operations (`health`, `frontend_tasks`,
  `frontend_task`, dispatch/DSH/workflow status). Both `vibemux_cli` and
  `vibemux_frontend` consume it through a path dependency on `vibemuxd`;
  `vibemux_web` follows the same pattern. Connection setup imitates the
  `connect()` sequence in `crates/vibemux_frontend/src/supervisor_client.rs`
  (descriptor existence, `validate_runtime_dir`, `verify_control_security`,
  then `from_descriptor`).
- **Paths and runtime security** come from `DaemonPaths`
  (`crates/vibemuxd/src/process.rs`: `descriptor_path`, `runtime_dir`,
  `validate_runtime_dir`, `verify_control_security`, and a test-only
  `from_project_root_with_local_app_data`).
- **Daemon restarts**: `ControlClient::same_generation()` detects a daemon
  restart; `vibemux_web` re-reads the descriptor and rebuilds its snapshot
  on generation change, and connector dedupe keys include the daemon
  generation so a restart-reset sequence number cannot suppress or
  duplicate notifications.
- **Loopback bind on Windows**: a default-configuration Windows Firewall
  does not prompt for `127.0.0.1` binds (prompts target external
  interfaces), but third-party antivirus/firewall products and Hyper-V
  reserved port ranges can fail the bind with an access-forbidden error —
  the port is configurable and this failure mode belongs in the operator
  notes.
- **HTTP house pattern**: axum is pinned `=0.8.7` in
  `[workspace.dependencies]`. `crates/vibemux_a2a/src/task_server.rs`
  establishes the conventions to copy: `Router::new().merge(...)` +
  `middleware::from_fn` layers + `DefaultBodyLimit` +
  `axum::serve(...).with_graceful_shutdown`; constant-time bearer
  comparison (`subtle`); and an ETag/`If-None-Match` → 304 + explicit
  `Cache-Control` middleware for static responses — the exact shape for
  serving the PWA assets and the OpenAPI document.
- **Config validation precedent**: `crates/vibemuxd/src/plugin_configuration.rs`
  (symlink check, byte-size cap, `deny_unknown_fields`, one snake_case
  error code) is the template for the bot/web config loader.
- **Workspace conventions**: members are listed explicitly in the root
  `Cargo.toml` (`crates/vibemux_web` gets appended there), resolver 3,
  edition 2024, rust-version 1.85, all dependencies exact-pinned
  (`=x.y.z`), `#![forbid(unsafe_code)]` in every lib, binaries via
  `src/bin/*.rs`.
- **Integration-test shape**: tempfile + `DaemonPaths` + a seeded writer
  store + `DaemonControlServer::start_for_paths` + a real named-pipe
  client, with polling helpers and explicit timeouts
  (`crates/vibemux_frontend/src/supervisor_client.rs` tests); daemon-level
  process tests live in `crates/vibemuxd/tests/`.

### 10.2 Dependencies under the §16 review

| Candidate | Role | Signal |
|---|---|---|
| `web-push` ≥ 0.10.3 | self-hosted Web Push sender | tokio-async, aes128gcm content encoding, VAPID via `VapidSignatureBuilder`; optional hyper client — build one instance and reuse it. **0.10.3 is the floor: earlier versions carry CVE-2025-53604 (memory-exhaustion DoS in the built-in clients via a large Content-Length integer).** Verify MSRV 1.85 and transitive crypto deps through `cargo deny`/`audit`. |
| QR rendering | enrollment QR | prefer a no-`unsafe`, SVG-output crate (the `qirust` class); zero-dependency alternative: render the QR client-side with a small vendored JS renderer on the loopback enrollment page. Decide at review. |
| OpenAPI document | JSON API contract | hand-written OpenAPI 3 file plus a contract test asserting route coverage — no macro surface, house-consistent; `utoipa-axum` is the fallback if drift becomes a maintenance burden. |
| Asset embedding | PWA dist served by the binary | no embedding precedent exists in the tree yet. Lean pick: one pinned `include_dir` dependency (whole-directory embed, no proc-macro) plus a hand-rolled MIME map for the fixed extension set, served through the ETag middleware with an SPA fallback route to `index.html`; `rust-embed` (proc-macro, automatic MIME, optional dev-from-disk) is the ergonomic alternative. Vite's `build.outDir` points into the crate so the dist is committed-free and embedded at compile time. |
| Frontend | read-only board | plain Vite + Svelte 5 + TypeScript + `vite-plugin-pwa` (no SvelteKit; Tauri documents plain Vite, and SvelteKit's SPA mode is the heavier alternative). SolidJS is the runner-up. The service worker matters only for the web target and is harmless inside a later Tauri shell. SW registration must use the non-inline mode so the no-inline CSP holds; the `web/` npm tree joins the §16 review with a committed lockfile and an audit gate, and vendor scripts (workbox) are bundled locally — never CDN `importScripts`. |

### 10.3 Proposed crate shape

`crates/vibemux_web`: `src/bin/vibemux_web.rs` (thin entry);
`src/server.rs` (router, bearer-auth middleware, ETag middleware, loopback
bind, graceful shutdown; security headers delivered as HTTP headers —
`default-src 'self'; connect-src 'self'; frame-ancestors 'none';
base-uri 'none'; form-action 'none'; referrer no-referrer`, no inline
scripts, so the policy
covers the service-worker scope); `src/control_bridge.rs` (5 s poll loop
over `ControlClient` into an internal snapshot — the `supervisor_client.rs`
pattern); `src/api.rs` (read-only JSON handlers mapping the bounded
frontend payloads); `src/push.rs` (VAPID keys, local subscription store,
sender with bounded retries); `src/enrollment.rs` (one-time enrollment
tokens, TTL, QR); `src/config.rs` (plugin-configuration-style loader);
`web/` (Svelte 5 + VitePWA sources, dist embedded). Bearer token is
per-instance; the JSON API exposes only the content-free frontend query
projections. Push keys and subscriptions follow the platform rules: the
VAPID P-256 pair is generated once at first start and stored permanently
in a permission-restricted file under the `.vibemux` runtime area
(regenerating invalidates existing subscriptions' trust; only the public
key reaches the frontend as `applicationServerKey`); subscriptions are
persisted as the full `PushSubscription` JSON (endpoint, `p256dh`,
`auth`); browsers expire subscriptions silently, so a 404/410 push
response drops the subscription (the bounded-retries rule) and the page
re-subscribes on its next load. Implementation note: the workspace pins
`tokio =1.47.1` without the `signal` feature, and graceful shutdown via
`tokio::signal::ctrl_c()` requires it — the crate adds
`features = ["signal"]` to its own tokio dependency line or the server
adopts a different shutdown trigger, decided at implementation review.

### 10.4 Test plan sketch

- Unit: enrollment-token TTL and single-use behavior; bearer rejection on
  every route; dedupe-key stability; VAPID signature construction; config
  validation failures.
- Integration (house pattern): real named-pipe fixture + `DaemonControlServer`,
  axum bound to `127.0.0.1:0`, requests through `reqwest` (already a
  workspace dependency) asserting auth on every route, route coverage
  against the OpenAPI document, and ETag/304 behavior.
- Web-push delivery cannot be end-to-end unit-tested without real browser
  endpoints: contract-test the request construction; live delivery on
  installed PWAs (iOS and Android) is part of the ADR's acceptance
  verification.

---

## 11. References

- Happy Coder: <https://happy.engineering> — <https://github.com/slopus/happy>
- OpenClaw gateway architecture:
  <https://docs.openclaw.ai/concepts/architecture>
- OpenClaw node hosting and companion apps:
  <https://docs.openclaw.ai> — Android app README (apps/android),
  release line v2026.7.x notes
- HeyAgent (CLI agent notifications): <https://shyft.ai>
- Vibe Kanban and its remote-access documentation:
  <https://www.vibekanban.com> — <https://vibekanban.com/docs/remote-access>
- Tailscale serve for local dev servers:
  <https://tailscale.com/docs/use-cases/application-testing/share-local-dev-server-with-team>
  — <https://tailscale.com/docs/reference/examples/serve>
- mTLS without Tailscale:
  <https://julianmeisel.dev/2026/01/securely-exposing-selfhosted-services-without-tailscale>
- OpenClaw external integration via webhooks:
  <https://resources.learnopenclaw.ai>
- Feishu custom bot guide:
  <https://open.feishu.cn/document/client-docs/bot-v3/add-custom-bot>
- WeCom group robot webhook:
  <https://www.tencentcloud.com/zh/document/product/1254/78645> —
  <https://cloud.tencent.com/document/product/1263/71731>
- DingTalk vs WeCom bot push comparison (Chinese):
  <https://blog.csdn.net/liumiaocn/article/details/103757666>
- Rust bot crates: <https://github.com/teloxide/teloxide> —
  <https://crates.io/crates/frankenstein>
- Tauri 2 (mobile support, docs): <https://v2.tauri.app>
- Claude Code on mobile and Remote Control:
  <https://code.claude.com/docs/en/mobile> —
  <https://code.claude.com/docs/en/remote-control>
- Cursor web agents: <https://cursor.com>
- Omnara: <https://github.com/omnara-ai/omnara> — <https://www.omnara.com>
- sugyan/claude-code-webui: <https://github.com/sugyan/claude-code-webui>
- coder/agentapi (terminal-bridge anti-pattern):
  <https://github.com/coder/agentapi>
- ccgram (Telegram approvals with inline buttons):
  <https://github.com/jsayubi/ccgram>
- OpenCode headless server mode: <https://opencode.ai/docs/server>
- Self-hosted Web Push (installed-PWA rules, VAPID): Apple's web push
  documentation at <https://developer.apple.com> — community spec at
  <https://dev.to> ("Self-Hosted Push Notifications", Oct 2025)
- Tailscale identity headers behind Serve:
  <https://tailscale.com> ("Four increasingly sophisticated ways to put a
  service on your tailnet")
- Tauri notification plugin (local-only on mobile):
  <https://v2.tauri.app/plugin/notification>
- OpenClaw webhook endpoint and MCP tool registration:
  <https://docs.openclaw.ai> (Webhooks; 配置: herramientas y proveedores
  personalizados)
- Tailscale policy file (default-deny ACLs and grants):
  <https://blog.starmorph.com> ("Tailscale 101", Feb 2026) — official
  policy docs at <https://tailscale.com/kb>
- 2026 frontend framework comparisons (bundle size, maintenance):
  <https://empire-ui.com/blog/frontend-framework-wars-2026> —
  <https://www.birjob.com/blog/svelte-solid-qwik-react-2026>
- Vite PWA plugin (update flows, periodic sync):
  <https://vite-pwa-org.netlify.app>
- DingTalk Stream protocol access specification (custom clients):
  <https://open.dingtalk.com/document/resourcedownload/stream-mode-protocol-access-description>
- OpenClaw webhook ingress semantics (wake/isolated agent runs):
  <https://openclaw-ai.com> (Webhooks)
- Feishu long-connection event mode (Event Ticket handshake, SDK frame
  handling): <https://open.feishu.cn/document> (使用长连接接收事件)
- Official Rust MCP SDK: <https://github.com/modelcontextprotocol/rust-sdk>
- Implementation-phase references: [`web-push` crate](https://docs.rs/web-push/latest/web_push)
  (pin ≥ 0.10.3; advisory tracked in the
  [RustSec advisory database](https://github.com/RustSec/advisory-db)) —
  [utoipa vs hand-written OpenAPI discussion](https://users.rust-lang.org)
  — [Tauri plain-Vite guide](https://v2.tauri.app/start/frontend/vite) —
  [vite-plugin-pwa for SvelteKit](https://vite-pwa-org.netlify.app/frameworks/sveltekit.html) —
  [Svelte 5 `$effect` docs](https://svelte.dev/docs/svelte/$effect) —
  [Push subscription expiry handling](https://pushpad.xyz/blog/the-push-api-and-its-wild-unsubscription-mechanism) —
  [vite-rs (alternative Vite-embed workflow)](https://github.com/Wulf/vite-rs) —
  [`include_dir` crate API](https://docs.rs/include_dir)
