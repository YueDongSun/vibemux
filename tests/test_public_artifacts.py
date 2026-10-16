from pathlib import Path

from scripts.check_public_artifacts import Finding, inspect_text, is_negative_fixture


def test_private_values_are_flagged_without_being_returned() -> None:
    samples = [
        ("credential_prefix", "sk-" + "a" * 30),
        ("personal_home", "/home/" + "private_account/workspace"),
        ("personal_home", "C:\\Users\\" + "private_account\\workspace"),
        ("private_network_address", "100." + "70.8.9"),
        ("tailnet_domain", "private_node." + "sample.ts" + ".net"),
        ("url_credentials", "https://" + "account:credential@example.com"),
    ]
    for expected, value in samples:
        findings = inspect_text("example.json", value)
        assert [finding.rule for finding in findings] == [expected]
        assert value not in repr(findings)


def test_public_placeholders_and_loopback_are_allowed() -> None:
    public = "\n".join(
        [
            "https://example.com/api",
            "127.0.0.1",
            "192.0.2.10",
            "203.0.113.10",
            "./workspace",
            "/home/" + "example/workspace",
            "SYNTHETIC_SECRET_TOKEN",
        ]
    )
    assert inspect_text("example.json", public) == []


def test_negative_fixture_exception_is_bound_to_content_and_path() -> None:
    relative = "crates/vibemux_a2a/src/lib.rs"
    source = (Path(__file__).resolve().parents[1] / relative).read_text(encoding="utf-8")
    lines = source.splitlines()
    findings = [
        finding for finding in inspect_text(relative, source) if finding.rule == "url_credentials"
    ]
    assert len(findings) == 1
    finding = findings[0]
    line = lines[finding.line - 1]
    assert is_negative_fixture(finding, line)
    assert not is_negative_fixture(finding, line + " changed")
    assert not is_negative_fixture(Finding("different.rs", finding.line, finding.rule), line)
