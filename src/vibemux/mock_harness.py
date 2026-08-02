"""Deterministic offline harness used by smoke tests."""

from __future__ import annotations

import os
import sys
from pathlib import Path


def main() -> int:
    cwd = Path.cwd().resolve()
    print(f"VibeMux mock harness ready ({os.environ.get('VIBEMUX_RUN_ID', 'unknown')})", flush=True)
    for raw in sys.stdin:
        line = raw.rstrip("\r\n")
        parts = line.split(" ", 1)
        command = parts[0].upper() if parts else ""
        argument = parts[1] if len(parts) == 2 else ""
        if command == "PING":
            print("PONG", flush=True)
        elif command == "ECHO":
            print(argument, flush=True)
        elif command == "WRITE":
            target = (cwd / argument).resolve()
            if Path(argument).is_absolute() or ".." in Path(argument).parts or os.path.commonpath([str(target), str(cwd)]) != str(cwd):
                print("ERROR unsafe path", flush=True)
                continue
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_text("written by mock harness\n", encoding="utf-8")
            print(f"WROTE {target.relative_to(cwd)}", flush=True)
        elif command == "EXIT":
            return int(argument or "0")
        elif line:
            print(f"UNKNOWN {line}", flush=True)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

