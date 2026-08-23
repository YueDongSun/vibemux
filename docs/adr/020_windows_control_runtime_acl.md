# ADR 020: Windows per-user control runtime and ACL boundary

Status: Accepted for pre-alpha implementation

## Context

The project-local `.vibemux` directory can inherit broad drive ACLs. On the current Windows test machine it grants `Authenticated Users` modify access, so storing the bearer-token descriptor there cannot support a cross-user control-plane claim. Tokio already rejects remote named-pipe clients by default, but the default pipe DACL and a broadly readable descriptor are not a sufficient explicit Windows boundary.

Windows ACLs cannot isolate two processes running under the same logon SID. VibeMux agents intentionally run as that user and must share the daemon. Calling this a “hostile same-user ACL” gate is therefore both technically impossible and inconsistent with the product model.

## Decision

- Define the Windows security boundary as the current logon SID. Processes under that SID are trusted local collaborators; administrators and `SYSTEM` remain operating-system authorities outside the application boundary.
- Keep Rust/Python databases in project-local `.vibemux`, but move secret-bearing control metadata and the cooperative writer lock to `%LOCALAPPDATA%\VibeMux\runtime\<project_hash>` on Windows. The project hash is a domain-separated SHA-256 of the canonical project path and does not reveal the path text.
- POSIX keeps the existing project-local runtime with descriptor mode `0600` and randomized UDS endpoint.
- Before creating a Windows descriptor or lock, create the hashed leaf directory and replace inherited access rules with a protected DACL granting full control only to the current user SID, `LOCAL_SYSTEM`, and built-in administrators.
- Implement the DACL operation through a fixed encoded system-Windows-PowerShell companion. Paths cross only through environment variables; no user text enters PowerShell source or a command string. Rust core crates remain `unsafe`-free.
- Verify the effective ACL after setting it. Broad principals such as Everyone, Authenticated Users, Users, or sandbox groups must not remain on the protected leaf.
- Explicitly configure named-pipe remote-client rejection rather than relying only on Tokio defaults. Bearer authentication remains mandatory because same-SID clients are allowed by design.
- Preserve upgrade safety: legacy project-local descriptor/lock artifacts are detected. A healthy legacy daemon remains queryable/stoppable; stale legacy artifacts use the explicit inspect/recover flow. A new daemon is never spawned while either legacy artifact remains.
- Runtime reports, hashes, errors, and support output omit canonical project paths, SIDs, descriptor/token paths, and raw ACL text.

## Consequences

- Other standard local users cannot read the bearer descriptor or remove the cooperative lock through inherited project-drive permissions.
- Same-SID malware or a compromised same-user agent remains able to read the descriptor; this is an explicit trust-model limitation, not an ACL defect.
- Windows lifecycle startup depends on the fixed system-PowerShell compatibility boundary for DACL construction and verification until a separately reviewed native platform crate is justified.
- Admins and `SYSTEM` can access the runtime by operating-system authority.
- Existing pre-alpha project-local runtime artifacts require compatibility detection and, if stale, explicit confirmation-bound recovery.

## Alternatives

- Apply ACLs to the entire project `.vibemux` directory: rejected because it would mutate Python/worktree sharing semantics and inherited repository policy.
- Claim isolation from processes under the same SID: rejected because Windows discretionary ACLs authorize SIDs, not individual cooperating processes.
- Keep the token in a broad project descriptor and rely only on endpoint randomness: rejected because bearer confidentiality should not depend on obscurity.
- Add unreviewed Windows API `unsafe` calls directly to core crates: rejected while the fixed companion can enforce and verify the required DACL without weakening `#![forbid(unsafe_code)]`.
- Permit legacy and new locks concurrently during migration: rejected because it would break the single-authoritative-writer invariant.
