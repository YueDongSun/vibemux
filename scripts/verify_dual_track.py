"""Dual-track acceptance runner (test driver; see scripts/dual_track_acceptance).

    python scripts/verify_dual_track.py --mode offline --output <owned-report-dir>
    python scripts/verify_dual_track.py --mode live --config <private-policy.json> \
        --live_opt_in --output <owned-report-dir>
    python scripts/verify_dual_track.py --validate <owned-report-dir>/dual_track_report.json

Exit codes: 0 PASS or OFFLINE_PASS (or a valid report), 1 FAIL (or an
invalid report), 2 BLOCKED, 3 usage error. An offline success is
OFFLINE_PASS, never full product acceptance.
"""

from __future__ import annotations

import argparse
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

from dual_track_acceptance.runner import (  # noqa: E402
    MODE_LIVE,
    MODE_OFFLINE,
    RunRequest,
    run,
    validate,
)
from dual_track_acceptance.runner_config import EXIT_USAGE  # noqa: E402


def parse_arguments(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--mode", choices=(MODE_OFFLINE, MODE_LIVE))
    parser.add_argument("--output", type=Path, help="new or empty directory outside the repository")
    parser.add_argument("--config", type=Path, help="private live policy JSON (live mode)")
    parser.add_argument(
        "--live_opt_in",
        action="store_true",
        help="explicitly allow a live run that may spend model budget",
    )
    parser.add_argument(
        "--validate", type=Path, help="re-derive a written report from its artifacts"
    )
    return parser.parse_args(argv)


def main(argv: list[str]) -> int:
    arguments = parse_arguments(argv)
    if arguments.validate is not None:
        if arguments.mode or arguments.output or arguments.config or arguments.live_opt_in:
            print("--validate takes no other option", file=sys.stderr)
            return EXIT_USAGE
        return validate(arguments.validate)
    if arguments.mode is None or arguments.output is None:
        print("--mode and --output are required", file=sys.stderr)
        return EXIT_USAGE
    if arguments.mode == MODE_OFFLINE and (arguments.config or arguments.live_opt_in):
        print("--config and --live_opt_in apply to live mode only", file=sys.stderr)
        return EXIT_USAGE
    return run(
        RunRequest(
            mode=arguments.mode,
            output=arguments.output,
            config=arguments.config,
            live_opt_in=arguments.live_opt_in,
            argv=[Path(sys.argv[0]).name, *argv],
        )
    )


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
