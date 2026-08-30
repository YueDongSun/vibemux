## Scope

- issue/task: M7.1/M8.0 local stateful A2A and a real Planner-Worker-Reviewer-Verifier supervisor loop, continuing the earlier M4.2 registry slice.
- owned modules: A2A task adapter/runtime; canonical A2A types/store commands; owned workspaces; optional model peers; daemon supervisor/CLI; relevant tests, tools, English protocol/ADR/evidence documentation.
- original validation baseline: `85697d6743b01b8e0aa10c48d79837dcc56c007c`; branch `codex/m7_a2a_supervisor`; the 2026-08-30 validation used an uncommitted working tree, not that baseline commit. Publication preparation is recorded in the 2026-08-31 progress ledger. See [implementation fingerprint](evidence/a2a_source_manifest.json).

## Implemented

- Authenticated local HTTP+JSON, JSONRPC and gRPC task services, bounded streaming/resubscription, authorization by subject, idempotency, explicit cancellation and owned shutdown.
- Forward schema-2 migration with atomic A2A binding, optimistic updates, immutable audit events and independent review/verifier authority.
- Native Git worktrees/branches per writable role Run, owner receipts and immutable JSON artifacts with SHA-256 references.
- Separate Rust model-peer processes reading selected CC Switch profiles read-only; Anthropic Messages and recognized OpenAI-compatible Chat Completions adapters. API-root version segments are preserved.
- Explicit daemon `--supervisor-config` and `vibemux_supervisor --config ... --job ...`, with bounded repair attempts and structured completion receipts.
- Real child/network/Git regression fixtures, official TCK runner and pinned official Python/Go SDK bilateral interoperability tools.

## Behavior and invariants

- The daemon's existing writer remains the sole canonical writer. Plugins, model peers and SDK handlers have no core database/writer access. M4.2 plugin status remains read-only and prohibits Task/Run mutations.
- External Completed is an observation. Canonical completion requires distinct reviewer peer/Run/workspace, bound artifacts, current versions, positive review and an exact local verifier result. Repair creates new Runs without erasing the rejected attempt.
- Cancellation intent precedes the remote request. Late cancellation does not forge a remote canceled state for an already-completed peer; local finalization waits for all related Runs to stop.
- No model-generated code/shell is executed; prompts are data. No automatic stash, reset, clean, commit, merge or push. Dirty/mismatched cleanup fails closed.
- Default daemon startup enables no model peers or supervisor. Each explicit one-shot run owns a new daemon and refuses an existing writer.

## Files changed

- `crates/vibemux_a2a/`: task contracts, wire/client/server/gRPC/runtime, integration tests and conformance/interop examples.
- `crates/vibemux_types/`, `crates/vibemux_store/`: A2A records, validated restore, migration and authoritative transaction commands.
- `crates/vibemux_workspace/`, `crates/vibemux_model_peer/`: owned filesystem/process/provider boundaries and real integration tests.
- `crates/vibemuxd/`: weak non-owning writer handles, supervisor service/workflow/model process owner, explicit binary entrypoints and failure tests.
- `Cargo.toml`, `Cargo.lock`, `tools/`, `README.md`, `CHANGELOG.md`, `PROGRESS.md`, protocol/architecture docs, ADR 024 and example/evidence JSON.
- Earlier M4.2 changes remain present; see [its scoped handoff](m4_2_handoff.md). `/.weave/` remains ignored and untouched. The unrelated concurrent `rust-analyzer` component edit in `rust-toolchain.toml` is preserved, not authored by this task.

## Validation executed

| Command / scenario | Verified result |
|---|---|
| `cargo test --locked --offline --workspace --all-features -j 2` | 184 passed, 0 failed, 1 ignored across 49 groups. The ignored subprocess entry helper is invoked explicitly by its parent test. |
| `cargo fmt --all -- --check` | Passed. |
| `cargo clippy --locked --offline --workspace --all-targets --all-features -j 2 -- -D warnings` | Passed. |
| Isolated Python: `python -m pytest` | 27 passed. |
| `python -m mypy .` | 21 source files passed. |
| `python -m ruff check .`; `python -m ruff format --check .` | Passed; 64 files formatted. |
| `cargo nextest run --workspace --all-features`; `cargo deny check`; `cargo audit` | Each attempted, exit 101: subcommand not installed. Unverified gates. |
| `python tools/run_a2a_tck.py --tck-root <pinned_checkout> --fixture <built_fixture> --output <private_output>` | Official TCK all levels, three bindings: 207 passed, 53 skipped, 5 xfailed, no failures/errors; pytest/fixture exit 0 and no forced cleanup. |
| Python `a2a_python_interop.py client/serve` with Rust examples | Six bilateral binding cases passed using official SDK v1.1.3. |
| Go `go test -count=1 -v .`; `go vet .`; `go mod verify` | Three tests passed, zero skipped; vet and module verification passed using official SDK v2.5.0. |
| Selected real CC Switch API profile probes | 9/14 passed; remaining results: HTTP 401, 403, 404, 429 and invalid structured output. |
| `vibemux_supervisor --config <private_config> --job <synthetic_work_order>` | Three provider combinations passed; another fresh loop passed after the final API-root fix. Canonical `done`, exact result, review/verifier artifact hashes and joined shutdown were checked. |

Final static checks passed: `git diff --check`, cached diff check, 13 changed Markdown files with no missing local links, five JSON artifacts parsed, zero added private-path/key signature findings, and all 102 implementation fingerprints matched. These signature checks supplement review; they are not a proof against every possible secret format.

The [sanitized receipt](evidence/a2a_supervisor_validation.json) binds these results to source pins, final binary identities, test counts and receipt hashes. [Conformance reproduction](a2a_conformance_validation.md) explains setup, unmodified official assertions and exclusions. [Supervisor protocol](a2a_supervisor_protocol.md) documents the actual CLI/configuration and work order. Raw diagnostics, private selections and credentials are not public artifacts.

The live synthetic task sorted unique integers and returned `values=[-3,0,1,5,9]`, `sum=12`. Three independently configured provider combinations were exercised; the public receipt uses opaque model/profile aliases and omits the private selection mapping. Each used independent worker/reviewer Runs and completed in one attempt. The private receipts retain requested model IDs; neither those IDs nor the public aliases attest to supplier weights or constitute a model-quality ranking.

Real-process automated scenarios also cover rejected verification followed by a new repair attempt, active and review-stage cancellation, gRPC canonical state, sibling fault containment, stream lag, ownership mismatch and dirty-worktree refusal. Existing mock-plugin crash/restart exhaustion, quarantine, read-only visibility, cancellation and shutdown tests remain green. Earlier failing attempts are retained separately; no blind retries or weakened checks were used to turn failures green.

## Platform coverage

- Windows: native Windows 11 Pro build 26200, x86_64, Rust 1.85.0, CPython 3.13.12, Go 1.26.4; real localhost networking, child processes, named-pipe daemon lifecycle, Git worktrees and authorized cloud model requests.
- Linux/WSL: this slice was not executed there. Historical Linux results do not cover the new code.
- live terminal backend: not executed; no WezTerm/ConPTY/tmux or vendor coding CLI acceptance claim.

## Compatibility / migration

- A2A wire 1.0 with pinned official SDKs; VibeMux request contract `vibemux.task.request.v1`; private pre-alpha CLI/API contracts, not stable public compatibility.
- SQLite schema 1 migrates forward to 2 while retaining historical rows/events. Existing canonical state graphs are unchanged. Legacy snapshot writes cannot bypass gates on A2A-bound entities.
- Plugin wire/manifest remains unchanged; prior M4.2 IPC v2 remains, including its v1 lifecycle-request compatibility. No writable plugin API is added.
- Python state migration and default cutover are not implemented. Stop the daemon before maintenance. Disabling supervisor configuration stops new workflow admission, but does not downgrade a schema-2 database; do not point an older writer at it. Keep a stopped, consistent pre-migration backup for version rollback.

## Security impact

- Credentials are read only by explicitly selected peer profiles; no provider defaults/configuration were changed. Only synthetic task data was sent during acceptance. No repository files or inherited environment secrets were implicitly forwarded.
- Subject authorization, numeric loopback origin allowlists, redirect/proxy refusal, bounded bodies/queues/deadlines, sanitized errors and owned cancellation/shutdown are explicit boundaries.
- Git worktrees and environment filtering are not OS sandboxes. Same-user malicious interference, provider-side generation after disconnect and descendant process-tree containment are outside verified guarantees.
- Official conformance uses only a synthetic credential and deterministic backend; it cannot invoke real providers. Private raw evidence remains local. See ADR 024 for the threat model.

## Remaining work

- Overall M4/M7/M8 remain partial: complete official ITK, CI artifact integration, remote/TLS/SSRF deployment, signed cards/push, full message history, inbound cross-restart recovery, vendor CLI/terminal parity, generalized artifacts/DAG/merge review, fuzz/soak and missing cargo gate tooling.
- This is a structured-data supervisor recipe, not an unrestricted coding agent. The minimum real supervisor-loop acceptance is complete; it does not satisfy every future milestone.
- At the original 2026-08-30 handoff, no commit/push/deployment or new CI run had occurred. On 2026-08-31, feature-branch commit/push was explicitly authorized; its privacy and revalidation results are recorded in PROGRESS.md. This is not a tagged release, main-branch merge or production deployment.
- All three task-created developer worktrees were archived and removed with normal `git worktree remove`, without force; their branches were retained. Only the implementation worktree remains registered in this repository. Live synthetic repository worktrees, explicit artifacts, failed receipts and isolated verification environments are intentionally retained as private evidence, not unknown cleanup targets.

## Known risks

- Incoming runtime task/deduplication indexes and plugin restart/quarantine budgets are daemon-lifetime state. Canonical Runs/events persist, but automatic inbound resume after restart is absent.
- The TCK's 53 skips and five expected failures are not implementation evidence; message history and some unknown-field compatibility remain incomplete. Custom Python/Go scenarios are not the full official ITK.
- Provider HTTP/access/JSON failures remain failed profiles; OAuth-only, unsupported interfaces and the local large-model profile were not tested. No spend estimate or quality ranking is inferred.
- Abrupt daemon termination and forced/unconfirmed process cleanup are not covered by the successful normal shutdown receipt.
- A default-concurrency build exhausted memory during development; the final complete checks used `-j 2`. Unrelated build processes were left running. Auxiliary cargo checks could not run because the tools are absent.
