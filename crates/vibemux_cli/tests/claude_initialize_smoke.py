"""Exercise the built CLI, daemon, and selective cache producer on POSIX.

Build vibemuxctl, vibemuxd, vibemux_launch_trampoline, and vibemux_probe into
one directory, then run this file with that directory as its sole argument.
The only launcher on the probe's PATH is the controlled Claude fixture.
"""

from __future__ import annotations

import argparse
import json
import os
import sqlite3
import subprocess
import sys
import tempfile
import time
from concurrent.futures import ThreadPoolExecutor
from contextlib import closing
from pathlib import Path
from typing import Any

TIMEOUT_SECONDS = 30
VERSION = "1.2.3"
STDOUT_SENTINEL = "SYNTHETIC_CLAUDE_INITIALIZE_CONTENT"
STDERR_SENTINEL = "SYNTHETIC_CLAUDE_STDERR_CONTENT"
FIXTURE = f"""
import json
import sys
from pathlib import Path

if sys.argv[1:] == ["--version"]:
    print({VERSION!r})
    raise SystemExit(0)

for flag in ("--tools=", "--strict-mcp-config", "--restricted", "--no-session-persistence"):
    if flag not in sys.argv[1:]:
        raise SystemExit(2)

message = json.loads(sys.stdin.buffer.readline(131073))
if message.get("type") != "control_request" or message.get("request", {{}}).get("subtype") != "initialize":
    raise SystemExit(2)
print({STDERR_SENTINEL!r}, file=sys.stderr, flush=True)
print(json.dumps({{"type": "control_response", "response": {{
    "subtype": "success", "request_id": message["request_id"],
    "response": {{"fixture_content": {STDOUT_SENTINEL!r}}},
}}}}), flush=True)
if sys.stdin.buffer.read(1) != b"":
    raise SystemExit(2)
Path(__file__).with_name("initialize_eof").touch(exist_ok=False)
"""


def checked_json(result: subprocess.CompletedProcess[bytes], operation: str) -> dict[str, Any]:
    if result.returncode != 0:
        raise RuntimeError(f"{operation} failed with exit {result.returncode}")
    assert not result.stderr, f"{operation} unexpectedly emitted diagnostics"
    decoded = json.loads(result.stdout)
    assert isinstance(decoded, dict), f"{operation} did not return a JSON object"
    return decoded


def run_smoke(bin_dir: Path, scratch: Path) -> None:
    probe = bin_dir / "vibemux_probe"
    ctl = bin_dir / "vibemuxctl"
    daemon = bin_dir / "vibemuxd"
    trampoline = bin_dir / "vibemux_launch_trampoline"
    if not all(
        path.is_file() and os.access(path, os.X_OK) for path in (probe, ctl, daemon, trampoline)
    ):
        raise RuntimeError("build all four VibeMux binaries before running the smoke test")

    project = scratch / "project"
    runtime = project / ".vibemux"
    runtime.mkdir(parents=True, mode=0o700)
    launcher = scratch / "claude"
    launcher.write_text(f"#!{sys.executable} -I\n{FIXTURE}", encoding="utf-8")
    launcher.chmod(0o700)
    config = {
        "schema_version": 1,
        "limits": {"deadline_ms": 5_000},
        "routes": [
            {
                "harness": "claude",
                "protocol": "claude_stream_json",
                "executable": str(launcher),
                "enabled": True,
                "allow_execution": False,
            }
        ],
    }
    (runtime / "harness_dispatch.json").write_text(json.dumps(config), encoding="utf-8")
    environment = {"PATH": str(scratch), "TMPDIR": str(scratch)}
    if "HOME" in os.environ:
        environment["HOME"] = os.environ["HOME"]
    project_args = ("--project-root", str(project))

    def run(*arguments: str | Path) -> subprocess.CompletedProcess[bytes]:
        return subprocess.run(
            arguments,
            cwd=project,
            env=environment,
            stdin=subprocess.DEVNULL,
            capture_output=True,
            timeout=TIMEOUT_SECONDS,
            check=False,
        )

    def cli(*arguments: str) -> subprocess.CompletedProcess[bytes]:
        return run(ctl, "--format", "json", *arguments, *project_args)

    def command(*arguments: str) -> dict[str, Any]:
        return checked_json(cli(*arguments), arguments[0])

    version_report = checked_json(
        run(probe, "--version-only", "--harness", "claude", "--write-cache", *project_args),
        "version-only probe",
    )
    assert version_report["schema_version"] == 2
    assert version_report["coverage"] == "version_only"
    observation = version_report["observation"]
    assert observation["agent"] == "claude" and observation["launcher_state"] == "verified"
    assert observation["version"] == VERSION and observation["path"] == str(launcher)

    process = subprocess.Popen(
        (daemon, *project_args),
        cwd=project,
        env=environment,
        stdin=subprocess.DEVNULL,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    descriptor = runtime / "control.json"
    try:
        deadline = time.monotonic() + TIMEOUT_SECONDS
        while not descriptor.is_file():
            if process.poll() is not None or time.monotonic() >= deadline:
                raise RuntimeError("owned daemon did not publish its control descriptor")
            time.sleep(0.05)
        health = command("daemon", "health")
        assert health["healthy"] and health["process_id"] == process.pid
        sockets = list(scratch.glob("vibemux_ctl_*.sock"))
        assert len(sockets) == 1 and sockets[0].is_socket()
        unobserved = cli("dispatch", "probe", "claude")
        assert unobserved.returncode == 4
        assert json.loads(unobserved.stderr)["error_code"] == "harness_dispatch_not_detected"
        assert not (scratch / "initialize_eof").exists()
        rows = command("harnesses")["harnesses"]
        assert [(row["name"], row["version"]) for row in rows if row["available"]] == [
            ("claude", VERSION)
        ]
        assert command("tasks", "list")["tasks"] == []

        result = cli("dispatch", "probe", "claude")
        evidence = checked_json(result, "dispatch probe")["probe"]
        assert evidence["harness"] == "claude" and evidence["protocol"] == "claude_stream_json"
        assert evidence["outcome"] == "probed" and evidence["error_code"] is None, (
            evidence["outcome"],
            evidence["error_code"],
        )
        assert evidence["process"] == {
            "exit_code": 0,
            "forced_termination": False,
            "stderr_bytes": len(STDERR_SENTINEL) + 1,
        }
        assert evidence["capture"]["record_count"] == evidence["capture"]["kinds"]["opaque"] == 1
        for withheld in (STDOUT_SENTINEL, STDERR_SENTINEL, str(launcher)):
            assert withheld.encode() not in result.stdout
        assert (scratch / "initialize_eof").is_file(), (
            "fixture did not observe EOF after initialize"
        )
        assert command("tasks", "list")["tasks"] == []
    finally:
        if process.poll() is None:
            # The stop command waits for process exit. Reap our owned
            # foreground daemon in parallel so a zombie cannot hold it up.
            with ThreadPoolExecutor(max_workers=1) as pool:
                stop = pool.submit(command, "daemon", "stop")
                try:
                    assert process.wait(timeout=TIMEOUT_SECONDS) == 0, "daemon failed to drain"
                    assert stop.result()["status"] == "stopped"
                finally:
                    if process.poll() is None:
                        process.kill()
                        process.wait(timeout=TIMEOUT_SECONDS)

    assert not descriptor.exists() and not (runtime / "writer.lock").exists()
    assert not list(scratch.glob("vibemux_ctl_*.sock"))
    database_uri = (runtime / "vibemux_rust.sqlite3").as_uri()
    with closing(sqlite3.connect(f"{database_uri}?mode=ro", uri=True)) as database:
        assert database.execute(
            "SELECT COUNT(*) FROM projections WHERE entity_kind IN ('task', 'run')"
        ).fetchone() == (0,)
        assert database.execute(
            "SELECT COUNT(*) FROM events WHERE json_extract(envelope_json, '$.task_id') IS NOT NULL "
            "OR json_extract(envelope_json, '$.run_id') IS NOT NULL"
        ).fetchone() == (0,)
        probe_events = database.execute(
            "SELECT json_extract(envelope_json, '$.payload') FROM events "
            "WHERE json_extract(envelope_json, '$.event_type') = 'harness_probed'"
        ).fetchall()
        assert [json.loads(payload) for (payload,) in probe_events] == [
            {"detected": ["claude"], "missing": []}
        ]
        assert database.execute("SELECT COUNT(*) FROM harness_dispatches").fetchone() == (0,)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("bin_dir", type=Path)
    args = parser.parse_args()
    if os.name != "posix":
        parser.error("the controlled launcher requires POSIX executable scripts")
    with tempfile.TemporaryDirectory(prefix="vmx-", dir="/tmp") as directory:
        run_smoke(args.bin_dir.resolve(), Path(directory).resolve())
    print("Controlled Claude initialize-only probe passed with zero Tasks and Runs.")


if __name__ == "__main__":
    main()
