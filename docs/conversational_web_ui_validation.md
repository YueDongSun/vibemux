# Conversational web UI validation

Evidence date: 2026-10-25. Candidate branch: `feat/conversational_web_ui`, based on `45308fc`. This records local frontend evidence; it does not change VibeMux's overall pre-alpha maturity.

## Scope

- Task: complete the requested optional conversational web workspace in a separate worktree.
- Owned modules: `crates/vibemux_chat/web`, its documentation, and `.github/workflows/chat_web.yml`.

## Implemented

- Responsive welcome, sidebar/drawer, time-grouped history and debounced title/message search.
- Pin, rename, duplicate, confirmed delete/clear, undo, Markdown/JSON export and account/settings views.
- Model/thinking controls, streaming/loading, stop, retry, regenerate, edit/resend, copy and local feedback.
- Auto-growing composer, keyboard controls, bounded attachment preparation, image previews and per-file removal.
- Sanitized highlighted Markdown and a persistent, resizable artifact panel with raw code, supported preview, copy, download and fullscreen.
- Persisted system/manual themes, reading size, density and behavioral defaults; guided memory-only API configuration.

## Behavior and invariants

- Browser conversations are not canonical Tasks, Runs or Events. The monitor, daemon writer, control protocol and desktop workstream are unchanged.
- Requests use the fixed Anthropic HTTPS origin only on explicit configuration validation or send. No real key was used during this verification.
- Corrupt browser records are preserved until explicit recovery/reset. Quota failures remain visible. API keys are excluded from saved history and exports.
- A new draft context resets pending attachments even when New chat leaves the welcome Composer mounted. Late reads cannot reinsert removed entries.
- Interrupted responses restore as stopped, and duplication preserves user messages that have no status field. Surviving message artifacts persist through ordinary follow-ups.

## Files changed

- `crates/vibemux_chat/web/src`: app orchestration, nine components, shared configuration/tokens, UI policies, persistence and provider services.
- `crates/vibemux_chat/web/tests`: controlled service, attachment, persistence, UI-policy and contrast checks.
- Package/lockfile, build configuration, favicon and development-only QA provider.
- Setup/design documentation, this evidence record, README/PROGRESS entries and a prepared CI job.

## Validation executed

- Node 22.22.2: `npm test -- --test-reporter=spec` — **58 passed, 0 failed, 0 skipped**.
- `npm run build` — successful production bundle; no Svelte compilation warnings.
- `npm audit --audit-level=moderate` — **0 vulnerabilities**.
- Contrast check — **30 semantic text/background pairs** meet 4.5:1 in light and dark themes, including accent chips, sidebar text and highlighted code comments.
- `vite build --mode qa` — rejected by the development-only guard. Production bundle search contains no QA key or fixture copy.
- Controlled browser walkthrough: create conversation; select model/mode; validate fixture configuration; observe incremental streaming; stop/regenerate/edit; inline retry; attachment preparation/removal; live HTML artifact button; code/preview/fullscreen; copy/download; rename/pin/duplicate/delete/Undo; content search; settings and exports; theme, feedback and artifact restoration.
- Duplication was reproduced failing export, corrected, then verified with a **300-message** duplicate through actual Markdown/JSON download and reload, with no storage error banner.
- A **300-message** history renders **40** messages initially and **80** after one earlier-page action.
- Keyboard/focus checks: composer focus, new chat, sidebar/search shortcuts, Ctrl+Enter, Enter setting, native dialog Tab containment/Escape, and visible drawer close controls.
- Offline emulation: disconnected indicator appeared; sending preserved the draft. Online state was restored and verified afterward.
- Final production preview: empty guided state and local error handling verified with **zero captured browser errors or warnings**. No QA model response was used in production mode.

## Platform coverage

- Windows 11, Node 22.22.2, Svelte 5.57.2, Vite 8.3.3, Chromium in-app browser.
- Desktop 1440×900, tablet 1024×768 and mobile 390×844 checked. No page-level horizontal overflow observed. Tablet sidebar access, keyboard panel resizing and mobile fullscreen artifacts were exercised.
- Linux/WSL, Safari/Firefox and native terminal/harness backends were not exercised by this frontend-only work.

## Compatibility / migration

- Separate package and namespaced browser history; no daemon, SQLite, Control or existing monitor migration.
- New timestamps use UTC/RFC 3339; earlier numeric browser fixture records remain readable.
- ADR 044 is proposed for integration review. This branch does not merge or publish the optional UI.

## Security impact

- Markdown raw HTML is escaped and rendered output sanitized. Preview HTML is sandboxed without same-origin access, networking or form submission.
- Keys live only in page memory and clear on reload. User-selected files are prepared locally and included with explicit sends.
- The maintained `@lucide/svelte` package replaces the deprecated package name; its published ISC license is compatible with this source distribution.
- Independent scoped source review passed after the identified persistence, draft, attachment and artifact findings were resolved.

## Remaining work

- A real API key is needed to exercise live provider access, account-specific models, billing and limits. Controlled fixtures are not live inference evidence.
- Hosted CI has been prepared but not run. Integration and publication remain separate actions.

## Known risks

- Browser-local history is subject to the browser's storage quota; the app surfaces failures and supports export/recovery.
- In-memory credentials are not an encrypted secret vault and must be re-entered after reload.
- Artifact previews intentionally block external resources; downloaded source remains faithful to the generated artifact.

## Codex presentation follow-up — 2026-10-25

### Scope and implementation

- Added a Codex-inspired presentation to the same package. The existing Claude presentation remains selectable, with no changes to the provider, daemon, monitor, desktop app or core persistence contract.
- The header menu and Appearance setting change views without reloading. Draft text, prepared attachments, the memory-only connection and an active response survive manual switches. A valid URL override is transient; legacy preferences default to Claude without losing history.
- Added coding prompt cards, a collapsible workspace history group and a real conversation context inspector with Overview/Files tabs, usage, model/mode, attachment/artifact metadata, export and API setup actions.
- The context inspector uses a desktop aside or a smaller-screen native dialog. The existing artifact panel remains available. Both themes consume the central token source.

### Validation executed

- Windows / Node 22.22.2: `node --test` — **67 passed, 0 failed, 0 skipped**. The additional tests cover URL/persisted interface precedence, genuinely old settings without the new field, corrupt-value preservation and context derivation/deduplication.
- `node node_modules/vite/bin/vite.js build` — successful production bundle, no Svelte compilation warnings.
- `npm audit --audit-level=moderate` — **0 vulnerabilities**. The test runner emits Node's experimental MockTimers warning; the browser console and Svelte build have no corresponding warning.
- Contrast tests now cover **60 semantic text/background pairs**, each at least 4.5:1, across Claude/Codex light/dark palettes.
- Isolated development-only QA server on port 5184 preserved the user's production origin/history on 5182. The fixture key was not a real credential and no live model was called.
- Browser: validated configuration; changed model/thinking; switched views with a draft and attachment; switched during incremental streaming and verified completed output; retried, stopped, edited and confirmed resend; generated and downloaded an HTML artifact; reopened it from Files; exported Markdown from Context; renamed, duplicated, deleted/undid and searched message content; collapsed/expanded workspace history; changed appearance through Settings; started a coding example prompt.
- Direct `?view=claude` displayed Claude without replacing the saved Codex preference. Returning to the bare URL restored Codex, the dark theme and selected history. Reload correctly cleared the memory-only API key.
- Desktop 1440px, tablet 1024px, and mobile 390px/320px checks showed no page-level horizontal overflow. At 320px the populated header and interface menu stayed within the viewport. Tablet artifact/context access, dialog tab-arrow/Escape handling, focus entry and the mobile sidebar backdrop/close affordance were exercised.
- Captured browser errors/warnings: **zero** during the controlled walkthrough. Independent scoped source review passed after fixing the tablet context-toggle selector, narrow menu placement and the artifact-library build prompt.
- The production build was opened on port 5182 at 1280px and 1440px. The Codex welcome/context view rendered without QA fixture content or captured browser errors/warnings. Temporary implementation/documentation worktrees were removed through Git after integration, with their branches retained.

### Compatibility, security and remaining limits

- No new package dependencies or storage namespace; old browser history remains readable. Presentation preferences are validated before use.
- Codex styling uses the current Anthropic API service. It does not imply Codex harness execution, repository access, Git diffs or an OpenAI provider connection.
- Live provider access, account limits, hosted CI, Linux/WSL, other browser engines and publication remain unverified or separate gates as documented above.

## Integration verification - 2026-10-29

The complete Claude/Codex source was copied from the existing design worktree
into `codex/ui_publish_20261029`, based on remote main `5c11231a`. All eight
previously untracked Codex follow-up files are included. Existing design and
backend worktrees are preserved. The unpublished browser ADR was renumbered to
056 to avoid an independently owned worker-tools ADR. Earlier sections above
remain historical observations from 2026-10-25.

Fresh checks on native Windows with Node 24.19.0 and npm 11.16.0:

- `npm ci --no-audit --no-fund --fetch-retries=0 --fetch-timeout=30000` installed
  the locked package dependencies.
- `npm test`: 67 passed, zero failures, cancellations or skips.
- `npm run build`: successful production output with Svelte 5.57.2 / Vite 8.3.3;
  Vite reported plugin timing information, without compilation warnings.
- `npm audit --audit-level=moderate`: exit 0, two low-severity development-tool
  advisories through deepmerge and the Svelte Vite plugin; no fix available.
  This supersedes the earlier zero-advisory observation for the current date.
- Exact development fixture markers are absent from `dist`; `vite build --mode qa`
  was rejected by the development-only guard as required.
- `git diff --cached --check`: passed; the public-artifact gate checked 685
  publishable text files with zero findings and 18 exact reviewed fixtures.
- The production preview binds only 127.0.0.1:5182 and returns HTTP 200 for the
  Codex view. The compiled HTML references the built JavaScript and CSS.
- Current independent source review found no missing-import, secret-persistence,
  production-fixture or core-state-boundary blocker after the documentation
  correction below.

### Correction to the earlier security wording

The earlier references to requests using one fixed origin apply to **provider
API requests**. User-activated Markdown links are ordinary links. HTML/SVG
artifacts use an opaque-origin iframe sandbox and a resource-restricting CSP;
inline scripts may run. This does not guarantee complete network isolation:
frame self-navigation is not fenced. The parent origin, stored chat history and
memory-only key are not exposed through a preview bridge. Source inspection
supports these boundaries; a navigation request was not freshly observed in a
browser. The [HTML sandbox navigation definition](https://html.spec.whatwg.org/multipage/browsers.html#sandboxed-navigation-browsing-context-flag)
and [CSP navigation directives](https://www.w3.org/TR/CSP3/#directives-navigation)
explain the distinction. The guide and accepted ADR state this narrower contract.

### Current verification limits

Browser automation timed out during startup. No fresh visual interaction,
browser-console, responsive-layout or preview-navigation claim is made for this
integration. The earlier dated browser evidence is retained above. No real API
key or live inference was used. Linux/WSL and hosted CI remain separate from the
local checks; Git source publication is separate from a hosted app deployment.
