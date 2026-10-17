"""Contract tests for ``scripts/setup_dsh_prereq.ps1``.

The script decides whether a machine can drive the DeepSeek Harness (DSH)
runtime and its interactive ``tui`` profile. Running it against the real
machine would make the suite depend on an installed DSH, a network install,
and the operator's home directory, so every test drives it against a fabricated
npm prefix and a fabricated DSH home with ``-SkipProbes``, which starts no
child process at all.

The script is Windows-first PowerShell, so a host without ``pwsh`` skips these
tests with an explicit reason; a skip is not a pass.
"""

from __future__ import annotations

import json
import shutil
import subprocess
from pathlib import Path
from typing import Any

import pytest

REPO_ROOT = Path(__file__).resolve().parents[1]
SCRIPT = REPO_ROOT / "scripts" / "setup_dsh_prereq.ps1"
PWSH = shutil.which("pwsh")

pytestmark = pytest.mark.skipif(
    PWSH is None or not SCRIPT.is_file(),
    reason="requires PowerShell 7 (pwsh) and scripts/setup_dsh_prereq.ps1",
)

DSH_PACKAGE = "@deepseek-ai/dsh"
READY_EXIT = 0
NOT_READY_EXIT = 2


def pwsh_executable() -> str:
    """Return the PowerShell path; the module is already gated on it existing."""
    assert PWSH is not None
    return PWSH


def build_npm_prefix(root: Path, version: str = "0.2.0-rc.2", name: str = DSH_PACKAGE) -> Path:
    """Create a minimal npm prefix carrying one ``@deepseek-ai/dsh`` install.

    A real global prefix also carries npm and pnpm; the script treats both as
    required, so the stub entries keep the fixture honest.
    """
    package = root / "node_modules" / "@deepseek-ai" / "dsh"
    (package / "lib").mkdir(parents=True, exist_ok=True)
    (package / "lib" / "bin.js").write_text("// fixture entry\n", encoding="utf-8")
    (package / "package.json").write_text(
        json.dumps(
            {
                "name": name,
                "version": version,
                "bin": {"dsh": "lib/bin.js"},
            }
        ),
        encoding="utf-8",
    )
    npm_cli = root / "node_modules" / "npm" / "bin"
    npm_cli.mkdir(parents=True, exist_ok=True)
    (npm_cli / "npm-cli.js").write_text("// fixture npm\n", encoding="utf-8")
    pnpm_bin = root / "node_modules" / "pnpm" / "bin"
    pnpm_bin.mkdir(parents=True, exist_ok=True)
    (pnpm_bin / "pnpm.cjs").write_text("// fixture pnpm\n", encoding="utf-8")
    return root


def build_profile(home: Path, profile: str, bundles: list[str]) -> None:
    """Create one profile manifest the way ``dsh plugin`` writes it."""
    directory = home / "profiles" / profile
    directory.mkdir(parents=True, exist_ok=True)
    (directory / "package.json").write_text(
        json.dumps(
            {
                "name": f"dsh-profile-{profile}",
                "private": True,
                "dsh": {"profile": {"bundles": bundles, "patchReload": "live"}},
            }
        ),
        encoding="utf-8",
    )


def run_script(*args: str, timeout: int = 120) -> subprocess.CompletedProcess[str]:
    """Run the prerequisite script once and capture its streams."""
    return subprocess.run(
        [pwsh_executable(), "-NoProfile", "-NonInteractive", "-File", str(SCRIPT), *args],
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        timeout=timeout,
        check=False,
    )


def run_prereq(
    tmp_path: Path,
    *extra: str,
    npm_prefix: Path | None = None,
    dsh_home: Path | None = None,
) -> tuple[int, dict[str, Any]]:
    """Run the script in its probe-free, read-only mode and parse its JSON report."""
    npm_prefix = npm_prefix if npm_prefix is not None else tmp_path / "npm"
    dsh_home = dsh_home if dsh_home is not None else tmp_path / "dsh-home"
    project = tmp_path / "project"
    project.mkdir(exist_ok=True)
    completed = run_script(
        "-Json",
        "-SkipProbes",
        "-NpmPrefix",
        str(npm_prefix),
        "-DshHome",
        str(dsh_home),
        "-ProjectRoot",
        str(project),
        *extra,
    )
    assert completed.returncode in (READY_EXIT, NOT_READY_EXIT), completed.stderr
    report: dict[str, Any] = json.loads(completed.stdout)
    return completed.returncode, report


def check_by_id(report: dict[str, Any], check_id: str) -> dict[str, Any]:
    """Return one named check from a report."""
    matches = [check for check in report["checks"] if check["id"] == check_id]
    assert matches, f"no check named {check_id!r} in {[c['id'] for c in report['checks']]}"
    result: dict[str, Any] = matches[0]
    return result


def test_missing_install_is_not_ready_and_names_the_install_command(tmp_path: Path) -> None:
    code, report = run_prereq(tmp_path)

    assert code == NOT_READY_EXIT
    assert report["ok"] is False
    assert report["mode"] == "check"
    install = check_by_id(report, "dsh-install")
    assert install["status"] == "missing"
    assert install["severity"] == "required"
    assert "npm install -g @deepseek-ai/dsh" in install["remediation"]
    assert check_by_id(report, "dsh-version")["status"] == "unknown"


def test_install_below_the_supported_version_is_stale(tmp_path: Path) -> None:
    prefix = build_npm_prefix(tmp_path / "npm", version="0.1.2-rc.1")
    code, report = run_prereq(tmp_path, npm_prefix=prefix)

    assert code == NOT_READY_EXIT
    assert report["ok"] is False
    assert check_by_id(report, "dsh-install")["status"] == "ok"
    version = check_by_id(report, "dsh-version")
    assert version["status"] == "stale"
    assert "0.1.2-rc.1" in version["detail"]
    assert "0.2.0-rc.2" in version["remediation"]


@pytest.mark.parametrize(
    ("version", "expected"),
    [
        ("0.2.0-rc.2", "ok"),
        ("0.2.1-alpha.1", "ok"),
        ("0.2.0-rc.10", "ok"),
        ("0.3.0", "ok"),
        ("0.2.0-rc.1", "stale"),
        ("0.1.9", "stale"),
        ("0.2.0-beta.1", "stale"),
    ],
)
def test_prerelease_aware_version_comparison(tmp_path: Path, version: str, expected: str) -> None:
    """Semantic-version ordering, including prerelease identifiers."""
    prefix = build_npm_prefix(tmp_path / "npm", version=version)
    _, report = run_prereq(tmp_path, npm_prefix=prefix)

    assert check_by_id(report, "dsh-version")["status"] == expected


def test_foreign_package_with_the_same_layout_is_not_trusted(tmp_path: Path) -> None:
    """The adapter's trust check is mirrored: the manifest must name the package."""
    prefix = build_npm_prefix(tmp_path / "npm", name="@example/dsh")
    code, report = run_prereq(tmp_path, npm_prefix=prefix)

    assert code == NOT_READY_EXIT
    install = check_by_id(report, "dsh-install")
    assert install["status"] == "missing"
    assert "@example/dsh" in install["detail"]


def test_ready_runtime_reports_ok_and_resolves_the_session_paths(tmp_path: Path) -> None:
    prefix = build_npm_prefix(tmp_path / "npm")
    home = tmp_path / "dsh-home"
    build_profile(home, "sdk", ["@deepseek-ai/dsh-base", "@deepseek-ai/dsh-sdk-app"])
    build_profile(
        home,
        "tui",
        ["@deepseek-ai/dsh-base", "@deepseek-harness-tui/dsh-tui"],
    )
    code, report = run_prereq(tmp_path, npm_prefix=prefix, dsh_home=home)

    assert code == READY_EXIT
    assert report["ok"] is True
    assert check_by_id(report, "dsh-sdk-profile")["status"] == "ok"
    assert check_by_id(report, "dsh-tui-profile")["status"] == "ok"
    assert report["dsh"]["version"] == "0.2.0-rc.2"
    entry_script = Path(report["dsh"]["entry_script"])
    assert entry_script.name == "bin.js"
    assert entry_script.parent.name == "lib"
    template = report["session_config"]["template"]
    assert template["schema_version"] == 1
    assert template["entry_script"] == report["dsh"]["entry_script"]
    assert Path(template["executable"]).name.lower().startswith("node")


def test_tui_profile_without_the_bundle_is_missing(tmp_path: Path) -> None:
    prefix = build_npm_prefix(tmp_path / "npm")
    home = tmp_path / "dsh-home"
    build_profile(home, "sdk", ["@deepseek-ai/dsh-base", "@deepseek-ai/dsh-sdk-app"])
    build_profile(home, "tui", ["@deepseek-ai/dsh-base"])
    code, report = run_prereq(tmp_path, npm_prefix=prefix, dsh_home=home)

    assert code == NOT_READY_EXIT
    profile = check_by_id(report, "dsh-tui-profile")
    assert profile["status"] == "missing"
    assert "dsh plugin --profile tui add" in profile["remediation"]


def test_skip_tui_does_not_require_the_terminal_ui(tmp_path: Path) -> None:
    prefix = build_npm_prefix(tmp_path / "npm")
    home = tmp_path / "dsh-home"
    build_profile(home, "sdk", ["@deepseek-ai/dsh-base", "@deepseek-ai/dsh-sdk-app"])
    code, report = run_prereq(tmp_path, "-SkipTui", npm_prefix=prefix, dsh_home=home)

    assert code == READY_EXIT
    assert report["ok"] is True
    assert check_by_id(report, "dsh-tui-profile")["severity"] == "advisory"


def test_check_mode_is_read_only(tmp_path: Path) -> None:
    """The default mode must not create, write, or delete anything."""
    prefix = tmp_path / "npm"
    prefix.mkdir()
    home = tmp_path / "dsh-home"
    home.mkdir()
    (tmp_path / "project").mkdir()
    before = sorted(path.relative_to(tmp_path).as_posix() for path in tmp_path.rglob("*"))

    run_prereq(tmp_path, npm_prefix=prefix, dsh_home=home)

    after = sorted(path.relative_to(tmp_path).as_posix() for path in tmp_path.rglob("*"))
    assert after == before


def test_session_config_is_reported_present_and_valid(tmp_path: Path) -> None:
    prefix = build_npm_prefix(tmp_path / "npm")
    home = tmp_path / "dsh-home"
    build_profile(home, "sdk", ["@deepseek-ai/dsh-base", "@deepseek-ai/dsh-sdk-app"])
    project = tmp_path / "project"
    (project / ".vibemux").mkdir(parents=True)
    (project / ".vibemux" / "dsh_session.json").write_text(
        json.dumps(
            {
                "schema_version": 1,
                "executable": shutil.which("node") or "node",
                "entry_script": str(
                    prefix / "node_modules" / "@deepseek-ai" / "dsh" / "lib" / "bin.js"
                ),
                "provider": "deepseek-official",
                "model": "deepseek-v4-flash",
            }
        ),
        encoding="utf-8",
    )
    completed = run_script(
        "-Json",
        "-SkipProbes",
        "-SkipTui",
        "-NpmPrefix",
        str(prefix),
        "-DshHome",
        str(home),
        "-ProjectRoot",
        str(project),
    )
    assert completed.returncode == READY_EXIT, completed.stderr
    report = json.loads(completed.stdout)

    assert report["session_config"]["present"] is True
    assert check_by_id(report, "vibemux-session-config")["status"] == "ok"


def test_apply_writes_the_session_config_without_a_bom(tmp_path: Path) -> None:
    """-Apply -WriteSessionConfig must produce a BOM-free schema-1 document.

    This covers the write path itself: a byte-order mark would make the document
    invalid JSON, and PowerShell variable names are case-insensitive, so a local
    colliding with the [switch]$Json parameter aborts the write.
    """
    prefix = build_npm_prefix(tmp_path / "npm")
    home = tmp_path / "dsh-home"
    build_profile(home, "sdk", ["@deepseek-ai/dsh-base", "@deepseek-ai/dsh-sdk-app"])
    build_profile(home, "tui", ["@deepseek-ai/dsh-base", "@deepseek-harness-tui/dsh-tui"])
    project = tmp_path / "project"
    project.mkdir()

    completed = run_script(
        "-Json",
        "-Apply",
        "-WriteSessionConfig",
        "-NpmPrefix",
        str(prefix),
        "-DshHome",
        str(home),
        "-ProjectRoot",
        str(project),
    )

    assert completed.returncode == READY_EXIT, completed.stderr
    report = json.loads(completed.stdout)
    assert report["mode"] == "apply"
    assert report["session_config"]["present"] is True

    written = project / ".vibemux" / "dsh_session.json"
    raw = written.read_bytes()
    assert not raw.startswith(b"\xef\xbb\xbf"), "a BOM makes the document invalid JSON"
    document = json.loads(raw.decode("utf-8"))
    assert document["schema_version"] == 1
    assert document["profile"] == "sdk"
    assert Path(document["entry_script"]).name == "bin.js"

    rewritten = run_script(
        "-Json",
        "-Apply",
        "-WriteSessionConfig",
        "-NpmPrefix",
        str(prefix),
        "-DshHome",
        str(home),
        "-ProjectRoot",
        str(project),
    )
    assert rewritten.returncode == READY_EXIT, rewritten.stderr
    assert json.loads(rewritten.stdout)["applied"] == [] or any(
        "unchanged" in line for line in json.loads(rewritten.stdout)["applied"]
    )
    assert written.read_bytes() == raw, "an existing configuration must survive without -Force"


def test_apply_cannot_be_combined_with_skip_probes(tmp_path: Path) -> None:
    """-Apply runs installers, so a probe-free apply must fail closed."""
    completed = run_script(
        "-Apply",
        "-SkipProbes",
        "-NpmPrefix",
        str(tmp_path / "npm"),
        "-DshHome",
        str(tmp_path / "dsh-home"),
        "-ProjectRoot",
        str(tmp_path),
    )

    assert completed.returncode == 1
    assert "-SkipProbes" in ((completed.stderr or "") + (completed.stdout or ""))
