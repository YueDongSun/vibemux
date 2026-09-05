"""Typer CLI; services own application behavior."""

from __future__ import annotations

import json
import platform
from pathlib import Path
from uuid import UUID

import typer
from rich.console import Console
from rich.table import Table

from . import __version__
from .config import config_path, database_path, require_config
from .errors import VibeMuxError
from .services import (
    HarnessService,
    ProjectService,
    RunService,
    TaskService,
    choose_terminal_backend,
)

app = typer.Typer(no_args_is_help=True, add_completion=False, invoke_without_command=True)
console = Console()


def root() -> Path:
    return Path.cwd().resolve()


@app.callback()
def callback(version: bool = typer.Option(False, "--version", is_eager=True)) -> None:
    if version:
        typer.echo(__version__)
        raise typer.Exit()


@app.command()
def init(terminal_backend: str = typer.Option("auto", "--terminal-backend")) -> None:
    config = ProjectService(root()).initialize(terminal_backend)
    typer.echo(f"initialized {config.project_id} ({config.terminal_backend})")


@app.command()
def doctor(as_json: bool = typer.Option(False, "--json")) -> None:
    repo_root = root()
    checks = {
        "platform": platform.system(),
        "python": platform.python_version(),
        "git": "required",
        "config": config_path(repo_root).exists(),
        "database": database_path(repo_root).exists(),
    }
    try:
        config = require_config(repo_root)
        checks["terminal_backend"] = config.terminal_backend
        checks["default_harness"] = config.default_harness
        checks["terminal_available"] = choose_terminal_backend(config.terminal_backend).probe()
    except VibeMuxError as exc:
        checks["error"] = str(exc)
    if as_json:
        typer.echo(json.dumps(checks, ensure_ascii=False, indent=2))
        return
    table = Table("component", "status")
    for key, value in checks.items():
        table.add_row(key, str(value))
    console.print(table)


@app.command()
def status(as_json: bool = typer.Option(False, "--json")) -> None:
    service = RunService(root())
    runs = service.storage.list_runs(service.project_id)
    rows = [
        {
            "run_id": str(run.run_id),
            "task_id": str(run.task_id),
            "status": run.status.value,
            "harness": run.harness,
            "role": run.role.value,
            "branch": run.branch,
            "pane": run.terminal.pane_id if run.terminal else None,
        }
        for run in runs
    ]
    if as_json:
        typer.echo(json.dumps(rows, ensure_ascii=False, indent=2))
        return
    table = Table("run_id", "task_id", "status", "harness", "role", "branch", "pane")
    for row in rows:
        table.add_row(
            *(
                str(row[k])
                for k in ("run_id", "task_id", "status", "harness", "role", "branch", "pane")
            )
        )
    console.print(table)


@app.command("task")
def task(title: str, description: str = typer.Option("", "--description")) -> None:
    service = TaskService(root())
    item = service.create(title, description)
    typer.echo(str(item.task_id))


@app.command()
def tasks(as_json: bool = typer.Option(False, "--json")) -> None:
    rows = [
        {"task_id": str(t.task_id), "title": t.title, "status": t.status.value}
        for t in TaskService(root()).list()
    ]
    typer.echo(
        json.dumps(rows, ensure_ascii=False, indent=2)
        if as_json
        else "\n".join(f"{r['task_id']} {r['status']} {r['title']}" for r in rows)
    )


@app.command()
def spawn(
    task_id: str,
    harness: str | None = typer.Option(
        None,
        "--harness",
        help="Harness name; defaults to the project default (see 'vibemux switch'). Must be detected locally.",
    ),
    role: str = typer.Option(
        "worker", "--role", help="Run role: worker, reviewer, or orchestrator."
    ),
    terminal_backend: str | None = typer.Option(None, "--terminal-backend"),
) -> None:
    run = RunService(root()).spawn(
        UUID(task_id), harness, role=role, terminal_backend=terminal_backend
    )
    typer.echo(str(run.run_id))


@app.command("harnesses")
def harnesses(
    cached: bool = typer.Option(
        False, "--cached", help="Read the persisted snapshot instead of probing now."
    ),
    as_json: bool = typer.Option(False, "--json"),
) -> None:
    service = HarnessService(root())
    rows = service.cached() if cached else service.refresh()
    if as_json:
        typer.echo(json.dumps(rows, ensure_ascii=False, indent=2))
        return
    table = Table(
        "default", "name", "command", "protocol", "provider", "available", "roles", "path"
    )
    for row in rows:
        command = row["command"]
        command_text = " ".join(command) if isinstance(command, list) else str(command)
        roles = row["roles"]
        roles_text = (
            ",".join(str(item) for item in roles) if isinstance(roles, list) else str(roles or "")
        )
        table.add_row(
            "*" if row["default"] else "",
            str(row["name"]),
            command_text,
            str(row["protocol"]),
            str(row["provider"] or ""),
            "yes" if row["available"] else "no",
            roles_text,
            str(row["path"] or ""),
        )
    console.print(table)


@app.command()
def switch(harness: str) -> None:
    config = HarnessService(root()).switch(harness)
    typer.echo(f"default harness -> {config.default_harness}")


@app.command()
def send(run_id: str, text: str, submit: bool = typer.Option(False, "--submit")) -> None:
    receipt = RunService(root()).send(UUID(run_id), text, submit)
    typer.echo(json.dumps(receipt.__dict__, ensure_ascii=False))


@app.command()
def diff(run_id: str) -> None:
    typer.echo(RunService(root()).diff(UUID(run_id)))


@app.command()
def trace() -> None:
    for event in RunService(root()).trace():
        typer.echo(
            json.dumps(
                {
                    "sequence": event.sequence,
                    "type": event.event_type,
                    "task": str(event.task_id) if event.task_id else None,
                    "run": str(event.run_id) if event.run_id else None,
                    "payload": event.payload,
                },
                ensure_ascii=False,
            )
        )


@app.command()
def stop(run_id: str) -> None:
    typer.echo(RunService(root()).stop(UUID(run_id)).status.value)


def main() -> None:
    try:
        app()
    except VibeMuxError as exc:
        console.print(f"[red]error:[/red] {exc}")
        # typer.Exit raised here would escape click's handler and print a traceback.
        raise SystemExit(4) from exc


if __name__ == "__main__":
    main()
