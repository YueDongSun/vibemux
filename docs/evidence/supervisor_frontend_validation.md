# Supervisor frontend validation

Date: 2026-09-27. Platform: native Windows, Rust 1.85/MSVC.

## Scope

The Supervisor Chat frontend, Studio appearance, task windows, Control v4 reads,
plugin request routing and explicit WezTerm observation are implemented. The
normal GUI consumes daemon state through `LiveApp`/`FrontendClient`; the preview
binary is a separate, feature-gated renderer with visible DEMO labels.

The first implementation's repeated dark cards was rejected during review.
The delivered revision uses a light Studio canvas, restrained green accents,
Segoe UI with CJK fallback, a grouped activity list and project/task navigation.
Existing dark palette IDs are retained, and existing saved preferences are not
overwritten. New profiles default to Studio.

## Checks

- Final complete workspace: **340 passed, 0 failed, 2 ignored**, 63 test groups,
  using `cargo test --workspace --all-features -j 2 --target-dir target/supervisor_delivery -- --test-threads=1`
  with the three profile variables recorded below. Ignored cases are the
  pre-existing golden generator and child-process fixture entry point, not new exclusions.
- `cargo fmt --all -- --check` and `cargo clippy --workspace --all-targets --all-features -- -D warnings`.
- Final runtime build: `cargo build -p vibemux_frontend --bin vibemux_frontend -p vibemux_terminal_observer --bin vibemux_terminal_observer -p vibemuxd --bin vibemuxd --target-dir target/supervisor_delivery -j 2`
  passed with dev debug symbols and incremental compilation disabled. The normal
  frontend build does not require the screenshot feature.
- Frontend library: 57 passed, 1 ignored fixture generator. Includes 54
  theme/size/scale combinations, all ten harness shortcuts, Escape precedence,
  IME preedit isolation, bounded queues, configuration debounce and a real
  temporary-daemon read/disconnect integration.
- Daemon library: 52 passed before the final UI-only revision. Terminal
  observation integration: 3 passed, including request correlation, permission
  refusal, timeout, identity mismatch, absent plugin, unlink and unchanged Run
  state. Plugin supervisor process contract tests: 13 passed. Registry lifecycle
  integration tests: 9 passed.
- `python -m pytest`: 61 passed; `python -m ruff check .` and
  `python -m ruff format --check .`: passed; `python -m mypy src/vibemux`:
  passed for 17 files, using the project interpreter and active `src` path.
- `python -m mypy .` remains blocked by optional A2A/grpc/httpx/uvicorn/Starlette
  type dependencies and an existing typing error in `tests/test_harness.py`.
- `cargo nextest`, `cargo deny` and `cargo audit` are unavailable on this host.

## Render evidence

Actual native egui frames were captured after layout settled, including a
separate task viewport. These are synthetic fixtures, not proof of model work.

- [Main conversation](../frontend_previews/supervisor_chat.png)
- [Docked task detail](../frontend_previews/supervisor_drawer.png)
- [Independent task window](../frontend_previews/supervisor_task.png)
- [Native terminal observation view](../frontend_previews/supervisor_task_terminal.png)
- [Disconnected state](../frontend_previews/supervisor_disconnected.png)

The six palettes were rendered at 960×600, 1280×800 and 1920×1080 logical sizes,
with 100%, 150% and 200% egui scaling (54 native captures). Representative
light/dark, narrow/wide and scaled frames were visually inspected. Scrollable
content remains scrollable at small sizes; retained screenshots are the final
deliverables, while matrix images and scratch configuration are removed.
This exercises renderer scaling, not a physical monitor DPI-transition test.

## Limits and remaining gates

- WezTerm was not found in the inspected executable/installation paths and no
  running WezTerm GUI was found. Live pane focus and native TUI observation are
  **not verified**. No terminal was installed or started for the user.
- Continuous coordinator chat, automatic coding-harness dispatch and embedded
  ConPTY rendering remain unavailable by design in this slice. The input draft
  is retained and Send clearly indicates this boundary.
- Native Windows rendering and tests do not establish Linux/WSL, real IME
  candidate-window behavior, hosted CI, remote A2A, or deployment acceptance.
- The original parallel full-suite attempt hit two pre-existing CLI recovery
  ACL tests while another test intentionally loosened the shared ACL marker.
  The failing starts used `ControlRuntimeSecurityInvalid`; source inspection
  confirmed the shared-marker test interference. The final serial run passed;
  security assertions were not weakened. Ordinary parallel full-suite readiness
  remains a known repository issue.
- A separate initial debug build exhausted disk space and failed with MSVC
  `LNK1318` / PDB `LIMIT (12)`. Task-owned duplicate build caches were removed;
  final workspace validation uses `CARGO_PROFILE_TEST_DEBUG=0`,
  `CARGO_PROFILE_DEV_DEBUG=0`, `CARGO_INCREMENTAL=0` and an isolated target dir.
