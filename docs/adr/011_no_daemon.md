# ADR 011: No daemon in MVP

Status: Superseded by [ADR 015](015_daemon_single_writer.md).

Context: local-first CLI should be inspectable and easy to install. Decision: services run in the CLI process and persist via SQLite. Consequences: no socket/process supervisor dependency. Alternative: always-on service, rejected.
