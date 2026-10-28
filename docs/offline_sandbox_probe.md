# Offline Windows sandbox probe

`vibemux_sandbox_probe` performs a fixed, explicit OS experiment. It creates an
owned disposable directory, starts a temporary IPv4 loopback listener, executes
its native fixture through Codex's MXC backend, and checks the results. The
ordinary `vibemux_probe` and `vibemuxctl probe` remain read-only diagnostics.

This command does not invoke an AI model, change saved Codex settings, install a
sandbox, alter machine accounts/firewalls, or modify daemon state. A successful
report does not qualify Claude, an online route, or a workflow slot.

## Run

Use the repository-pinned Rust toolchain and an existing official Codex executable
that supports the documented MXC command. Run from an already authorized normal
Windows execution context; nested restrictions can prevent backend startup.
The runner never changes its token or retries through another backend.

```powershell
cargo build --locked --offline -p vibemux_probe --bin vibemux_sandbox_probe
New-Item -ItemType Directory -Path C:\scratch\vibemux_probe -Force
.\target\debug\vibemux_sandbox_probe.exe --backend C:\tools\codex.exe --scratch-parent C:\scratch\vibemux_probe
```

Replace the example paths with the installed executable and an operator-owned
scratch directory. No model, command body, network destination, policy override
or custom writable roots can be supplied to the public command. The selected
backend is an operator-trusted executable; a filename/version string is not an
authenticity proof. MXC and built-in Windows PowerShell must already be available.
Fixture setup uses a fixed PowerShell junction command with paths supplied only
through environment values; no user text becomes a script.
Missing prerequisites fail explicitly; the command does not provision them.

Build-artifact integrity labels are part of the execution context. An executable
carrying a Windows Low Mandatory Level label can fail fixture creation or MXC
startup even when its user identity matches and its restricted-SID list is empty.
Inspect only the intended executable's label with `icacls <probe.exe>` when
diagnosing this condition. Do not remove or raise labels to manufacture a pass.
For an explicitly authorized diagnostic, build the reviewed source into a fresh
operator-owned output directory using a process-scoped `CARGO_TARGET_DIR`, retain
the original evidence, and run the entire suite against that exact new artifact.
Changing build location is not itself qualification.

Exit codes:

| Code | Meaning |
|---|---|
| `0` | The complete fixed suite and independent postconditions passed. |
| `2` | Execution, observations, framing, postconditions or cleanup failed. |
| `3` | The platform or initial disposable fixture is unavailable. |
| `4` | Invalid command syntax; no diagnostic was launched. |

The backend's explicit MXC-unavailable response also returns exit `3`; the
reported backend exit code is kept separately from the diagnostic's exit code.

The JSON report has schema version `1` and suite `offline_mxc_native_v1`.
`requested_backend`, `requested_network_mode` and `managed_requirements_requested` describe explicit CLI policy,
not independent kernel attestation. `reusable_qualification` is always `false`.
Only the aggregate `outcome: "passed"` with exit `0` is a pass. The presence of
a JSON report, successful startup, a version string or an individual denial is
insufficient.

## What is checked

The fixed native workload attempts workspace read/write, scratch write,
protected read/write, sealed/outside reads, outside writes, directory-link
read/write, a synchronous descendant write and IPv4 loopback connection.
Expected successes must succeed; denied operations must return the recognized
Windows permission error. Missing files, connection refusal and timeout are
unexpected errors, not evidence of enforcement.

The owner separately checks the fixture prerequisites, a loopback challenge,
exact allowed output bytes, the descendant's startup marker, unchanged protected
and outside canaries, absence of every forbidden output, zero additional listener
accepts, completed bounded output, process-tree cleanup and fixture removal.
A malformed, missing, duplicate, reordered, stale or future-version child report
cannot pass. A child claim cannot override contradictory host observations.

The backend and fixed workload are started behind the existing contained
trampoline. Execution is bounded to 30 seconds per invocation, with separate
three-second release/reaping/output bounds. A failed or uncertain process cleanup
prevents success. If the runner retains a fixture for recovery, its generated leaf
name appears as `retained_fixture_name` beneath the supplied scratch parent.
Reconcile the owned process before manually removing that exact directory.

## Scope and migration

There is no database, Control protocol, existing probe-cache or configuration
migration. Existing clients never consume this new report implicitly. Any future
consumer must validate the suite/schema and must not turn this diagnostic into a
permission grant. Daemon qualification still needs artifact identity, validity,
revocation, owned launch policy and single-writer lifecycle records.

The suite measures the listed native operations. It does not test general Node
script loading, arbitrary agents, IPv6, external-network routing, system-service
process creation, live credentials, provider/model identity or remote devices.
MXC managed networking permits host-loopback connections; this implementation
therefore exposes only the offline policy. Its loopback probe does not claim to
prove every network protocol is blocked.

## Source checks

```text
cargo test --locked --offline -p vibemux_platform sandbox_mxc --lib
cargo test --locked --offline -p vibemux_probe sandbox_probe --lib
cargo test --locked --offline -p vibemux_probe --bin vibemux_sandbox_probe
cargo clippy --locked --offline -p vibemux_platform -p vibemux_probe --all-targets -- -D warnings
```

Expected results are passing focused tests, no Clippy diagnostics, and a complete
native report on a supported host. Unit tests do not replace the native command.
Linux compilation can check unsupported-platform behavior; it cannot establish
Windows enforcement. [ADR 043](adr/043_offline_sandbox_probe.md) records the
decision, alternatives, threat model and remaining qualification work.

The [evidence record](evidence/offline_sandbox_probe_validation.md) preserves
the initial source checks and startup failures, followed by a complete native
pass on a fresh artifact on 2026-10-27. That result still is not a reusable
installed-harness or workflow qualification credential.

The subsequent [installed Claude profile observations](evidence/claude_profile_validation.md)
found an incompatible native Write/Edit mapping and demonstrated a fixed stdio
MCP marker with built-ins removed. They do not qualify a writable workflow.
