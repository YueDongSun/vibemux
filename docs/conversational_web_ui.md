# Conversational web UI

The optional browser chat workspace is in `crates/vibemux_chat/web`. It is separate from the authenticated, read-only `vibemux_web` monitor and the Tauri desktop workstream. Its conversations are browser-local records, not canonical VibeMux Tasks, Runs, or Events.

## Run locally

Use Node.js 22.22 or newer and the checked-in lockfile:

```powershell
cd crates/vibemux_chat/web
npm ci
npm run dev
```

Open the printed loopback URL, normally `http://127.0.0.1:5182`. `npm run build` creates the static `dist/` output, and `npm run preview` serves that build on the same loopback port. `npm test` runs controlled, network-free service tests.

## Choose a workspace view

The header's Switch interface menu offers a Claude-inspired conversational view and a Codex-inspired coding workspace. Settings → Appearance → Interface provides the same choice. A manual selection persists and changes the presentation without reloading the page, so the current draft, prepared attachments, API connection and active response remain available.

Open `/?view=codex` or `/?view=claude` to choose a view for that visit. A valid URL override takes precedence without changing the saved preference; a subsequent manual selection saves the choice. Both views use the same browser-local conversations, search, settings and artifacts. Existing history defaults to the Claude presentation until another view is selected.

The Codex presentation adds coding example prompts, a collapsible Personal workspace conversation group and a context inspector. Overview shows the conversation's recorded message count, token usage, model and thinking mode. Files lists sent attachments and the saved artifact; it can open the artifact, export the conversation or open API settings. The inspector appears alongside the stream on wide screens and in a keyboard-operable dialog on smaller screens. Opening an artifact uses the existing artifact panel.

The presentation choice does not change the Anthropic provider or add Codex harness execution, repository access, Git diffs or an OpenAI API integration.

## Connect and chat

Open Settings → API connection, enter an Anthropic API key, and choose Save & validate. Validation queries the provider's Models API without creating an inference request. The key is held only in the current page's memory and is cleared on reload. It is excluded from conversation persistence, exports, and logs. This is a single-user browser client; browser memory is not an encrypted credential store.

Sending explicitly transmits the conversation context and its selected attachments directly to `https://api.anthropic.com`. Access, billing, and rate limits belong to the configured API account. The unconfigured state gives setup guidance and retryable errors; production code has no simulated inference path.

The workspace supports persistent conversation titles and message-content search, time groups, pinning, rename, duplication, Markdown/JSON export, confirmed deletion and undo. The composer accepts up to five supported images, PDFs, or text/code documents, each up to 10 MB. Its progress reports local preparation; files are sent with the message. Responses stream incrementally and support cancellation, regeneration, user-message editing, copying, and local thumbs feedback.

Long fenced code and documents can open an artifact panel with code, supported preview, copy, download, resizing, and fullscreen controls. HTML/SVG previews use an opaque-origin sandbox without parent-origin access. A CSP restricts subresource loads, scripted fetches, and forms, while inline preview scripts may run. This is not a complete network sandbox: frame self-navigation is not fenced. Markdown raw HTML is escaped and rendered output is sanitized.

Settings persist theme, reading font size, density, Enter behavior, autoscroll, default model, and default thinking mode. Theme follows the system until manually changed. Long histories render a bounded page of messages with an explicit earlier-message control; scrolling up suspends autoscroll. Invalid browser records remain intact until the user downloads or explicitly resets them.

| Action                        | Shortcut                              |
| ----------------------------- | ------------------------------------- |
| New conversation              | Alt+N                                 |
| Focus composer                | Alt+/                                 |
| Toggle sidebar                | Alt+B                                 |
| Toggle theme                  | Alt+T                                 |
| Search                        | Alt+K                                 |
| Send                          | Ctrl/Cmd+Enter, or Enter when enabled |
| Stop response or close dialog | Escape                                |

## Authority and verification

The UI has no daemon credential, core database writer, tool execution, filesystem execution, or automatic Git action. Provider chat completion does not prove a VibeMux Run succeeded. This package adds no HTTP route or authentication exception to ADR 033 and does not implement the desktop multi-turn contract.

Build, service-test, visual, and interaction evidence must be reported separately. Controlled HTTP fixtures can verify frontend flows without credentials; they do not establish live provider access, a billed model response, daemon integration, desktop acceptance, or release readiness.
