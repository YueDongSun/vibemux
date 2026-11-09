# Superagent workflow enhancement handoff

## Scope

- issue/task: adapt authorized operator intake, follow-ups and structured worker reports from the pinned DeepSeek superagent prototype (ADR 057).
- owned modules: `vibemux_workflow`, `vibemux_store`, `vibemuxd`, `vibemux_cli`, their tests, protocol/architecture docs and evidence.
- branch: `codex/dsh-superagent-enhancements`; this handoff describes the source/evidence change on that branch.

## Implemented

- `workflow intake` prepares an event-bound, policy-admitted workflow and returns its immutable mandate.
- `workflow followup` admits context for an exact known worker/session/contract at a turn boundary, with freshness, capacity and duplicate checks.
- `workflow reports` pages bounded structured claims; explicit content reads require owned opt-in retention.
- `worker_v2` requires schema-2 checkpoints. Human questions and unresolved outcomes fence automatic progress; ready still requires review and trusted verification.

## Behavior and invariants

- One writer commits report rows/events and follow-up admission atomically. Reports are append-only and verified against their audit events and owned attempts.
- Follow-ups do not widen contracts, change grants, reset budgets or revive terminal workflows.
- User text is data carried through JSON/stdin. Default events contain report hashes and sizes.
- Report readiness cannot become acceptance without existing gates. Corrupted projection flags fail closed.
- Existing `worker_v1` rendered bytes and established operation minimum versions are preserved.

## Files changed

- [CHANGELOG.md](../../CHANGELOG.md)
- [PROGRESS.md](../../PROGRESS.md)
- [README.md](../../README.md)
- [crates/vibemux_cli/src/command.rs](../../crates/vibemux_cli/src/command.rs)
- [crates/vibemux_cli/src/workflow_commands.rs](../../crates/vibemux_cli/src/workflow_commands.rs)
- [crates/vibemux_store/src/lib.rs](../../crates/vibemux_store/src/lib.rs)
- [crates/vibemux_store/src/workflow.rs](../../crates/vibemux_store/src/workflow.rs)
- [crates/vibemux_store/src/workflow/worker_reports.rs](../../crates/vibemux_store/src/workflow/worker_reports.rs)
- [crates/vibemux_store/tests/model_call_ledger.rs](../../crates/vibemux_store/tests/model_call_ledger.rs)
- [crates/vibemux_store/tests/workflow.rs](../../crates/vibemux_store/tests/workflow.rs)
- [crates/vibemux_store/tests/workflow_supervisor.rs](../../crates/vibemux_store/tests/workflow_supervisor.rs)
- [crates/vibemux_workflow/src/checkpoint.rs](../../crates/vibemux_workflow/src/checkpoint.rs)
- [crates/vibemux_workflow/src/lib.rs](../../crates/vibemux_workflow/src/lib.rs)
- [crates/vibemux_workflow/src/renderer.rs](../../crates/vibemux_workflow/src/renderer.rs)
- [crates/vibemux_workflow/src/templates.rs](../../crates/vibemux_workflow/src/templates.rs)
- [crates/vibemux_workflow/src/worker_report.rs](../../crates/vibemux_workflow/src/worker_report.rs)
- [crates/vibemuxd/src/bin/vibemux_worker_fixture.rs](../../crates/vibemuxd/src/bin/vibemux_worker_fixture.rs)
- [crates/vibemuxd/src/control.rs](../../crates/vibemuxd/src/control.rs)
- [crates/vibemuxd/src/control_workflow.rs](../../crates/vibemuxd/src/control_workflow.rs)
- [crates/vibemuxd/src/workflow/candidate.rs](../../crates/vibemuxd/src/workflow/candidate.rs)
- [crates/vibemuxd/src/workflow/coordinator.rs](../../crates/vibemuxd/src/workflow/coordinator.rs)
- [crates/vibemuxd/src/workflow/error.rs](../../crates/vibemuxd/src/workflow/error.rs)
- [crates/vibemuxd/src/workflow/service.rs](../../crates/vibemuxd/src/workflow/service.rs)
- [crates/vibemuxd/src/workflow/service/superagent.rs](../../crates/vibemuxd/src/workflow/service/superagent.rs)
- [crates/vibemuxd/src/workflow_writer.rs](../../crates/vibemuxd/src/workflow_writer.rs)
- [crates/vibemuxd/tests/workflow_superagent.rs](../../crates/vibemuxd/tests/workflow_superagent.rs)
- [docs/adr/057_superagent_workflow_intake.md](../../docs/adr/057_superagent_workflow_intake.md)
- [docs/architecture.md](../../docs/architecture.md)
- [docs/evidence/superagent_workflow_handoff.md](../../docs/evidence/superagent_workflow_handoff.md)
- [docs/evidence/superagent_workflow_validation.md](../../docs/evidence/superagent_workflow_validation.md)
- [docs/guides/superagent_workflow.md](../../docs/guides/superagent_workflow.md)
- [docs/protocol_boundaries.md](../../docs/protocol_boundaries.md)

## Validation executed

- command: `cargo check -p vibemux_workflow -p vibemux_store -p vibemuxd -p vibemux_cli --all-features --locked`
- result: passed.
- command: `TMPDIR=/private/tmp cargo test -p vibemux_workflow -p vibemux_store -p vibemux_cli -p vibemuxd --all-features --locked -- --skip failed_answer_delivery_does_not_block_later_repair --skip two_tracks_build_taskboard_with_a_scoped_bundle_and_isolated_integration`
- result: 644 passed, zero failed, zero ignored; two legacy browser acceptance cases explicitly filtered because their verifier does not discover the macOS browser path. All five new real-IPC fixture scenarios passed.
- command: `cargo fmt --all -- --check`
- result: passed.
- command: `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings`
- result: passed after building the existing frontend bundles.
- command: `git diff --check` and `python3 scripts/check_public_artifacts.py`
- result: passed, zero privacy findings.
- command: `TMPDIR=/private/tmp cargo test --workspace --all-features --locked`
- result: failed at three MCP lifecycle tests. The same three failures reproduced on the clean baseline. The unfiltered focused run separately reached the two unsupported browser cases. Neither run is claimed as successful.
- command: `cargo nextest run --workspace --all-features`, `cargo deny check`, `cargo audit`
- result: unavailable tools, exit 101.
- Refreshed merged-tree checks: all five new IPC scenarios, all 25 workflow-store tests, focused Clippy, formatting and staged-source/PR-body privacy checks passed. Original broad results above remain dated evidence.
- Full command history, prerequisites and diagnosed failures: [validation record](superagent_workflow_validation.md).

## Platform coverage

- Windows: not executed; release gates remain unverified.
- Linux/WSL: not executed.
- live terminal backend: not executed.
- native macOS: offline Rust fixtures, real IPC, owned processes/worktrees and syntax verification; Rust 1.92.0 with `TMPDIR=/private/tmp`.

## Compatibility / migration

- Control v11 adds three private operations; daemon and clients upgrade together.
- Store schema 10 adds immutable report rows through an atomic forward migration. Migration rollback/history tests passed; older binaries refuse the new schema.
- `worker_v2` is opt-in; `worker_v1` retains its rendered contract bytes.
- [ADR 057](../adr/057_superagent_workflow_intake.md) records old/new shapes and backup-based rollback; [operator guide](../guides/superagent_workflow.md) gives usage.

## Security impact

- Strict envelopes, bounded strings/encoded reports/pages/queues, exact contract/session/lease bindings and audit verification preserve existing boundaries.
- Report text requires per-workflow permission, ownership, retention and verified digests. Matching bytes in another workflow cannot grant retention permission.
- Privacy review covers publication and local retention: metadata permits correlation, plans retain source quotations, outboxes retain pending text, and opt-in report content is plaintext behind OS-user access controls. The guide records stdout disclosure and the limits of purge/expiry.
- Implementation validation used synthetic fixtures and invoked no source integration or live model turn. PR publication is a separate, explicit operator action.

## Remaining work

- Install/run nextest, deny and audit in the qualified validation environment.
- Recheck the dated baseline MCP/browser limitations with full current-tree CI and native Windows/Linux acceptance; upstream fixes were integrated during PR preparation.
- Live vendor certification and source integrations remain separate milestones; GitHub/Feishu monitoring, subscriptions/hand-back and external replies are outside this increment.

## Known risks

- This remains private pre-alpha functionality with offline fixture evidence.
- Initial baseline validation needed a short temporary directory; current main includes the subsequent macOS socket/process/browser fixes. Refreshed focused validation and historical limits are separated in the validation record.
- Old binaries cannot read schema 10. Restoring a separately retained schema-9 backup with the daemon stopped is the documented rollback path.
