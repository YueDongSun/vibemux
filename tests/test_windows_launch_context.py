from __future__ import annotations

import ctypes
import sys
from pathlib import Path

import pytest

repo_root = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(repo_root / "scripts"))

from dual_track_acceptance import windows_launch_context as launch_context  # noqa: E402


class Fake_Token_Api:
    def __init__(
        self,
        group_count: int,
        *,
        reported_size: int | None = None,
        token_handle_value: int | None = 123,
        close_succeeds: bool = True,
    ) -> None:
        self.group_count = group_count
        self.reported_size = reported_size
        self.token_handle_value = token_handle_value
        self.close_succeeds = close_succeeds
        self.close_count = 0
        self.last_error = launch_context.ERROR_INSUFFICIENT_BUFFER

    def open_process_token(self, process: object, access: int, token: object) -> bool:
        ctypes.cast(token, ctypes.POINTER(ctypes.c_void_p)).contents.value = self.token_handle_value
        return True

    def get_token_information(
        self,
        token: object,
        information_class: int,
        buffer: object,
        size: int,
        returned_size: object,
    ) -> bool:
        required_size = (
            ctypes.sizeof(ctypes.c_uint32)
            + (ctypes.alignment(launch_context._Sid_And_Attributes) - 1)
            + self.group_count * ctypes.sizeof(launch_context._Sid_And_Attributes)
        )
        required_size = max(required_size, 8)
        size_ref = ctypes.cast(returned_size, ctypes.POINTER(ctypes.c_uint32)).contents
        if buffer is None:
            size_ref.value = required_size
            self.last_error = launch_context.ERROR_INSUFFICIENT_BUFFER
            return False
        if self.reported_size is not None:
            size_ref.value = self.reported_size
        else:
            size_ref.value = required_size
        ctypes.cast(buffer, ctypes.POINTER(ctypes.c_uint32)).contents.value = self.group_count
        self.last_error = 0
        return True

    def close_handle(self, token: object) -> bool:
        self.close_count += 1
        return self.close_succeeds

    def api(self) -> launch_context._Token_Query_Api:
        return launch_context._Token_Query_Api(
            lambda: 1,
            self.open_process_token,
            self.get_token_information,
            self.close_handle,
            lambda: self.last_error,
        )


@pytest.mark.parametrize("state", ["restricted", "unknown"])
def test_require_launch_context_refuses_unsafe_or_unknown_token(
    monkeypatch: pytest.MonkeyPatch, state: str
) -> None:
    monkeypatch.setattr(launch_context.os, "name", "nt")
    with pytest.raises(launch_context.Windows_Launch_Error):
        launch_context.require_launch_context(lambda: state)


def test_require_launch_context_accepts_known_unrestricted_token(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(launch_context.os, "name", "nt")
    launch_context.require_launch_context(lambda: "unrestricted")


@pytest.mark.parametrize(("group_count", "expected"), [(0, 0), (2, 2)])
def test_restricted_sid_count_checks_valid_buffer_and_closes_token(
    group_count: int, expected: int
) -> None:
    api = Fake_Token_Api(group_count)
    assert launch_context._restricted_sid_count(api.api()) == expected
    assert api.close_count == 1


def test_restricted_sid_count_rejects_invalid_reported_length_and_closes_token() -> None:
    api = Fake_Token_Api(1, reported_size=4)
    with pytest.raises(OSError):
        launch_context._restricted_sid_count(api.api())
    assert api.close_count == 1


def test_restricted_sid_count_rejects_null_success_handle_without_closing_null() -> None:
    api = Fake_Token_Api(0, token_handle_value=None)
    with pytest.raises(OSError):
        launch_context._restricted_sid_count(api.api())
    assert api.close_count == 0


def test_restricted_sid_count_reports_owned_handle_close_failure() -> None:
    api = Fake_Token_Api(0, close_succeeds=False)
    with pytest.raises(OSError, match="CloseHandle failed"):
        launch_context._restricted_sid_count(api.api())
    assert api.close_count == 1


def test_token_restriction_state_returns_unknown_for_bad_size(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(launch_context.os, "name", "nt")
    assert (
        launch_context.token_restriction_state(lambda: (_ for _ in ()).throw(OSError()))
        == "unknown"
    )


def test_suppress_windows_error_dialogs_preserves_existing_bits(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(launch_context.os, "name", "nt")
    mode = {"value": 0x20}

    class Fake_Function:
        def __init__(self, callback: object) -> None:
            self.callback = callback

        def __call__(self, *args: int) -> int:
            return self.callback(*args)  # type: ignore[operator]

    class Fake_Kernel32:
        GetErrorMode = Fake_Function(lambda: mode["value"])

        @staticmethod
        def _set_error_mode(value: int) -> int:
            previous = mode["value"]
            mode["value"] = value
            return previous

        SetErrorMode = Fake_Function(_set_error_mode)

    # ctypes.WinDLL exists only on Windows; raising=False lets the fake stand in elsewhere.
    monkeypatch.setattr(
        launch_context.ctypes, "WinDLL", lambda *args, **kwargs: Fake_Kernel32(), raising=False
    )
    launch_context.suppress_windows_error_dialogs()
    assert (
        mode["value"]
        == 0x20 | launch_context.SEM_FAILCRITICALERRORS | launch_context.SEM_NOGPFAULTERRORBOX
    )


def test_suppress_windows_error_dialogs_normalizes_api_failure(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(launch_context.os, "name", "nt")

    class Fake_Function:
        def __init__(self, callback: object) -> None:
            self.callback = callback

        def __call__(self, *args: int) -> int:
            return self.callback(*args)  # type: ignore[operator]

    class Broken_Kernel32:
        GetErrorMode = Fake_Function(lambda: (_ for _ in ()).throw(OSError("native failure")))
        SetErrorMode = Fake_Function(lambda mode: 0)

    monkeypatch.setattr(
        launch_context.ctypes, "WinDLL", lambda *args, **kwargs: Broken_Kernel32(), raising=False
    )
    with pytest.raises(launch_context.Windows_Launch_Error) as error:
        launch_context.suppress_windows_error_dialogs()
    assert error.value.code == "windows_error_mode_unavailable"
    assert isinstance(error.value.__cause__, OSError)


def test_validate_cli_does_not_probe_or_mutate_windows_state(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    import importlib.util

    script_path = repo_root / "scripts" / "verify_dual_track.py"
    spec = importlib.util.spec_from_file_location("verify_dual_track_cli", script_path)
    assert spec is not None and spec.loader is not None
    cli = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(cli)
    monkeypatch.setattr(cli, "require_launch_context", lambda: pytest.fail("token queried"))
    monkeypatch.setattr(cli, "suppress_windows_error_dialogs", lambda: pytest.fail("mode changed"))
    monkeypatch.setattr(cli, "validate", lambda path: 0)
    assert cli.main(["--validate", str(tmp_path / "report.json")]) == 0


def test_cli_refuses_restricted_context_before_version_probe(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    import importlib.util

    script_path = repo_root / "scripts" / "verify_dual_track.py"
    spec = importlib.util.spec_from_file_location("verify_dual_track_cli_refusal", script_path)
    assert spec is not None and spec.loader is not None
    cli = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(cli)
    monkeypatch.setattr(
        cli,
        "require_launch_context",
        lambda: (_ for _ in ()).throw(
            launch_context.Windows_Launch_Error("windows_restricted_token", "refused")
        ),
    )
    monkeypatch.setattr(cli, "run", lambda request: pytest.fail("runner called"))
    monkeypatch.setattr(cli, "suppress_windows_error_dialogs", lambda: None)
    assert cli.main(["--mode", "offline", "--output", "output"]) == 2


def test_cli_reports_native_initialization_failure_without_losing_status(
    monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    import importlib.util

    script_path = repo_root / "scripts" / "verify_dual_track.py"
    spec = importlib.util.spec_from_file_location("verify_dual_track_cli_native_error", script_path)
    assert spec is not None and spec.loader is not None
    cli = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(cli)
    monkeypatch.setattr(cli, "require_launch_context", lambda: None)
    monkeypatch.setattr(cli, "suppress_windows_error_dialogs", lambda: None)
    monkeypatch.setattr(
        cli,
        "run",
        lambda request: (_ for _ in ()).throw(
            launch_context.Windows_Launch_Error(
                "windows_tool_initialization_failed",
                "tool exited with status -1073741502 (0xc0000142)",
            )
        ),
    )
    assert cli.main(["--mode", "offline", "--output", "output"]) == 2
    assert "-1073741502 (0xc0000142)" in capsys.readouterr().err
