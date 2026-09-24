# ADR 025: Native read-only ACL verification for the Windows control runtime

Status: Accepted for pre-alpha implementation. Amends [ADR 020](020_windows_control_runtime_acl.md) for the read-only verification path only.

## Context

ADR 020 implements the Windows control-runtime DACL through a fixed encoded system-PowerShell companion and deferred a native platform crate "until a separately reviewed native platform crate is justified". Issue #7 Stage 1 (`1a112d6`) wired per-start verification through that companion: one batched `powershell.exe` invocation re-verifies the protected root, the per-project runtime leaf, the `.acl_v1` marker, and the published artifacts on every trusted daemon start.

Stage 1 closed the security gap but the helper spawn costs a measured ~400 ms per start on the reference machine (release benchmark, existing database, 1 warmup + 20 samples: p50 482.80 → 903.22 ms, p95 558.06 → 920.31 ms). `PROGRESS.md` §9 requires cold start to authenticated health at p95 < 500 ms *after the one-time bootstrap*; per-start verification is not exempt. A native read of an object's security descriptor is ~1–5 ms with no process spawn — this is the justification ADR 020 anticipated. The write path (`secure_user_directory`) runs only at one-time bootstrap or marker heal, where the existing reviewed helper stays within the §9 bootstrap budget.

## Decision

- Implement **read-only** ACL verification natively in `vibemux_platform` (`windows_acl_native` module) via `windows-sys` (workspace-pinned `=0.59.0`, already a transitive workspace dependency): `GetNamedSecurityInfoW(SE_FILE_OBJECT, DACL_SECURITY_INFORMATION)`, a `GetAclInformation`/`GetAce` walk of the effective DACL, and `EqualSid` comparisons against the current process-token user SID and the fixed well-known SIDs `S-1-5-18` (`LOCAL_SYSTEM`) and `S-1-5-32-544` (Builtin Administrators). The returned security descriptor is freed with `LocalFree`; the token handle is closed.
- The rule set is unchanged from ADR 020 / Stage 1: the effective ACL contains exactly three Allow ACEs, every principal is in {current user, `SYSTEM`, Builtin Administrators}, and the current user holds `FullControl`. Failure reporting keeps the Stage 1 stage/index semantics (4 = missing, 5 = disallowed principal or non-Allow ACE, 6 = current user lacks FullControl, 7 = effective ACE count is not exactly three, 9 = unverifiable/API failure) and never contains path, SID, or ACL text (ADR 020 lines 17/21 continue to apply verbatim).
- The native module becomes the **single** implementation of the verification rule set; the Stage 1 batched PowerShell verify script is deleted rather than kept as a fallback, so the security rule set cannot drift between two implementations. The batched `verify_restricted_path_acls` / single-path `verify_restricted_path_acl` API shapes are unchanged.
- ACL **writes** (`secure_user_directory`) remain on the reviewed fixed PowerShell companion script. This ADR introduces no native ACL mutation.
- `unsafe` policy for `vibemux_platform`: the crate root moves from `#![forbid(unsafe_code)]` to `#![deny(unsafe_code)]`; `unsafe` is allowed in exactly one module, `windows_acl_native`, which carries `#![allow(unsafe_code)]`, a reference to this ADR, and a `// SAFETY:` comment on every `unsafe` block. All other workspace crates keep `#![forbid(unsafe_code)]`. The unsafe surface is read-only: no ACL is written, no memory is handed to the OS beyond API-owned buffers this module frees exactly once, and every FFI result is checked and fails closed.
- Because native verification is ~ms-cheap, trusted daemon starts verify in **two phases**: root, runtime leaf, marker, and writer lock *before* the token-bearing descriptor is published (closing the Stage 1 sub-second window in which the descriptor existed before verification could fail closed), then the published artifacts after publication.
- Blocking-boundary rule unchanged: callers in async contexts reach verification through `spawn_blocking` (AGENTS §7.4), now for filesystem/LSA read latency hygiene rather than process-spawn blocking.

## Consequences

- Trusted daemon start returns to the same machine-specific level as the pre-change baseline (the ~400 ms Stage 1 helper-spawn overhead is removed; the §9 deviation note in `PROGRESS.md` was **rewritten**, not removed, to record the measured absolute p95 and the baseline-vs-target context - on the reference machine both the pre-change baseline and the post-change measurement sit above the 500 ms target, which remains open).
- `powershell.exe` spawns on the lifecycle path are reduced to the one-time bootstrap/heal write and the CLI process-launch companion (ADR 018); the issue #6 helper-load class shrinks accordingly (`helper_slot` now guards the write path only).
- The same-SID trust-model limitation is unchanged (ADR 020 line 26): native verification detects ACL drift and misconfiguration; it does not isolate same-user processes.
- `vibemux_platform` is no longer `unsafe`-free at the crate level; containment is the root-level `deny` plus the single module-level `allow`, and any future `unsafe` outside that module requires its own accepted ADR.
- The verification path no longer depends on PowerShell availability or .NET behavior; its failure modes are Win32 error codes that fail closed.

## Alternatives

- Keep the Stage 1 batched PowerShell verifier: rejected — measured +400 ms per trusted start violates the §9 budget and keeps a per-start process spawn (the issue #6 load class).
- Keep the PowerShell verifier as a fallback under the native path: rejected — two implementations of one security rule set can drift; the native path is deterministic and fail-closed on every API error.
- Full `windows` crate instead of `windows-sys`: rejected — wrapper weight is unnecessary for a fixed read-only call sequence, and `windows-sys 0.59.0` is already in the dependency lock.
- Native ACL writes (`SetNamedSecurityInfoW`) too: rejected for this slice — the write path runs once at bootstrap inside the §9 bootstrap budget and the reviewed fixed script stays authoritative; a native writer would need its own review.
- A separate crate to keep `vibemux_platform` forbid-clean: rejected — AGENTS §7.2 already scopes `unsafe` to "a narrowly scoped platform module", and `vibemux_platform` is the established platform boundary; module containment with a root `deny` provides the same guarantee without workspace-graph churn.
