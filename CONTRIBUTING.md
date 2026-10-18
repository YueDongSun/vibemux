# Contributing

1. Create a local `.venv` and run `python -m pip install -e ".[dev]"`.
2. Run `python -m ruff format --check .`, `python -m ruff check .`, `python -m mypy .`, `python -m pytest`, and `python scripts/smoke_test.py`.
3. Use Conventional Commits; keep each commit to a single concern.
4. Never commit `.env`, credentials, `.vibemux/`, or user-machine absolute paths.
5. Backend behavior should also cover the Mock contract and run tmux/WezTerm contract tests on platforms where they are available.
6. For Rust changes, run `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, and `cargo test --workspace --all-features`.
7. For DeepSeek Harness (DSH) work, run `pwsh -File scripts/setup_dsh_prereq.ps1` first: it reports whether this machine has the supported `@deepseek-ai/dsh` runtime, the `sdk` profile and the optional interactive `tui` profile. Add `-Apply` to provision what is missing, and `-SkipTui` to check only the `sdk` profile the VibeMux adapter drives.
