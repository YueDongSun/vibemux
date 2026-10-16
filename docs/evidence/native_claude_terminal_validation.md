# Interactive Claude terminal validation

> Historical validation snapshot: 2026-10-14. References to local/uncommitted
> state describe that snapshot. Feature-branch publication is verified separately.

## Scope

- Task: display and interact with Claude Code's actual native TUI inside the
  VibeMux GUI, preserving the earlier English/Chinese and compact UI work.
- Owned modules: `vibemux_terminal`, frontend terminal presentation/navigation,
  dependency declarations, and the related architecture/user documentation.
- Baseline: GitHub-derived `9e738f12c6dae4d97a5a567a0d261427c4c6fb86`;
  local branch `codex/claude_tui_gui`. Changes are uncommitted.
- Pre-existing A2A documents and the user's progress entry are preserved.

## Implemented

- Dedicated **Claude terminal** page with explicit Start/Stop, terminal focus,
  12 px monospace cells, native trust/permission presentation, and EN/CN labels.
- First-party process host with real Windows ConPTY/POSIX PTY, a bounded
  Alacritty VT model, and private length-framed Protobuf v1 pipes.
- Focused text/control keys, arrows, Enter, Tab, Escape, bracketed paste,
  Unicode commits, scrollback, and resize forwarding. No reconstructed Claude
  transcript or model-generated terminal mockup.
- Each writable Start creates a fresh branch/worktree at committed HEAD and
  a content-free ownership receipt. Stop retains Git resources for review.
- Terminal lifecycle polling continues when another GUI page is selected.
- Screenshot scenes disable preference persistence, including live terminal
  captures with a non-demo task snapshot.

## Behavior and invariants

- Opening the page does not start Claude. Duplicate Start is blocked; pending
  startup can be stopped. Closed clients discard queued GUI input.
- Only the owned process tree is stopped. Native Claude handles permission
  prompts; VibeMux never accepts them or enables a permission bypass.
- The terminal is a manual session, separate from daemon Task/Run ownership
  and existing WezTerm observation leases. No SQLite access or canonical state
  transition occurs. Terminal text/exit never establishes task success.
- Git operations use executable plus argv, disable hooks, resolve the actual
  repository root, require ignored runtime paths, and preserve dirty main files.
  Windows canonical paths are normalized at the Git argv boundary.
- Input, frames, screen dimensions, scrollback, and queues are bounded. Fixed
  error codes distinguish launch, protocol, size, and resource failures.

## Files changed

- Root `Cargo.toml`/`Cargo.lock` and frontend dependency declaration.
- `crates/vibemux_terminal/`: host/client, VT model, workspace allocation,
  private schema/framing, actual PTY fixture, and boundary/integration tests.
- Frontend `gui/native_terminal.rs`, sidebar, supervisor state/app, module
  registration, localization catalog, and explicit screenshot scenes.
- README, CHANGELOG, PROGRESS, architecture, ADR 016/028 amendments, ADR 036,
  and this verification record. Earlier UI evidence remains historical.

## Validation executed

Native Windows NT 10.0 build 26300, process-local Rust
`1.95-x86_64-pc-windows-msvc`; the repository toolchain pin is unchanged.
Commands run from the repository root:

```text
cargo test -p vibemux_terminal -p vibemux_frontend --all-features --offline -j 2
cargo clippy -p vibemux_terminal --all-targets --all-features --no-deps --offline -j 2 -- -D warnings
cargo clippy -p vibemux_frontend --all-targets --all-features --no-deps --offline -j 2 -- -D warnings
cargo clippy --workspace --all-targets --all-features --offline -j 2 -- -D warnings
cargo test --workspace --all-features --offline -j 2
cargo fmt --all -- --check
git diff --check
```

- Final combined targeted run: **157 passed, 0 failed, 1 ignored** (146 frontend,
  11 terminal). The ignored case is the existing frontend golden generator.
  This run includes the final parser caps and preview-persistence safeguard.
  Log: `target/claude_terminal_delivery_tests.log`.
- Strict terminal lint passed. The final GUI/host/fixture build passed. Final
  formatting and whitespace checks passed. No release lint is suppressed.
- The broader workspace run completed **956 passed, 1 failed, 2 ignored** before
  Cargo stopped. The unchanged `workflow_optimizer.rs:204` asserted that the
  candidate holdout's `verified_success` was true; it was false. Rollback was
  not reached. The daemon/workflow/store source diff is empty. The root cause
  was not established or fixed in this terminal task; no blind retry was used.
  This run predates the final VT caps; the final targeted run above validates
  those changes. Log: `target/claude_terminal_workspace_tests.log`.
- Terminal tests use actual PTY processes. They verify literal input, resize,
  independent live sessions, owned-session stop, early fixture exit, a real
  isolated worktree, dirty-main preservation, and rejection of unignored state.
- Protocol/VT tests cover alternate screen, colors, Unicode, cursor queries,
  oversized/truncated frames, version/input limits, clipboard denial, and
  bounded query pressure. A deterministic fuzz test exercises 1,024 malformed
  wire/terminal cases and screen validation after a narrow resize. Separate
  regressions reject unfinished OSC strings and excess combining marks across
  input chunks before unbounded engine accumulation.
- GUI tests verify opt-in navigation and that terminal focus keeps Ctrl+K and
  Escape in the terminal; key/paste/IME encoding is checked separately.
- Strict frontend lint finds the two existing diagnostics in
  `dsh_tui/mod.rs:251` and `theme/mod.rs:81`; they are not suppressed or changed.
- Strict workspace lint stops at the existing `manual_div_ceil` diagnostic in
  `vibemux_platform/src/windows_acl_native.rs:140`.
- `cargo nextest --version`, `cargo deny --version`, and `cargo audit --version`
  report that those commands are not installed. Their release gates did not run.
- The first real worktree test found Git for Windows rejecting extended path
  argv. An isolated reproduction confirmed it; the fixed real-Git test passed.
  Original failure logs remain in ignored `target/` evidence.
- Real Claude initialization was captured in the GUI at 1280x800. It displayed
  Claude's native folder-trust dialog, which was left unanswered. No input or
  model prompt was submitted. The observed capture child processes exited.
- Final public-safe fixture captures cover EN/CN chrome. The real Claude image
  and complete process receipts remain private under ignored `target/`.
  [Source/build/image hashes](native_claude_terminal_source_manifest.json).

## Platform coverage

- Windows: native GUI, real PTY fixture, real Git worktree operations, and live
  installed Claude initialization were exercised.
- Linux/WSL: not run; POSIX code is not a verified platform claim.
- Live terminal backend: embedded ConPTY verified as above. Existing external
  WezTerm observation was not exercised by this change.
- Physical IME, accessibility, DPI transitions, and live inference: unverified.

## Compatibility / migration

- No database, canonical event, existing Control IPC, or public plugin migration.
- Private terminal protocol v1 and receipt schema v1 are new first-party formats;
  GUI and host must be built/deployed together as adjacent binaries.
- Existing GUI configuration schema and theme/language preferences remain valid.
- Build: `cargo build -p vibemux_frontend --bin vibemux_frontend -p vibemux_terminal --bin vibemux_terminal_host`.
- Rollback: use the previous local GUI/host bundle. Retained worktrees require
  separately reviewed cleanup; no Git resource is automatically removed.

## Security impact

- The contained trampoline waits until process ownership is established before
  launching Claude. Commands remain fixed argv; user text travels as PTY bytes.
- Child environment is allowlisted, including existing Anthropic authentication
  variables for native CLI use. Values are never copied to logs or receipts.
- OSC clipboard access and external URL opening are denied. Terminal contents
  stay in bounded memory. This is a trusted-user process boundary, not a sandbox.
- Live screenshots and private process receipts stay in Git-ignored `target/`
  because native output can include machine paths or account information.

## Remaining work

- Hosted CI, Rust 1.85/MSRV validation, Linux/WSL, live model/tool execution,
  and the full release security/dependency gates remain separate evidence.
- Existing-pane attach, other vendor terminals, mouse reporting, text
  selection/copy, and advanced keyboard protocols are outside this slice.
- No commit, remote publication, deployment, or global configuration change.

## Known risks

- Terminal compatibility is an initial subset; native permission prompts are
  still required, and retained worktrees consume disk space.
- Fonts depend on installed system faces. Physical IME behavior needs a human
  test even though composition/commit routing has automated coverage.
- The full-workspace test failure, existing strict lint findings, and unavailable
  release tools prevent a clean workspace release claim.
- Readiness is conditional pre-alpha: fixture interaction plus live Claude
  initialization; it does not certify inference, tools, or production use.
