"""SQLite persistence with append-only events and atomic state updates."""

from __future__ import annotations

import json
import sqlite3
from dataclasses import asdict
from datetime import datetime
from pathlib import Path
from typing import Any
from uuid import UUID

from .errors import StorageError
from .models import Event, Project, Run, RunStatus, Task, TaskStatus, TerminalLocation


class Storage:
    def __init__(self, path: Path):
        self.path = path
        self.path.parent.mkdir(parents=True, exist_ok=True)
        self.connection = sqlite3.connect(path)
        self.connection.row_factory = sqlite3.Row
        self.connection.execute("PRAGMA foreign_keys = ON")
        self.initialize()

    def initialize(self) -> None:
        self.connection.executescript(
            """
            CREATE TABLE IF NOT EXISTS projects (
                project_id TEXT PRIMARY KEY, root TEXT NOT NULL, created_at TEXT NOT NULL,
                execution_backend TEXT NOT NULL, terminal_backend TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS tasks (
                task_id TEXT PRIMARY KEY, project_id TEXT NOT NULL, title TEXT NOT NULL,
                description TEXT NOT NULL, status TEXT NOT NULL, created_at TEXT NOT NULL,
                metadata TEXT NOT NULL, FOREIGN KEY(project_id) REFERENCES projects(project_id)
            );
            CREATE TABLE IF NOT EXISTS runs (
                run_id TEXT PRIMARY KEY, task_id TEXT NOT NULL, project_id TEXT NOT NULL,
                harness TEXT NOT NULL, role TEXT NOT NULL, protocol TEXT NOT NULL,
                execution_backend TEXT NOT NULL, terminal_backend TEXT NOT NULL,
                status TEXT NOT NULL, branch TEXT, worktree TEXT, terminal TEXT,
                created_at TEXT NOT NULL, metadata TEXT NOT NULL,
                FOREIGN KEY(task_id) REFERENCES tasks(task_id), FOREIGN KEY(project_id) REFERENCES projects(project_id)
            );
            CREATE TABLE IF NOT EXISTS events (
                sequence INTEGER PRIMARY KEY AUTOINCREMENT, event_id TEXT UNIQUE NOT NULL,
                event_type TEXT NOT NULL, project_id TEXT NOT NULL, task_id TEXT,
                run_id TEXT, payload TEXT NOT NULL, timestamp TEXT NOT NULL, actor TEXT NOT NULL
            );
            """
        )
        self.connection.commit()

    def close(self) -> None:
        self.connection.close()

    def save_project(self, project: Project) -> None:
        self.connection.execute(
            "INSERT OR REPLACE INTO projects VALUES (?, ?, ?, ?, ?)",
            (str(project.project_id), project.root, project.created_at.isoformat(), project.execution_backend, project.terminal_backend),
        )
        self.connection.commit()

    def get_project(self, project_id: UUID) -> Project | None:
        row = self.connection.execute("SELECT * FROM projects WHERE project_id = ?", (str(project_id),)).fetchone()
        if not row:
            return None
        return Project(root=row["root"], project_id=UUID(row["project_id"]), created_at=datetime.fromisoformat(row["created_at"]), execution_backend=row["execution_backend"], terminal_backend=row["terminal_backend"])

    def save_task(self, task: Task, event: Event | None = None) -> None:
        try:
            with self.connection:
                self.connection.execute("INSERT OR REPLACE INTO tasks VALUES (?, ?, ?, ?, ?, ?, ?)", (str(task.task_id), str(task.project_id), task.title, task.description, task.status.value, task.created_at.isoformat(), json.dumps(task.metadata, ensure_ascii=False)))
                if event:
                    self._append_event(event)
        except sqlite3.Error as exc:
            raise StorageError(str(exc)) from exc

    def get_task(self, task_id: UUID) -> Task | None:
        row = self.connection.execute("SELECT * FROM tasks WHERE task_id = ?", (str(task_id),)).fetchone()
        if not row:
            return None
        return Task(title=row["title"], project_id=UUID(row["project_id"]), description=row["description"], task_id=UUID(row["task_id"]), status=TaskStatus(row["status"]), created_at=datetime.fromisoformat(row["created_at"]), metadata=json.loads(row["metadata"]))

    def list_tasks(self, project_id: UUID) -> list[Task]:
        return [task for row in self.connection.execute("SELECT task_id FROM tasks WHERE project_id = ? ORDER BY created_at", (str(project_id),)) if (task := self.get_task(UUID(row["task_id"]))) is not None]

    def save_run(self, run: Run, event: Event | None = None) -> None:
        terminal = json.dumps(asdict(run.terminal), ensure_ascii=False) if run.terminal else None
        with self.connection:
            self.connection.execute("INSERT OR REPLACE INTO runs VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)", (str(run.run_id), str(run.task_id), str(run.project_id), run.harness, run.role, run.protocol, run.execution_backend, run.terminal_backend, run.status.value, run.branch, run.worktree, terminal, run.created_at.isoformat(), json.dumps(run.metadata, ensure_ascii=False)))
            if event:
                self._append_event(event)

    def get_run(self, run_id: UUID) -> Run | None:
        row = self.connection.execute("SELECT * FROM runs WHERE run_id = ?", (str(run_id),)).fetchone()
        if not row:
            return None
        terminal_data = json.loads(row["terminal"]) if row["terminal"] else None
        return Run(task_id=UUID(row["task_id"]), project_id=UUID(row["project_id"]), harness=row["harness"], role=row["role"], protocol=row["protocol"], execution_backend=row["execution_backend"], terminal_backend=row["terminal_backend"], run_id=UUID(row["run_id"]), status=RunStatus(row["status"]), branch=row["branch"], worktree=row["worktree"], terminal=TerminalLocation(**terminal_data) if terminal_data else None, created_at=datetime.fromisoformat(row["created_at"]), metadata=json.loads(row["metadata"]))

    def list_runs(self, project_id: UUID, statuses: set[RunStatus] | None = None) -> list[Run]:
        query = "SELECT run_id FROM runs WHERE project_id = ?"
        args: list[Any] = [str(project_id)]
        if statuses:
            query += " AND status IN (" + ",".join("?" for _ in statuses) + ")"
            args.extend(status.value for status in statuses)
        query += " ORDER BY created_at"
        return [run for row in self.connection.execute(query, args) if (run := self.get_run(UUID(row["run_id"]))) is not None]

    def append_event(self, event: Event) -> Event:
        with self.connection:
            sequence = self._append_event(event)
        return Event(event_type=event.event_type, project_id=event.project_id, task_id=event.task_id, run_id=event.run_id, payload=event.payload, event_id=event.event_id, sequence=sequence, timestamp=event.timestamp, actor=event.actor)

    def _append_event(self, event: Event) -> int:
        cursor = self.connection.execute("INSERT INTO events(event_id,event_type,project_id,task_id,run_id,payload,timestamp,actor) VALUES (?, ?, ?, ?, ?, ?, ?, ?)", (str(event.event_id), event.event_type, str(event.project_id), str(event.task_id) if event.task_id else None, str(event.run_id) if event.run_id else None, json.dumps(event.payload, ensure_ascii=False), event.timestamp.isoformat(), event.actor))
        return int(cursor.lastrowid)

    def list_events(self, project_id: UUID) -> list[Event]:
        rows = self.connection.execute("SELECT * FROM events WHERE project_id = ? ORDER BY sequence", (str(project_id),))
        return [Event(event_type=row["event_type"], project_id=UUID(row["project_id"]), task_id=UUID(row["task_id"]) if row["task_id"] else None, run_id=UUID(row["run_id"]) if row["run_id"] else None, payload=json.loads(row["payload"]), event_id=UUID(row["event_id"]), sequence=row["sequence"], timestamp=datetime.fromisoformat(row["timestamp"]), actor=row["actor"]) for row in rows]
