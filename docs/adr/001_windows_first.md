# ADR 001: Windows-first cross-platform core

Context: product runs from PowerShell on Windows. Decision: keep core Python/SQLite/Git abstractions platform-neutral and test Windows in CI. Consequences: WSL is optional; POSIX helpers stay behind adapters. Alternative: WSL-first, rejected.

