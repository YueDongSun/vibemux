# ADR 036: Interactive Claude TUI inside the GUI

Status: Accepted for the user's explicit interactive-terminal request.

## Decision and authority

Add a first-party, out-of-process native terminal host and an embedded GUI
terminal page. This amends ADR 016's no-screen-emulation restriction and ADR
028's external-terminal-only presentation. The generic Alacritty VT engine
interprets terminal escape sequences; VibeMux does not reproduce Claude's UI.
PTY screen content is presentation data and never proves Task/Run completion.

Like the standalone DSH surface in ADR 032, this is an explicit manual session,
not a second authoritative Task/Run writer. The Rust terminal service creates
an independent Git worktree and branch before starting writable Claude. It
retains them on exit for review. No commit, merge, push, stash, reset, or cleanup
is automatic. Existing daemon tasks and external terminal bindings are separate.

The GUI is a thin input/display client. A contained first-party host owns the
vendor process, worktree receipt, PTY, bounded terminal model, and teardown.
Its private length-framed Protobuf protocol is versioned independently of
public plugin v1 and Control IPC. It is not a third-party plugin interface.
The host starts nothing before a validated Open command. The UI starts a
session only after an explicit Start action; reopening the page does not launch.

Keyboard input and paste travel as bytes through owned pipes and PTY input;
they never enter a shell command string or launch argv. Native Claude handles
its own trust and tool-permission prompts. VibeMux never answers them or enables
permission bypass. The process tree is contained before the native CLI starts.
The terminal host and vendor are separate processes; neither opens core SQLite.

## Resource and privacy contract

- One session per GUI terminal controller; no restart loop or duplicate Start.
- Absolute fixed adjacent host path from the trusted GUI build/bundle; absolute
  vendor/Git executable paths outside the selected project. The host starts in
  its bundle directory, outside the writable session worktree. No operator
  shell fragments; the vendor is `claude`.
- Project and workspace paths are canonical and reject symlink/reparse escapes.
  Git hooks are disabled for the worktree-creation operation. Git's root is
  resolved explicitly; `.vibemux` must be ignored before creating runtime data.
  Canonical Windows path prefixes are normalized only at the Git argv boundary.
- Private session receipts record identity, base commit, branch, and worktree;
  terminal bytes, prompts, and responses are retained in bounded memory only.
- Bounded frames, dimensions, text, input/output queues, and scrollback. Overflow
  fails the session visibly. Errors are fixed codes with no raw command output.
  OSC payloads are capped at 1,024 bytes and combining marks at eight per cell,
  before the VT engine can accumulate an unbounded string across input chunks.
- Explicit input focus; GUI shortcuts do not intercept terminal-owned keys.
  Resize is propagated to the PTY and VT model. Clipboard writes and external
  URL opening requested by escape sequences are denied.
- Stop/app exit terminates only the contained host/vendor tree, reaps children,
  and joins owned I/O workers. Worktree and branch are retained.
- Environment inheritance is allowlisted for the native CLI's existing login
  and local provider setup; VibeMux does not read or copy credential files.
  Existing `ANTHROPIC_API_KEY`/`ANTHROPIC_AUTH_TOKEN` values, when present, are
  inherited privately by the child and are never written to receipts or logs.
  This is the same trusted-user boundary as a manually launched CLI, not an OS
  sandbox. Worktree isolation is separate from process containment.

## Dependencies and compatibility

Pin `portable-pty` 0.9.0 (MIT) for Windows ConPTY/POSIX PTY and
`alacritty_terminal` 0.25.0 (Apache-2.0, optional serde disabled) for mature VT,
alternate screen, Unicode, terminal modes, and query responses. Reuse existing
Prost, vendored protoc, process containment, UUID, and UI dependencies.
The VT engine has no egui dependency, preserving egui 0.32.3 and existing UI
contracts. Additional build/transitive cost belongs to this optional surface.
`home` 0.5.11 provides the native-user executable fallback within the declared
MSRV range. Kitty keyboard negotiation is disabled until that input protocol
is implemented. Mouse reporting, text selection/copy, advanced modifier keys,
and physical IME/accessibility certification are outside this initial slice.

No database or existing IPC migration. The host and GUI binaries must be built
together for private terminal protocol v1. Removing the surface is a local
binary rollback; retained worktrees require separately reviewed cleanup.

## Verification

Exercise the real host/PTY with a fixture terminal: input, ANSI/color/cursor,
resize, alternate screen, Unicode, focus, malformed/oversized frames, invalid
paths, queue pressure, early process exit, and owned-tree cleanup. Validate
keyboard/paste encoding and GUI rendering independently. A live installed
Claude initialization may show its native trust/login screen; it does not
authorize inference or answering a trust/permission prompt. Bind evidence to
the actual process, source, and screenshot. Fixture evidence is not live-Claude
evidence. Linux and physical IME/accessibility remain explicit coverage gates.
