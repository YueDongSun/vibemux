# Contributing

1. Create a local `.venv` and run `python -m pip install -e ".[dev]"`.
2. Run `python -m ruff format --check .`, `python -m ruff check .`, `python -m mypy .`, `python -m pytest`, and `python scripts/smoke_test.py`.
3. Use Conventional Commits; keep each commit to a single concern.
4. Never commit `.env`, credentials, `.vibemux/`, or user-machine absolute paths.
5. Backend behavior should also cover the Mock contract and run tmux/WezTerm contract tests on platforms where they are available.
6. For Rust changes, run `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, and `cargo test --workspace --all-features`.
