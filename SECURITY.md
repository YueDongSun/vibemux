# Security

VibeMux worktrees provide concurrent-edit isolation only; they are not security sandboxes. Core security boundaries include `shell=False` with structured argv, stdin-based message delivery, `commonpath` path validation, fail-closed symlink/reparse handling, pane-ownership validation, append-only events, and dry-run cleanup. Do not publicly disclose credentials or exploit payloads when reporting an issue; contact the maintainer first.

