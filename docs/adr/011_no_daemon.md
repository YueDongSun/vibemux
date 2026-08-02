# ADR 011: No daemon in MVP

Context: local-first CLI should be inspectable and easy to install. Decision: services run in the CLI process and persist via SQLite. Consequences: no socket/process supervisor dependency. Alternative: always-on service, rejected.

