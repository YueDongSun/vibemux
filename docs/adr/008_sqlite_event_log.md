# ADR 008: SQLite append-only event log

Context: MVP is local-first and no daemon is needed. Decision: SQLite stores state and ordered events in transactions. Consequences: simple install and audit trail. Alternative: network database, rejected.

