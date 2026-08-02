# ADR 002: TerminalBackend abstraction

Context: pane control differs by terminal. Decision: protocol exposes open/list/send/stop/activate and generic TerminalLocation. Consequences: domain avoids tmux fields. Alternative: provider mega-class, rejected.

