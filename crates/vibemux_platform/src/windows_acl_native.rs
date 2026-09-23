//! Native read-only ACL verification (ADR 025, issue #8).
//!
//! This is the ONLY module in the workspace permitted to contain `unsafe`
//! (the crate root denies it; ADR 025 records the review). The surface is
//! strictly read-only: `GetNamedSecurityInfoW` reads a path's effective
//! DACL, `GetAclInformation`/`GetAce` walk it, and `EqualSid` compares
//! against the current process-token user and the two fixed well-known
//! SIDs. Nothing here writes an ACL, spawns a process, or hands memory to
//! the OS beyond API-owned buffers freed exactly once (`LocalFree`,
//! `CloseHandle`). Every FFI result is checked and fails closed.
//!
//! The rule set and the stage/index failure semantics are the ones the
//! Stage 1 batched PowerShell verifier established (issue #7): the
//! effective ACL must contain exactly three Allow ACEs, every principal
//! must be the current user, `LOCAL_SYSTEM`, or Builtin Administrators,
//! and the current user must hold `FullControl`. Errors carry no path,
//! SID, or ACL text (ADR 020 lines 17/21).
#![allow(unsafe_code)] // ADR 025: narrow read-only Win32 verification surface

use std::ffi::c_void;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;

use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_FILE_NOT_FOUND, ERROR_INVALID_NAME, ERROR_PATH_NOT_FOUND, HANDLE, LocalFree,
};
use windows_sys::Win32::Security::Authorization::{GetNamedSecurityInfoW, SE_FILE_OBJECT};
use windows_sys::Win32::Security::{
    ACCESS_ALLOWED_ACE, ACL, ACL_SIZE_INFORMATION, AclSizeInformation, DACL_SECURITY_INFORMATION,
    EqualSid, GetAce, GetAclInformation, GetTokenInformation, PSID, TOKEN_QUERY, TOKEN_USER,
    TokenUser,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

/// `FILE_ALL_ACCESS` (0x001F01FF): the access mask the reviewed PowerShell
/// bootstrap grants as `FileSystemRights::FullControl`, which ADR 020
/// requires for the current user. Defined locally so this module needs no
/// storage-filesystem feature surface for one constant.
const FILE_ALL_ACCESS_MASK: u32 = 0x001F_01FF;
const EXPECTED_EFFECTIVE_ACE_COUNT: u32 = 3;
/// `ACCESS_ALLOWED_ACE_TYPE` (0): the only ACE type the restricted rule set
/// accepts. Defined locally because windows-sys exposes no constant for it.
const ACCESS_ALLOWED_ACE_TYPE: u8 = 0;
/// A user SID is tens of bytes; `TOKEN_USER` adds a pointer plus attributes.
/// Anything larger is treated as an API contract violation, not buffered.
const MAX_TOKEN_INFORMATION_BYTES: u32 = 4096;

// Stage numbers mirror the Stage 1 helper contract so callers, tests, and
// the public error shapes stay unchanged across the native swap.
const STAGE_MISSING: u32 = 4;
const STAGE_DISALLOWED_PRINCIPAL: u32 = 5;
const STAGE_NO_FULL_CONTROL: u32 = 6;
const STAGE_ACE_COUNT: u32 = 7;
const STAGE_API_FAILURE: u32 = 9;

/// A fixed well-known SID in its wire layout. `EqualSid` reads only
/// `Revision`, `IdentifierAuthority`, and the first `SubAuthorityCount`
/// subauthorities, so over-allocating `sub_authorities` for the
/// single-subauthority `SYSTEM` SID is harmless.
#[repr(C)]
struct WellKnownSid {
    revision: u8,
    sub_authority_count: u8,
    identifier_authority: [u8; 6],
    sub_authorities: [u32; 2],
}

/// `S-1-5-18` (`LOCAL_SYSTEM`), NT authority 5 (big-endian wire order).
static SYSTEM_SID: WellKnownSid = WellKnownSid {
    revision: 1,
    sub_authority_count: 1,
    identifier_authority: [0, 0, 0, 0, 0, 5],
    sub_authorities: [18, 0],
};

/// `S-1-5-32-544` (Builtin Administrators).
static ADMINISTRATORS_SID: WellKnownSid = WellKnownSid {
    revision: 1,
    sub_authority_count: 2,
    identifier_authority: [0, 0, 0, 0, 0, 5],
    sub_authorities: [32, 544],
};

fn system_sid() -> PSID {
    // SAFETY: `SYSTEM_SID` is a 'static value in the SID wire layout; the
    // pointer outlives every use and `EqualSid` only reads it.
    std::ptr::addr_of!(SYSTEM_SID) as PSID
}

fn administrators_sid() -> PSID {
    // SAFETY: identical to `system_sid` for the administrators layout.
    std::ptr::addr_of!(ADMINISTRATORS_SID) as PSID
}

/// An open process-token handle, closed exactly once on drop.
struct ProcessToken(HANDLE);

impl Drop for ProcessToken {
    fn drop(&mut self) {
        // SAFETY: the handle came from `OpenProcessToken`, is owned solely
        // by this guard, and is closed exactly once here.
        unsafe { CloseHandle(self.0) };
    }
}

/// The current process-token user SID. `GetTokenInformation(TokenUser)`
/// writes the SID INLINE into the caller's buffer (right behind the
/// `TOKEN_USER` struct), so the buffer is owned here too: the pointer is
/// valid only while this guard, buffer included, lives. An earlier version
/// returned the pointer while dropping the buffer - a use-after-free that
/// compared against stale or reused memory and intermittently failed
/// healthy runtimes with stage 5.
struct CurrentUserSid {
    _token: ProcessToken,
    _buffer: Vec<u64>,
    sid: PSID,
}

fn current_user_sid() -> Option<CurrentUserSid> {
    let mut token: HANDLE = std::ptr::null_mut();
    // SAFETY: `GetCurrentProcess` returns a pseudo-handle valid inside this
    // process; `OpenProcessToken` writes the token handle on success and
    // the result is checked before the handle is used or stored.
    let opened = unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) };
    if opened == 0 || token.is_null() {
        return None;
    }
    let token = ProcessToken(token);

    let mut needed: u32 = 0;
    // SAFETY: sizing call with a null buffer - `GetTokenInformation`
    // documents ERROR_INSUFFICIENT_BUFFER plus the required byte count as
    // the result; `needed` is a valid writable out-parameter.
    unsafe { GetTokenInformation(token.0, TokenUser, std::ptr::null_mut(), 0, &mut needed) };
    if needed == 0 || needed > MAX_TOKEN_INFORMATION_BYTES {
        return None;
    }
    // `u64` slots guarantee the 8-byte alignment `TOKEN_USER`'s pointer
    // field requires; the length is rounded up and range-checked above.
    let buffer_length = usize::try_from((needed + 7) / 8).ok()?;
    let mut buffer: Vec<u64> = vec![0; buffer_length];
    let mut returned: u32 = 0;
    // SAFETY: `buffer` is a valid, aligned, live allocation of at least
    // `needed` bytes for the duration of the call; on success it holds a
    // `TOKEN_USER` whose `Sid` points into OS storage tied to the token,
    // which this function keeps open via the returned guard.
    let ok = unsafe {
        GetTokenInformation(
            token.0,
            TokenUser,
            buffer.as_mut_ptr().cast::<c_void>(),
            u32::try_from(buffer.len() * 8).ok()?,
            &mut returned,
        )
    };
    if ok == 0 || returned == 0 {
        return None;
    }
    // SAFETY: the API just wrote a `TOKEN_USER` at the buffer start; the
    // read stays inside the initialized region and the `Sid` pointer is
    // checked before use. The pointer references THIS buffer, which moves
    // into the returned guard (a `Vec` move keeps the heap storage at the
    // same address), so the SID outlives the function.
    let sid = unsafe { (*buffer.as_ptr().cast::<TOKEN_USER>()).User.Sid };
    if sid.is_null() {
        return None;
    }
    Some(CurrentUserSid {
        _token: token,
        _buffer: buffer,
        sid,
    })
}

/// A security descriptor allocated by `GetNamedSecurityInfoW`, freed with
/// `LocalFree` exactly once on drop (the documented release; any DACL/SID
/// pointers obtained from it stay valid only while this guard lives).
struct OwnedSecurityDescriptor(*mut c_void);

impl Drop for OwnedSecurityDescriptor {
    fn drop(&mut self) {
        // SAFETY: the descriptor was allocated by the security API for this
        // module and is freed exactly once here.
        unsafe { LocalFree(self.0) };
    }
}

/// Verify every path's effective DACL against the ADR 020/025 rule set.
/// The first violation is returned as `(stage, 1-based path index)`; a
/// token-read failure is reported as `(9, 0)` because no path was reached.
/// Batch-size policy lives with the public API in `windows_security`.
pub(crate) fn verify_paths(paths: &[&Path]) -> Result<(), (u32, u32)> {
    let user = current_user_sid().ok_or((STAGE_API_FAILURE, 0))?;
    for (offset, path) in paths.iter().enumerate() {
        let index = u32::try_from(offset + 1).map_err(|_| (STAGE_API_FAILURE, 0))?;
        verify_path(path, user.sid).map_err(|stage| (stage, index))?;
    }
    Ok(())
}

fn verify_path(path: &Path, current_user: PSID) -> Result<(), u32> {
    let wide: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let mut dacl: *mut ACL = std::ptr::null_mut();
    let mut descriptor: *mut c_void = std::ptr::null_mut();
    // SAFETY: `wide` is a NUL-terminated UTF-16 buffer that outlives the
    // call; both out-parameters are valid writable pointers, and the
    // return code is checked before either output is read. On failure no
    // descriptor was allocated.
    let status = unsafe {
        GetNamedSecurityInfoW(
            wide.as_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut dacl,
            std::ptr::null_mut(),
            &mut descriptor,
        )
    };
    if status != 0 {
        return Err(match status {
            ERROR_FILE_NOT_FOUND | ERROR_PATH_NOT_FOUND | ERROR_INVALID_NAME => STAGE_MISSING,
            _ => STAGE_API_FAILURE,
        });
    }
    // The guard must outlive `evaluate_dacl`: the DACL pointer references
    // memory inside the descriptor.
    let _descriptor = OwnedSecurityDescriptor(descriptor);
    evaluate_dacl(dacl, current_user)
}

fn evaluate_dacl(dacl: *const ACL, current_user: PSID) -> Result<(), u32> {
    let mut size_info = ACL_SIZE_INFORMATION {
        AceCount: 0,
        AclBytesInUse: 0,
        AclBytesFree: 0,
    };
    // SAFETY: `dacl` points inside the live descriptor guard's memory;
    // `size_info` is a valid writable struct of exactly the size the API
    // expects for `AclSizeInformation`, and the result is checked.
    let ok = unsafe {
        GetAclInformation(
            dacl,
            std::ptr::addr_of_mut!(size_info).cast::<c_void>(),
            u32::try_from(std::mem::size_of::<ACL_SIZE_INFORMATION>()).unwrap_or(0),
            AclSizeInformation,
        )
    };
    if ok == 0 {
        return Err(STAGE_API_FAILURE);
    }
    if size_info.AceCount != EXPECTED_EFFECTIVE_ACE_COUNT {
        return Err(STAGE_ACE_COUNT);
    }
    let mut current_full_control = false;
    for index in 0..EXPECTED_EFFECTIVE_ACE_COUNT {
        let mut ace: *mut c_void = std::ptr::null_mut();
        // SAFETY: `index` is within the `AceCount` just returned by the
        // API; `ace` is a valid out-parameter receiving a pointer into the
        // ACL, and both the BOOL and the pointer are checked before use.
        if unsafe { GetAce(dacl, index, &mut ace) } == 0 || ace.is_null() {
            return Err(STAGE_API_FAILURE);
        }
        // SAFETY: the ACE type check below gates interpretation; the header
        // prefix (type/flags/size/mask) has the same layout for every ACE
        // type, and `SidStart` marks the first byte of the SID the API
        // stores inline - a pointer that stays valid inside the ACL.
        let (ace_type, mask, sid) = unsafe {
            let ace = ace.cast::<ACCESS_ALLOWED_ACE>();
            (
                (*ace).Header.AceType,
                (*ace).Mask,
                std::ptr::addr_of!((*ace).SidStart) as PSID,
            )
        };
        if ace_type != ACCESS_ALLOWED_ACE_TYPE {
            return Err(STAGE_DISALLOWED_PRINCIPAL);
        }
        // SAFETY: all three comparison pointers reference well-formed SIDs
        // valid for this scope: two 'static well-known layouts and the
        // token-owned current-user SID held open by `verify_paths`.
        // `EqualSid` is read-only.
        let (matches_current, matches_system, matches_administrators) = unsafe {
            (
                EqualSid(sid, current_user) != 0,
                EqualSid(sid, system_sid()) != 0,
                EqualSid(sid, administrators_sid()) != 0,
            )
        };
        if matches_current {
            if mask & FILE_ALL_ACCESS_MASK == FILE_ALL_ACCESS_MASK {
                current_full_control = true;
            }
        } else if !matches_system && !matches_administrators {
            return Err(STAGE_DISALLOWED_PRINCIPAL);
        }
    }
    if current_full_control {
        Ok(())
    } else {
        Err(STAGE_NO_FULL_CONTROL)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secure_user_directory;
    use windows_sys::Win32::Security::Authorization::ConvertSidToStringSidW;
    use windows_sys::core::PWSTR;

    fn sid_to_string(sid: PSID) -> Option<String> {
        let mut text: PWSTR = std::ptr::null_mut();
        // SAFETY: test helper; `ConvertSidToStringSidW` allocates the wide
        // string, which is read to its terminator (bounded) and released
        // with `LocalFree` exactly once.
        unsafe {
            if ConvertSidToStringSidW(sid, &mut text) == 0 || text.is_null() {
                return None;
            }
            let mut units: Vec<u16> = Vec::new();
            let mut cursor = text;
            for _ in 0..128 {
                if *cursor == 0 {
                    break;
                }
                units.push(*cursor);
                cursor = cursor.add(1);
            }
            LocalFree(text.cast::<c_void>());
            Some(
                units
                    .iter()
                    .map(|unit| char::from_u32(u32::from(*unit)).unwrap_or('?'))
                    .collect(),
            )
        }
    }

    #[test]
    fn fixed_sids_match_their_canonical_string_forms() {
        // Pins the hand-written wire layouts: a wrong byte order or
        // subauthority count would silently compare unequal (fail-closed)
        // or, worse, match nothing and reject healthy runtimes.
        assert_eq!(sid_to_string(system_sid()).as_deref(), Some("S-1-5-18"));
        assert_eq!(
            sid_to_string(administrators_sid()).as_deref(),
            Some("S-1-5-32-544")
        );
    }

    #[test]
    fn current_user_sid_is_readable_and_canonical() {
        let user = current_user_sid().expect("token user SID");
        let text = sid_to_string(user.sid).expect("SID string");
        assert!(text.starts_with("S-1-"), "unexpected SID form");
    }

    #[test]
    fn concurrent_verification_with_heap_churn_is_stable() {
        // Regression pin for the first version of this module: it returned
        // a SID POINTER into the TOKEN_USER buffer but let the buffer drop
        // (use-after-free). Most reads still saw the old bytes, so the bug
        // surfaced only intermittently - a healthy secured runtime failed
        // closed with stage 5 once the allocator reused the memory (a
        // concurrently running suite tripped it within a few rounds, a
        // single-threaded loop never did, because the dangling pointer is
        // used immediately after the drop with no allocation in between).
        // The threads below interleave same-size poisoned allocations with
        // verifications so the reuse actually happens inside the window.
        let temp = tempfile::tempdir().expect("temp directory");
        let protected = temp.path().join("protected_runtime");
        std::fs::create_dir(&protected).expect("protected directory");
        secure_user_directory(&protected).expect("secure directory");
        std::thread::scope(|scope| {
            for worker in 0..4usize {
                let protected = protected.as_path();
                scope.spawn(move || {
                    for round in 0..1500usize {
                        let mut junk: Vec<Vec<u64>> = Vec::with_capacity(8);
                        for block in 0..8usize {
                            // 2..8 u64 blocks: the size class the TOKEN_USER
                            // buffer (a handful of u64 slots) lands in,
                            // filled with poison so reuse is observable.
                            junk.push(vec![0xDEAD_BEEF_u64; (round + worker + block) % 7 + 2]);
                        }
                        std::hint::black_box(&junk);
                        assert!(
                            verify_paths(&[protected]).is_ok(),
                            "worker {worker} round {round} must not fail on a secured runtime"
                        );
                    }
                });
            }
        });
    }

    #[test]
    fn missing_path_fails_closed_at_stage_four() {
        let unreachable = std::env::temp_dir()
            .join(format!("vibemux_native_missing_{}", std::process::id()))
            .join("definitely_missing");
        assert_eq!(
            verify_paths(&[unreachable.as_path()]),
            Err((STAGE_MISSING, 1))
        );
    }

    #[test]
    fn unprotected_directory_fails_closed() {
        // A plain temp directory carries the default inherited ACEs of the
        // user profile (more than three effective rules and broader
        // principals), so it must never pass the restricted rule set.
        let temp = tempfile::tempdir().expect("temp directory");
        let plain = temp.path().join("plain");
        std::fs::create_dir(&plain).expect("plain directory");
        let result = verify_paths(&[plain.as_path()]);
        assert!(matches!(result, Err((stage, 1)) if stage != 0));
    }
}
