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

# Reviewed negative tests: synthetic values that exist to prove a rejection or
# redaction path (loopback userinfo on a reserved host, RFC 1918 and CGNAT
# samples for address-policy tests, fake credential shapes for redaction
# tests, and a redacted `C:\Users\...` path elision in the ledger). Bind each
# exception to the exact stripped source line, never to a whole file or rule.
negative_fixture_hashes = {
    ("crates/vibemux_a2a/src/lib.rs", "url_credentials"): (
        "11cc70cac01312d573a72bea4b4eb5738c620ab5c5f7b5ca2d7c14c10dd47414",
    ),
    ("crates/vibemux_a2a/tests/task_gateway.rs", "url_credentials"): (
        "6a6ede9760c2427b14f7ae839545c7010bf2c380d5ab8924d0a78ae48b63bdd1",
    ),
    ("crates/vibemux_a2a/tests/task_grpc.rs", "url_credentials"): (
        "91cd172aafe40360ea5dd9b56940f177b9562579f7feb33e2738ab3c2aa26e7f",
    ),
    ("crates/vibemux_decision_peer/src/jev_client.rs", "url_credentials"): (
        "79abed56570a1a9796d694a83a47ce7613f199543cf4f3447e7d83620d095a9d",
    ),
    ("crates/vibemux_web/src/push_tests.rs", "private_network_address"): (
        "08cd81b5ebc91f7e63cd2d00d312b80cf105d1b74fea20b286e697bd57233031",
        "0a9bab98b590e7cd1a0b7f66043e605c134ac792072ea9d74a07493f59ee1a80",
        "5b1f4af095eb41372e977823e256113b56fbf48c1eae2c74054554f19897ea6c",
        "aead42e64bef28efc05d6652e74387592b5448b018687e1332ce958036705b38",
        "dfd036046a554f2ad05de3ebe23d5343d717221d344d9d0bebd33a957071944e",
    ),
    ("crates/vibemux_workflow/src/context_bundle.rs", "credential_prefix"): (
        "51d9edf91144308238d56287e3a695654e754aa8b8fbc1bedb6e30f4d7d2fc74",
        "5c3b0c4682a3bf29a1afaf29f0bf8171b05ab9cfe106b0667c1b7e858ca17603",
    ),
    ("crates/vibemux_workflow/src/context_bundle.rs", "private_key"): (
        "20daa519dd95b2f92857be054cbdde51217b9390a0d7611b35ca08102189593b",
    ),
    ("crates/vibemux_workflow/src/gateway.rs", "private_network_address"): (
        "75ff24d851a2d7eb5b1449567f36e6921692bf08f10a9570ff498f6fafbcaa02",
    ),
    ("PROGRESS.md", "personal_home"): (
        "95342d91633a1583bdeae6d3a51bf4901779d867b8744ca6ffd1cf1e83334dc3",
    ),
}


@dataclass(frozen=True)
class Finding:
    path: str
    line: int
    rule: str


def is_negative_fixture(finding: Finding, line: str) -> bool:
    expected = negative_fixture_hashes.get((finding.path, finding.rule), ())
    return hashlib.sha256(line.strip().encode("utf-8")).hexdigest() in expected


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
