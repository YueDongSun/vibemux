# ADR 016: Read-only probe service and native-TUI-preserving unified frontend

Status: Accepted for pre-alpha implementation

## Context

VibeMux currently diagnoses local agent availability, launcher compatibility, local gateway routing, CC Switch health, and A2A loopback behavior through ad hoc session commands. Those checks produced useful evidence, but they are not repeatable product behavior and cannot feed a stable UI.

The desired unified frontend must show several harnesses together while preserving each CLI's native TUI. Reimplementing Claude, Codex, OpenCode, Copilot, or Grok terminal interfaces inside VibeMux would couple the product to private rendering behavior and would collapse terminal presentation into harness control. Scraping PTY text would also violate the rule that terminal output is not authoritative structured state.

## Decision

Add two dependency-isolated Rust components:

1. `vibemux_probe` provides read-only, bounded diagnostics and a versioned JSON report.
   - It detects known agent launchers and versions through explicit argv with deadlines.
   - It reads only allowlisted, non-secret provider/base URL fields.
   - It classifies direct, loopback-gateway, unavailable, and unknown routes.
   - It probes CC Switch TCP/HTTP health and reads only allowlisted aggregate columns from its SQLite database in read-only mode.
   - It can run the safe local A2A information-share self-test without invoking a model.
   - It never performs inference smoke, login, provider switching, config writes, database writes, or process termination by default.

2. `vibemux_frontend` provides a backend-neutral dashboard model plus a Windows-compatible Ratatui shell.
   - Overview panels consume `ProbeReport` and future daemon control-API snapshots.
   - Every supported CLI receives a stable `NativeTuiSlot` containing identity, launcher capability, terminal-surface state, and future attach metadata.
   - A native CLI TUI remains an owned PTY/ConPTY/terminal-plugin surface. The dashboard may activate or attach that surface later, but does not parse, emulate, proxy, or reinterpret its screen contents.
   - The pre-alpha frontend has no direct core-database access and performs no authoritative transition.

The initial frontend is a shell and renderable view model. Real native-TUI attachment remains blocked on the terminal plugin and Windows ConPTY lifecycle gates.

## Consequences

- Probe results become reproducible JSON that CLI, TUI, tests, and support bundles can share.
- Gateway failures can be distinguished from launcher, authentication, and model failures without capturing prompt bodies.
- Native agent interfaces remain upgradeable independently of the VibeMux dashboard.
- The first frontend can be tested with an in-memory terminal backend and a `--once` snapshot mode before interactive process attachment is safe.
- Ratatui/crossterm and probe parsing add dependencies and Windows terminal behavior that require dedicated CI.
- Full TUI embedding, focus routing, resize propagation, clipboard, approval prompts, and process teardown remain future terminal-plugin work.

## Alternatives

- Keep probes as operator scripts: rejected because results are not versioned, testable, or consumable by the frontend.
- Transparently intercept all HTTPS agent traffic: rejected because certificate injection, credential exposure, and streaming breakage outweigh the diagnostic benefit.
- Store prompt and response bodies in the probe database: rejected because metadata is sufficient for route/error attribution and default content capture violates logging policy.
- Reimplement each agent's TUI in React/Ratatui: rejected because it duplicates vendor UI behavior and encourages terminal-text state inference.
- Embed unrestricted shell commands in frontend panels: rejected because user or remote text must never become a shell command string.
- Build a browser-only frontend first: deferred because native CLI TUIs require a supervised terminal surface that a browser page alone does not provide.

## Amendment (2026-09-12): dashboard themes and color-independence

Decision: the renderer exposes classic, high-contrast, and mono themes selected by `--theme` or `VIBEMUX_FRONTEND_THEME` (classic is default). Consequences: no foreground colors in mono (emphasis via bold/dim only); state information is always carried by full-word text labels (`verified`/`failed`/`unavailable`/`not_run`), so color is an enhancement rather than the only channel and the dashboard stays usable under deuteranopia/protanopia and on colorless terminals (WCAG 1.4.1 Use of Color). Golden render fixtures per theme guard visual drift. Alternatives: symbol prefixes on every cell (noisier, rejected for now).
