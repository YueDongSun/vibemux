#![forbid(unsafe_code)]
//! Per-harness stub transcripts used to seed the harness panel
//! terminal area. Each harness gets ~40 lines that mimic the look of
//! its real native UI. No real PTY, no real shell — just visible
//! stub content so the GUI has something to display.

use vibemux_probe::AgentKind;

/// Pre-rendered stub transcripts, one entry per `AgentKind`. The order
/// in this table must match `AgentKind::all()` so the harness panel
/// can index directly into it.
pub const BANK: &[(AgentKind, &[&str])] = &[
    (
        AgentKind::Claude,
        &[
            "Claude Code v1.2.3 (stub)",
            "  > thinking...",
            "    considering 3 options",
            "  > ready",
            "Claude> ",
            "(stub transcript; ~40 lines; backend not wired)",
            "  history:",
            "    - hello world",
            "    - explain this codebase",
            "    - add tests for X",
            "  tokens: 0 in / 0 out (stub)",
            "  model: claude-sonnet (stub)",
            "  status: idle",
            "Claude> ",
            "...",
            "(no live PTY attach in this slice)",
            "  keyboard:",
            "    ctrl-c   interrupt",
            "    ctrl-d   eof",
            "    enter    submit",
            "  settings:",
            "    max_tokens=4096",
            "    temperature=0.7",
            "Claude> ",
            "ready",
            "  accepted: 0 turns",
            "  pending:  0 turns",
            "  context_window: 200000 (stub)",
            "...",
            "(stub terminal area; bounded redraws)",
            "Claude> ",
            "(end of stub transcript)",
            "  last_seen_at: 1970-01-01T00:00:00Z (stub)",
            "Claude> ",
            "  no backend running (stub)",
            "  click Send to append a line",
            "...",
            "Claude> ",
            "(stub)",
            "(stub)",
            "(stub)",
            "(end)",
        ],
    ),
    (
        AgentKind::Codex,
        &[
            "codex> (stub)",
            "  model: gpt-5",
            "  context: 0",
            "codex> ",
            "(stub transcript; ~40 lines; backend not wired)",
            "  history:",
            "    > 1+1",
            "    2",
            "    > reverse 'hello'",
            "    'olleh'",
            "  tokens: 0 in / 0 out (stub)",
            "  model: gpt-5 (stub)",
            "  status: idle",
            "codex> ",
            "...",
            "(no live PTY attach in this slice)",
            "  keyboard:",
            "    ctrl-c   interrupt",
            "    ctrl-d   eof",
            "    enter    submit",
            "  settings:",
            "    sandbox=workspace-write",
            "    approval=never",
            "codex> ",
            "ready",
            "  accepted: 0 turns",
            "  pending:  0 turns",
            "  context_window: 200000 (stub)",
            "...",
            "(stub terminal area; bounded redraws)",
            "codex> ",
            "(end of stub transcript)",
            "  last_seen_at: 1970-01-01T00:00:00Z (stub)",
            "codex> ",
            "  no backend running (stub)",
            "  click Send to append a line",
            "...",
            "codex> ",
            "(stub)",
            "(stub)",
            "(end)",
        ],
    ),
    (
        AgentKind::OpenCode,
        &[
            "OpenCode 1.18.21 (stub)",
            "[boot] providers: 1",
            "ready>",
            "(stub transcript; ~40 lines; backend not wired)",
            "  history:",
            "    [ok] project scan",
            "    [ok] provider init",
            "    [ok] model load",
            "  tokens: 0 in / 0 out (stub)",
            "  model: opencode-1.18 (stub)",
            "  status: idle",
            "ready> ",
            "...",
            "(no live PTY attach in this slice)",
            "  keyboard:",
            "    ctrl-c   interrupt",
            "    ctrl-d   eof",
            "    enter    submit",
            "  settings:",
            "    theme=dark",
            "    telemetry=off",
            "ready> ",
            "ready",
            "  accepted: 0 turns",
            "  pending:  0 turns",
            "  context_window: 128000 (stub)",
            "...",
            "(stub terminal area; bounded redraws)",
            "ready> ",
            "(end of stub transcript)",
            "  last_seen_at: 1970-01-01T00:00:00Z (stub)",
            "ready> ",
            "  no backend running (stub)",
            "  click Send to append a line",
            "...",
            "ready> ",
            "(stub)",
            "(stub)",
            "(end)",
        ],
    ),
    (
        AgentKind::Copilot,
        &[
            "gh copilot (stub)",
            "explain shell>",
            "suggest>",
            "(stub transcript; ~40 lines; backend not wired)",
            "  history:",
            "    > list files",
            "    - src/main.rs",
            "    - src/lib.rs",
            "  tokens: 0 in / 0 out (stub)",
            "  model: gh-copilot (stub)",
            "  status: idle",
            "explain shell> ",
            "...",
            "(no live PTY attach in this slice)",
            "  keyboard:",
            "    ctrl-c   interrupt",
            "    ctrl-d   eof",
            "    enter    submit",
            "  settings:",
            "    scope=repo",
            "    allow=*",
            "explain shell> ",
            "ready",
            "  accepted: 0 turns",
            "  pending:  0 turns",
            "  context_window: 32000 (stub)",
            "...",
            "(stub terminal area; bounded redraws)",
            "explain shell> ",
            "(end of stub transcript)",
            "  last_seen_at: 1970-01-01T00:00:00Z (stub)",
            "explain shell> ",
            "  no backend running (stub)",
            "  click Send to append a line",
            "...",
            "explain shell> ",
            "(stub)",
            "(stub)",
            "(end)",
        ],
    ),
    (
        AgentKind::Grok,
        &[
            "grok> (stub)",
            "  ask anything.",
            ">",
            "(stub transcript; ~40 lines; backend not wired)",
            "  history:",
            "    > ping",
            "    pong",
            "    > what is rust",
            "    a systems programming language",
            "  tokens: 0 in / 0 out (stub)",
            "  model: grok-2 (stub)",
            "  status: idle",
            "grok> ",
            "...",
            "(no live PTY attach in this slice)",
            "  keyboard:",
            "    ctrl-c   interrupt",
            "    ctrl-d   eof",
            "    enter    submit",
            "  settings:",
            "    temperature=0.7",
            "    top_p=0.9",
            "grok> ",
            "ready",
            "  accepted: 0 turns",
            "  pending:  0 turns",
            "  context_window: 128000 (stub)",
            "...",
            "(stub terminal area; bounded redraws)",
            "grok> ",
            "(end of stub transcript)",
            "  last_seen_at: 1970-01-01T00:00:00Z (stub)",
            "grok> ",
            "  no backend running (stub)",
            "  click Send to append a line",
            "...",
            "grok> ",
            "(stub)",
            "(stub)",
            "(end)",
        ],
    ),
];

/// Get the stub transcript for a given harness as a single `String`
/// with each line separated by `\n`. Used at app construction to
/// pre-seed the terminal area.
#[must_use]
pub fn stub_lines(agent: AgentKind) -> String {
    for (candidate, lines) in BANK {
        if *candidate == agent {
            return lines.join("\n");
        }
    }
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn harness_bank_is_non_empty_for_each_agent_kind() {
        for agent in AgentKind::all() {
            assert!(
                stub_lines(agent).len() >= 30,
                "harness {agent:?} has fewer than 30 chars"
            );
        }
    }

    #[test]
    fn harness_bank_is_ascii() {
        for (_, lines) in BANK {
            for line in *lines {
                for byte in line.bytes() {
                    assert!(byte <= 0x7E, "non-ASCII byte 0x{byte:02x} in {line:?}");
                }
            }
        }
    }

    #[test]
    fn every_agent_kind_is_covered() {
        let mut covered = std::collections::BTreeSet::new();
        for (agent, _) in BANK {
            covered.insert(*agent);
        }
        for agent in AgentKind::all() {
            assert!(covered.contains(&agent), "missing agent {agent:?}");
        }
    }
}
