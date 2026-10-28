# Documentation Map

- [Conversational web UI](conversational_web_ui.md), [ADR 056](adr/056_browser_local_chat.md), and [validation](conversational_web_ui_validation.md): optional browser-local Claude/Codex views and local startup.

- [Unified CLI guide](guides/unified_cli_guide.md): common commands, output, and dashboard.
- [Harness core guide](guides/harness_core_guide.md): bounded native requests and capture.
- [Branch workflow](workflow/branch_workflow.md): branch roles, worktree layout, and Git boundaries.
- [Architecture](architecture.md): ownership and dependency direction.
- [A2A implementation path](guides/a2a_implementation_learning_path.md): the next domain binding.
- [ADR 040](adr/040_native_harness_request_dispatch.md): standalone native harness dispatch (ADR 029 is the daemon-owned path).
- [ADR 041](adr/041_unified_cli_and_workspace_layout.md): common CLI and layout decisions.
- [Desktop app design](desktop_app_design.md), [ADR 043](adr/043_tauri_desktop_app.md) (proposed), and the [phase 1 plan](desktop_app_phase1_plan.md): Tauri shell first, harness chat second; not implemented.
- [Workspace and CLI validation, 2026-08-28](reports/workspace_cli_validation_20260828.md): current local checks and remaining gates.
- [Branch snapshot, 2026-08-27](reports/branch_inventory_20260827.md): dated ref evidence.
- [Harness validation, 2026-08-27](reports/harness_validation_20260827.md): dated test and live limits.

Guides describe repeatable use. Reports are historical snapshots and must not
silently replace current verification. `PROGRESS.md` remains the implementation
status entrypoint; ADRs retain decisions and alternatives.
