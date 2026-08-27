"""Check publishable source files without printing matched private values.

This deterministic gate supplements human review; it is not a secret scanner
with complete coverage. Ignored runtime data is never opened or exported.
"""

from __future__ import annotations

import argparse
import hashlib
import ipaddress
import re
import subprocess
from dataclasses import dataclass
from pathlib import Path

maximum_file_bytes = 2 * 1024 * 1024
text_suffixes = {".rs", ".py", ".md", ".json", ".jsonl", ".toml", ".yaml", ".yml", ".ps1"}
private_networks = tuple(
    ipaddress.ip_network(value)
    for value in (
        "10.0.0.0/8",
        "172.16.0.0/12",
        "192.168.0.0/16",
        "100.64.0.0/10",
    )
)
ipv4_pattern = re.compile(r"(?<![\w.])(?:\d{1,3}\.){3}\d{1,3}(?![\w.])")
content_rules = {
    "credential_prefix": re.compile(
        r"\b(?:sk-[A-Za-z0-9_-]{24,}|ghp_[A-Za-z0-9]{24,}|github_pat_[A-Za-z0-9_]{24,}|xox[baprs]-[A-Za-z0-9-]{20,})"
    ),
    "private_key": re.compile(r"-----BEGIN (?:RSA |EC |OPENSSH )?PRIVATE KEY-----"),
    "tailnet_domain": re.compile(r"\b[\w.-]+\.ts\.net\b", re.IGNORECASE),
    "personal_home": re.compile(
        r"(?:[A-Za-z]:[\\/]+Users[\\/]+|/(?:Users|home)/)"
        r"(?!(?:example|user|username|runner|runneradmin|public|test)(?:[\\/]|\b))"
        r"[A-Za-z0-9_.-]+[\\/]",
        re.IGNORECASE,
    ),
    "url_credentials": re.compile(r"\bhttps?://[^\s/@:]+:[^\s/@]+@", re.IGNORECASE),
}

# Reviewed negative test: loopback-looking userinfo on a reserved .example host.
# Bind the exception to exact source bytes, never to the whole file or rule.
negative_fixture_hashes = {
    (
        "crates/vibemux_a2a/src/lib.rs",
        "url_credentials",
    ): "11cc70cac01312d573a72bea4b4eb5738c620ab5c5f7b5ca2d7c14c10dd47414",
}


@dataclass(frozen=True)
class Finding:
    path: str
    line: int
    rule: str


def is_negative_fixture(finding: Finding, line: str) -> bool:
    expected = negative_fixture_hashes.get((finding.path, finding.rule))
    return (
        expected is not None
        and hashlib.sha256(line.strip().encode("utf-8")).hexdigest() == expected
    )


def inspect_text(path: str, content: str) -> list[Finding]:
    findings = []
    for line_number, line in enumerate(content.splitlines(), start=1):
        for name, pattern in content_rules.items():
            if pattern.search(line):
                findings.append(Finding(path, line_number, name))
        for match in ipv4_pattern.finditer(line):
            try:
                address = ipaddress.ip_address(match.group())
            except ValueError:
                continue
            if any(address in network for network in private_networks):
                # Network definitions in this scanner are public rule data.
                if path == "scripts/check_public_artifacts.py":
                    continue
                findings.append(Finding(path, line_number, "private_network_address"))
    return findings


def collect_paths(project_root: Path) -> list[Path]:
    result = subprocess.run(
        ["git", "ls-files", "--cached", "--others", "--exclude-standard", "-z"],
        cwd=project_root,
        capture_output=True,
        check=True,
    )
    names = sorted(set(result.stdout.decode("utf-8").split("\0")) - {""})
    paths = []
    for name in names:
        path = project_root / name
        if path.suffix.lower() not in text_suffixes or not path.is_file():
            continue
        if path.is_symlink() or not path.resolve().is_relative_to(project_root):
            raise ValueError("publishable_path_outside_project")
        paths.append(path)
    return paths


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--project_root", type=Path, default=Path(__file__).resolve().parents[1])
    args = parser.parse_args()
    project_root = args.project_root.resolve()
    findings = []
    negative_fixtures = 0
    paths = collect_paths(project_root)
    for path in paths:
        relative = path.relative_to(project_root).as_posix()
        if path.stat().st_size > maximum_file_bytes:
            findings.append(Finding(relative, 1, "manual_review_file_too_large"))
            continue
        try:
            content = path.read_text(encoding="utf-8-sig")
        except UnicodeDecodeError:
            findings.append(Finding(relative, 1, "manual_review_not_utf8"))
            continue
        lines = content.splitlines()
        for finding in inspect_text(relative, content):
            if is_negative_fixture(finding, lines[finding.line - 1]):
                negative_fixtures += 1
            else:
                findings.append(finding)
    for finding in findings:
        print(f"{finding.path}:{finding.line}: {finding.rule}")
    print(
        f"Checked {len(paths)} publishable text files; {len(findings)} findings; "
        f"{negative_fixtures} exact reviewed negative fixtures."
    )
    return 1 if findings else 0


if __name__ == "__main__":
    raise SystemExit(main())
