# Harness Core Validation Record

Date: 2026-08-27. Platform: native Windows, x86_64 MSVC. Source baseline:
`6498e6aa711f751d9f9e50e37034d61c17d4b341`, plus the uncommitted
`codex/harness_request_dispatch` slice. This is local validation, not a release
or deployment certificate.

## Automated Results

- `cargo test --offline --locked --workspace --all-features`: **140 passed,
  zero failed**, across 41 test targets including empty binary/doc-test targets.
- The new crate contributes **37 tests**: 18 normalization/framing/boundary tests
  and 19 native process/dispatch tests.
- `cargo clippy --offline --locked --workspace --all-targets --all-features -- -D warnings`:
  passed on the final source, including test helpers and the optional example.
- `cargo fmt --all -- --check`: passed.
- Python 3.12 project-interpreter regression: **30 passed**. `PYTHONPATH` selected
  the isolated worktree's source, not the old checkout's editable installation.
- `ruff check .`: passed. The existing Python `scripts/smoke_test.py` completed
  its mock workflow successfully.
- `git diff --check`: passed. New filenames contain no spaces or hyphens.
- The public example catalog and documented Codex app-server fixture replay
  were executed successfully; replay captured all ten fixture records.

Rust test runtime metadata and temporary files were isolated under an ignored
validation directory. Per-command `LOCALAPPDATA`, `TEMP`, and `TMP` overrides
did not change global defaults or deployed service state.

## Behavioral Evidence

The synthetic process tests use actual child executables and the existing
Protobuf supervisor, not a mocked function call. They cover all five harness
routes, complete prompt/unknown-field retention, permission denial, terminal
correlation, config-digest drift, duplicate IDs, shared-workspace admission,
malformed and oversized records, stderr draining, missing terminal evidence,
blocked stdin, full queues, and cancellation with trailing events.

Two adversarial plugin tests reject a fabricated zero-record success and a
success response contradicting a failed vendor terminal. A Windows test retains
an exact handle to a descendant created after prompt receipt, proves it is alive
before cleanup, and confirms that same handle is signaled after timeout. This
does not prove atomic containment before Job Object assignment or an OS sandbox.

## Installed Native Protocol Checks

The final binaries were used with execution disabled and private local paths:

- Codex CLI 0.149.1 / app-server: initialized, two JSON records, native exit 0,
  no forced termination.
- Claude Code 2.1.224 / control stream: initialized, one JSON record, native
  exit 0, no forced termination.
- OpenCode 1.18.21 / ACP: initialized, one JSON record, native exit 0,
  no forced termination.
- GitHub Copilot CLI 1.0.75 / ACP: initialized, one JSON record, native exit 0,
  no forced termination.
- Grok Build 0.2.114 / ACP: two JSON records were received, but shutdown did not
  satisfy the deadline. The outcome was `failed`, with
  `harness_deadline_exceeded` and forced cleanup. It remains live-unverified.

These are initialization and lifecycle observations, not authentication or
generation tests. The optional fixed-reply model smoke was rejected by the
execution authorization boundary and **was not run**. No real model result,
vendor tool action, or account-usage authorization is implied.

## Public Artifact Review

`scripts/check_public_artifacts.py` checks tracked and non-ignored candidate text
files and prints only path, line, and rule names for findings. It found no
unresolved private-path, private-address, credential-pattern, or userinfo-URL
matches. One existing A2A negative URL test is explicitly reviewed: it uses
loopback-looking userinfo and a reserved `.example` host. Its exception is bound
to the exact trimmed line SHA-256, file path, and rule; another line or changed
content is not exempt.

This supplements human review and safe-by-default reports. It does not scan Git
history or binary artifacts, prove absence of every secret form, or make raw
vendor records suitable for publication. Machine configuration, native capture,
test logs, and runtime state remain ignored and were not committed.

## Remaining Qualification

- Resolve Grok's native shutdown behavior without weakening completion checks.
- Obtain explicit authorization for a real model roundtrip and any permitted
  tool action; the optional example is ready for that separate decision.
- Test Linux independently before extending this slice's platform claim.
- Bind the dispatcher to daemon-owned canonical Task/Run admission, durable
  event cursors, and restart fencing before adding executable A2A tasks.
- Add remote authentication, authorization, and conformance evidence before
  exposing an A2A service beyond loopback.

No commit, branch merge, push, dependency upgrade, global runtime change, or
deployment was performed. The original checkout remains on its Python baseline.
