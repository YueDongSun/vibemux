"""Fail-closed Windows launch context checks for acceptance tools."""

from __future__ import annotations

import ctypes
import os
from collections.abc import Callable
from dataclasses import dataclass
from typing import Final

TOKEN_RESTRICTED_SIDS: Final = 11
TOKEN_QUERY: Final = 0x0008
ERROR_INSUFFICIENT_BUFFER: Final = 122
MAX_TOKEN_INFORMATION_BYTES: Final = 1 << 20
SEM_FAILCRITICALERRORS: Final = 0x0001
SEM_NOGPFAULTERRORBOX: Final = 0x0002
WINDOWS_TOOL_INITIALIZATION_FAILED: Final = -1073741502


class Windows_Launch_Error(RuntimeError):
    """A native Windows launch precondition or tool initialization failed."""

    def __init__(self, code: str, detail: str) -> None:
        super().__init__(f"{code}: {detail}")
        self.code = code


@dataclass(frozen=True)
class _Token_Query_Api:
    get_current_process: Callable[[], object]
    open_process_token: Callable[..., object]
    get_token_information: Callable[..., object]
    close_handle: Callable[..., object]
    get_last_error: Callable[[], int]


class _Sid_And_Attributes(ctypes.Structure):
    _fields_ = [("sid", ctypes.c_void_p), ("attributes", ctypes.c_uint32)]


def _windows_token_query_api() -> _Token_Query_Api:
    if os.name != "nt":
        raise OSError("Windows token APIs are unavailable")
    advapi32 = ctypes.WinDLL("advapi32", use_last_error=True)
    kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
    handle_type = ctypes.c_void_p
    advapi32.OpenProcessToken.argtypes = [handle_type, ctypes.c_uint32, ctypes.POINTER(handle_type)]
    advapi32.OpenProcessToken.restype = ctypes.c_int
    advapi32.GetTokenInformation.argtypes = [
        handle_type,
        ctypes.c_uint32,
        ctypes.c_void_p,
        ctypes.c_uint32,
        ctypes.POINTER(ctypes.c_uint32),
    ]
    advapi32.GetTokenInformation.restype = ctypes.c_int
    kernel32.CloseHandle.argtypes = [handle_type]
    kernel32.CloseHandle.restype = ctypes.c_int
    kernel32.GetCurrentProcess.argtypes = []
    kernel32.GetCurrentProcess.restype = handle_type
    return _Token_Query_Api(
        kernel32.GetCurrentProcess,
        advapi32.OpenProcessToken,
        advapi32.GetTokenInformation,
        kernel32.CloseHandle,
        ctypes.get_last_error,
    )


def _restricted_sid_count(api: _Token_Query_Api) -> int:
    token_handle = ctypes.c_void_p()
    process_handle = api.get_current_process()
    if not api.open_process_token(process_handle, TOKEN_QUERY, ctypes.byref(token_handle)):
        raise OSError(api.get_last_error(), "OpenProcessToken failed")
    if not token_handle.value:
        raise OSError("OpenProcessToken succeeded without returning a token handle")
    try:
        required_size = ctypes.c_uint32(0)
        size_query_succeeded = api.get_token_information(
            token_handle, TOKEN_RESTRICTED_SIDS, None, 0, ctypes.byref(required_size)
        )
        error_code = api.get_last_error()
        if size_query_succeeded or error_code != ERROR_INSUFFICIENT_BUFFER:
            raise OSError(error_code, "GetTokenInformation size query failed")
        size = int(required_size.value)
        if size <= 0 or size > MAX_TOKEN_INFORMATION_BYTES:
            raise OSError("GetTokenInformation returned an invalid buffer size")
        buffer = ctypes.create_string_buffer(size)
        returned_size = ctypes.c_uint32(0)
        if not api.get_token_information(
            token_handle,
            TOKEN_RESTRICTED_SIDS,
            buffer,
            size,
            ctypes.byref(returned_size),
        ):
            raise OSError(api.get_last_error(), "GetTokenInformation query failed")
        actual_size = int(returned_size.value)
        alignment = ctypes.alignment(_Sid_And_Attributes)
        header_size = ctypes.sizeof(ctypes.c_uint32)
        groups_offset = (header_size + alignment - 1) // alignment * alignment
        group_stride = ctypes.sizeof(_Sid_And_Attributes)
        if actual_size > size or actual_size < groups_offset:
            raise OSError("GetTokenInformation returned an invalid data length")
        group_count = ctypes.cast(buffer, ctypes.POINTER(ctypes.c_uint32)).contents.value
        if group_count > (actual_size - groups_offset) // group_stride:
            raise OSError("TOKEN_GROUPS count exceeds the returned buffer")
        return int(group_count)
    finally:
        if not api.close_handle(token_handle):
            raise OSError(api.get_last_error(), "CloseHandle failed")


def token_restriction_state(
    api_factory: Callable[[], _Token_Query_Api] = _windows_token_query_api,
) -> str:
    """Returns unrestricted, restricted, or unknown for the current token."""
    if os.name != "nt":
        return "unrestricted"
    try:
        return "restricted" if _restricted_sid_count(api_factory()) else "unrestricted"
    except (OSError, AttributeError, TypeError, ValueError, ctypes.ArgumentError):
        return "unknown"


def require_launch_context(
    restriction_probe: Callable[[], str] = token_restriction_state,
) -> None:
    """Refuses native child launches for restricted or unknown token state."""
    if os.name != "nt":
        return
    state = restriction_probe()
    if state == "unrestricted":
        return
    if state == "restricted":
        raise Windows_Launch_Error(
            "windows_restricted_token", "native child launch refused for a restricted token"
        )
    raise Windows_Launch_Error(
        "windows_token_context_unknown",
        "native child launch refused because token state is unknown",
    )


def suppress_windows_error_dialogs() -> None:
    """Set process-local fatal-error dialog bits while preserving existing bits."""
    if os.name != "nt":
        return
    try:
        kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
        kernel32.GetErrorMode.argtypes = []
        kernel32.GetErrorMode.restype = ctypes.c_uint32
        kernel32.SetErrorMode.argtypes = [ctypes.c_uint32]
        kernel32.SetErrorMode.restype = ctypes.c_uint32
        previous_mode = int(kernel32.GetErrorMode())
        requested_mode = previous_mode | SEM_FAILCRITICALERRORS | SEM_NOGPFAULTERRORBOX
        kernel32.SetErrorMode(requested_mode)
        observed_mode = int(kernel32.GetErrorMode())
    except (OSError, AttributeError, TypeError, ValueError, ctypes.ArgumentError) as error:
        raise Windows_Launch_Error(
            "windows_error_mode_unavailable", "process error mode could not be configured"
        ) from error
    if observed_mode != requested_mode:
        raise Windows_Launch_Error(
            "windows_error_mode_unavailable", "process error mode could not be verified"
        )
