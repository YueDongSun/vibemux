# ADR 068: Full-width Windows file identity from owned handles

- Status: Accepted for this narrow read-only platform adapter
- Date: 2026-11-03
- Related: [ADR 025](025_native_read_only_acl_verification.md), [ADR 029](029_daemon_harness_dispatch.md), [proposed ADR 066](066_explicit_worktree_removal.md)

## Context

Cleanup previews and release-arm records need to compare the actual repository,
Git metadata, worktree and pinned executable objects. Canonical path text and a
logical path-derived resource key do not identify those objects. The standard
Windows volume/file-index metadata methods remain unstable for the pinned Rust
toolchain. The existing safe `winapi-util` information wrapper exposes the older
64-bit file ID, which is insufficient as a general Windows identity on ReFS.

Microsoft's [file-information contract](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/ns-fileapi-by_handle_file_information#members)
requires the full-width query for ReFS. The
[FILE_ID_INFO structure](https://learn.microsoft.com/en-us/windows/win32/api/winbase/ns-winbase-file_id_info)
combines a volume serial number and 128-bit file ID for comparing open objects.
These are filesystem identifiers, not permanent identities after deletion/reuse.

## Decision and unsafe boundary

Add `windows_file_identity` as the third explicitly reviewed unsafe module in
`vibemux_platform`, alongside the existing ACL and Job modules. Keep the crate
root's deny lint and every other workspace library's forbid lint. Update the
mechanical containment test to allow exactly these three modules.

Expose a safe function accepting an already-open `&File`. Its only native call
is `GetFileInformationByHandleEx(FileIdInfo)` with a fully initialized, correctly
sized `FILE_ID_INFO` buffer. The file borrow keeps the handle valid for the call;
the ABI type supplies size/alignment. One documented unsafe block contains the
call. Return the full `u64` volume serial and all sixteen identifier bytes. Error
or unsupported/all-zero identity returns a typed failure, with no 64-bit,
path-derived or zero-filled success fallback. Debug/errors omit identity values
and paths. No pathname is accepted, opened, scanned, mutated or terminated here.

Enable the existing `windows-sys` FileSystem feature. No newly resolved package,
global runtime change or additional process API is required.

Caller code remains responsible for ordinary-object/reparse/ancestry checks,
handle-sharing policy and comparing the same object it inspected. A share-7
observation handle does not freeze a namespace. Executables claimed as consumed
must use immutable pins held through launch; merely querying an observed path
does not identify the already-loaded image. Keep full-width values in private
Rust/store data and avoid a JavaScript-number round trip.

## Verification and authority limits

Test upper identifier bits and high volume bits, stable observations from one
open handle, rename/alias equality, different objects and unsupported handles.
Run native Windows checks, unsafe-containment enforcement, formatting and Clippy,
then an independent source review. Other platforms expose no fabricated fallback.
Windows 10 and additional filesystems remain unqualified until exercised.

This adapter grants no deletion, executable launch, process signalling, durable
quiescence or restart authority. Plan expiry, fresh handle comparisons, complete
ownership receipts and historical Removed tombstones remain necessary. Process
start-time seconds plus a live unreaped Held owner are audit binding only; adding
unsafe code for a finer process timestamp is outside this decision. The complete
cleanup lifecycle in ADR 066 now consumes this adapter in its bounded Windows
implementation. Native desktop and broader platform acceptance remain separate.
