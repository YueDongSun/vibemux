# Offline sandbox probe validation — 2026-10-26

This is bounded engineering evidence for ADR 043. It does not qualify a live
workflow, Claude profile, online gateway route or remote device.

## Source and automated checks

The source adds `vibemux_platform::sandbox_mxc` and the explicit
`vibemux_sandbox_probe` binary. Rust 1.92.0 on native Windows ran:

- `cargo test --locked --offline -p vibemux_platform -p vibemux_probe --all-targets -- --test-threads=1`:
  **81 passed**, zero reported failures or ignored tests. The adapter's conditional
  symbolic-link alias assertion did not execute because Windows returned error
  1314. That skipped assertion is not a platform pass.
- `cargo clippy --locked --offline -p vibemux_platform -p vibemux_probe --all-targets -- -D warnings`:
  passed on the final source, as did `cargo fmt --all -- --check`.
- Focused probe regressions: **12 passed**, including resolved UNC/device
  namespace rejection without accessing a share, cancellation retention, bounded
  output, connection classification and contradictory host-evidence rejection.

The validator was first observed failing its complete-valid-evidence test, then
implemented. Integration exposed and corrected the Windows system-directory
spelling mismatch, invalid TOML key quoting, a temp-directory policy conflict,
and privilege-dependent symbolic-link setup. Fixed fixture setup now uses a
contained built-in PowerShell junction command. No privilege setting was changed.

Independent source review returned a qualified PASS after the resolved scratch
parent was checked before fixture creation. Dependency versions did not change;
the lockfile gained only references to existing workspace dependencies.
`cargo-deny` and `cargo-audit` were unavailable (`no such command`); they were not
installed. Full-workspace, Ubuntu and release validation were not performed.

## Native observations

The selected installed backend reported `codex-cli 0.164.0-alpha.11.1`.

| Experiment | Observed result | What it establishes |
|---|---|---|
| Official explicit MXC startup command in an authorized normal host context | `MXC_OK`, exit 0 | That invocation can start MXC on this host. |
| Fixed Node inline fixture, root denied, exact workspace writable, protected/sealed paths, network disabled | Expected filesystem, child-write and IPv4-loopback results; zero listener accepts | Feasibility of the tested offline recipe. |
| Same recipe with managed networking and only a reserved `.invalid` domain allowed | Raw host-loopback connection succeeded | Domain rules do not provide gateway-only host-loopback isolation. |
| Scratch positive control with `:tmpdir=deny` | Scratch write denied | The symbolic temp rule resolved to the owned scratch directory; removing it from the fresh profile corrected the conflict. |
| Compiled fixed Rust workload launched directly by the disposable Node driver | All 12 child syscall observations matched expectations | Native workload startup and its observations; this is not the integrated runner's complete independent verdict. |
| Integrated contained Rust runner | `outcome: unavailable`, `reason_code: sandbox_mxc_unavailable`, backend exit 1, CLI exit 3 | Integrated MXC startup remains unqualified. |

The final integrated report confirmed fixture preflight, actual junction-alias
constructor rejection, listener challenge, completed output, empty owned process
tree, listener shutdown and fixture removal. It contained no workload check
records because the backend refused startup. It did not report success.

A paired comparison used one Node parent and a single adapter-generated command
plan. Node launched the fixed help command successfully; a Rust forwarding helper
returned the same MXC-unavailable category. The reconstructed plans compared
equal. Supplying the two libuv-added identity variables made the observed child
bootstrap environments equal, but the launch difference persisted. Removing the
additional Job Object in a disposable diagnostic and comparing noninteractive
console flags did not resolve it. These comparisons changed no product policy
or live admission gate. The underlying process-startup cause is unresolved.

The ordinary restricted-SID query returned unrestricted in both compared
contexts; that result does not establish equality of all Windows execution
security contexts. No broader Rust, MXC or Job Object compatibility claim is made.

## Remaining acceptance

Resolve the integrated startup difference through a supported launcher path,
then rerun the complete owner-computed suite. Preserve the offline policy and
containment requirements. Before any live workflow can use it, add separately
reviewed daemon-owned qualification, actual installed-Claude negative controls,
per-launch authority and request-bound gateway routing evidence.

The primary references remain the [Windows sandbox documentation](https://learn.chatgpt.com/docs/windows/windows-sandbox),
[Codex native availability probe](https://github.com/openai/codex/blob/970111f6cb62cc02fe36a82ea7d71b37a2889c97/codex-rs/mxc-sandbox/src/availability.rs),
and [libuv's Windows process implementation](https://github.com/libuv/libuv/blob/338ac6ac0714e6d8043a2826c9b3bc78e85bb961/src/win/process.c).
They support the implementation investigation, not a claim that the unresolved
native acceptance has passed.

## Recovery and complete native pass, 2026-10-27

The earlier startup results remain recorded above. A later investigation found
inherited Low Mandatory Level labels on the retained forwarding helper, the
integrated probe executable and its test executable. The retained helper's child
had the same user identity and unrestricted-SID state as the waiting parent but
a different integrity level. A fresh build of the unchanged helper source had
no Low label, matched the parent's integrity level and started the same fixed
MXC help command successfully. This is why the earlier restricted-SID check did
not establish an equivalent Windows execution context.

The integrated executable also failed creation of an owned fixture directory
with Windows error 5. Both native and canonical path spellings failed; a Node
control created and removed an empty directory under the same parent. These
results agree with Windows Mandatory Integrity Control: a Low process cannot
write up to a normal-integrity object merely because its DACL permits access.
See [Microsoft's MIC documentation](https://learn.microsoft.com/en-us/windows/win32/secauthz/mandatory-integrity-control).

No existing label, ACL, token, privilege or Job limit was changed. The original
artifacts and failed reports were preserved. The current source was built with
Rust 1.92.0 and the locked offline dependencies into a fresh, approved task-owned
`CARGO_TARGET_DIR`. That new executable had no inherited Low label.
The host reported Windows build `26200.9457`. The diagnostic process comparison
reported an elevated common parent; this evidence does not qualify a different
account or non-elevated launch context.

The complete integrated command then returned exit 0 and `outcome: passed`
against Codex CLI `0.164.0-alpha.11.1`, run
`f9406c9f-4520-4e37-827b-260a5e799865`. All twelve native observations matched:
workspace/scratch writes and permitted reads succeeded; protected, sealed,
outside, junction and descendant-write controls returned Windows error 5;
IPv4 loopback returned 10013. Every independent prerequisite, canary, output,
bounded-process, listener and cleanup check passed, with zero unexpected listener
accepts and no retained fixture.

The policy, contained trampoline, source validator and public command were
unchanged. Private diagnostic handle-inheritance experiments did not demonstrate
a causal fix and were not incorporated into production. Version-specific source
review used Node 24.18.0/libuv 1.52.1 and Rust 1.92.0; conclusions from newer libuv
implementations were not applied to these runtimes.

This supersedes the earlier integrated-startup blocker for the tested fresh
artifact and execution context. It does not certify arbitrary programs, online
network isolation, installed Claude tools or daemon workflow admission.
`reusable_qualification` remains false. The remaining work is daemon-owned
qualification with artifact/context binding, installed-harness negative controls,
per-launch authority and correlated gateway evidence.
