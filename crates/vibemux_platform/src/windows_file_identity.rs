//! Read-only full-width identity for an already-owned file handle (ADR 068).
#![allow(unsafe_code)] // ADR 068: only GetFileInformationByHandleEx(FileIdInfo).

use crate::PlatformError;
use std::{fs::File, mem::size_of, os::windows::io::AsRawHandle, ptr};
use windows_sys::Win32::{
    Foundation::{ERROR_INVALID_FUNCTION, ERROR_INVALID_PARAMETER, ERROR_NOT_SUPPORTED},
    Storage::FileSystem::{FILE_ID_128, FILE_ID_INFO, FileIdInfo, GetFileInformationByHandleEx},
};

#[derive(Clone, Copy, Eq, PartialEq)]
pub struct WindowsFileIdentity {
    pub volume_serial_number: u64,
    pub file_id: [u8; 16],
}

impl std::fmt::Debug for WindowsFileIdentity {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("WindowsFileIdentity(<redacted>)")
    }
}

/// Observes this handle only. The caller owns object/reparse/ancestry checks
/// and sharing policy. Identity alone is not permanent ownership authority.
pub fn query_windows_file_identity(file: &File) -> Result<WindowsFileIdentity, PlatformError> {
    let mut information = FILE_ID_INFO {
        VolumeSerialNumber: 0,
        FileId: FILE_ID_128 {
            Identifier: [0; 16],
        },
    };
    let size = u32::try_from(size_of::<FILE_ID_INFO>())
        .map_err(|_| PlatformError::FileIdentityUnsupported { os_code: None })?;
    // SAFETY: the borrowed File keeps its valid handle alive for this call.
    // information is fully initialized and has the ABI type's required size and
    // alignment. The pointer stays valid for the call and is not retained by it.
    let result = unsafe {
        GetFileInformationByHandleEx(
            file.as_raw_handle(),
            FileIdInfo,
            ptr::from_mut(&mut information).cast(),
            size,
        )
    };
    if result == 0 {
        return Err(query_error(std::io::Error::last_os_error().raw_os_error()));
    }
    from_information(information)
}

fn from_information(information: FILE_ID_INFO) -> Result<WindowsFileIdentity, PlatformError> {
    if information.FileId.Identifier.iter().all(|byte| *byte == 0) {
        return Err(PlatformError::FileIdentityUnsupported { os_code: None });
    }
    Ok(WindowsFileIdentity {
        volume_serial_number: information.VolumeSerialNumber,
        file_id: information.FileId.Identifier,
    })
}

fn query_error(os_code: Option<i32>) -> PlatformError {
    if os_code.is_some_and(|code| {
        [
            ERROR_INVALID_FUNCTION,
            ERROR_INVALID_PARAMETER,
            ERROR_NOT_SUPPORTED,
        ]
        .iter()
        .any(|expected| i32::try_from(*expected).ok() == Some(code))
    }) {
        PlatformError::FileIdentityUnsupported { os_code }
    } else {
        PlatformError::FileIdentityQuery { os_code }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn full_native_identifier_and_volume_width_survive_conversion() {
        let identifier = [
            1, 2, 3, 4, 5, 6, 7, 8, 0xf1, 0xf2, 0xf3, 0xf4, 0xf5, 0xf6, 0xf7, 0xf8,
        ];
        let observed = from_information(FILE_ID_INFO {
            VolumeSerialNumber: u64::MAX - 1,
            FileId: FILE_ID_128 {
                Identifier: identifier,
            },
        })
        .expect("identity");
        assert_eq!(observed.volume_serial_number, u64::MAX - 1);
        assert_eq!(observed.file_id, identifier);
        let mut other = observed;
        other.file_id[15] = 0;
        assert_ne!(observed, other, "upper bits participate in identity");
    }
    #[test]
    fn absent_identity_and_query_failures_never_mint_a_fallback() {
        assert!(
            from_information(FILE_ID_INFO {
                VolumeSerialNumber: 1,
                FileId: FILE_ID_128 {
                    Identifier: [0; 16]
                }
            })
            .is_err()
        );
        assert!(matches!(
            query_error(Some(50)),
            PlatformError::FileIdentityUnsupported { .. }
        ));
        assert!(matches!(
            query_error(Some(5)),
            PlatformError::FileIdentityQuery { .. }
        ));
    }
}
