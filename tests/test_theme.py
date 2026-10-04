"""Theme palette loading, resolution, and terminal snippet generation."""

from __future__ import annotations

import json
import os
import subprocess
import sys
import tomllib
from pathlib import Path

import pytest
from rich.style import Style

from vibemux.terminal_theme import (
    render_tmux_theme,
    render_wezterm_scheme,
    xterm256_nearest,
    xterm256_palette,
)
from vibemux.theme import (
    DEFAULT_THEME_NAME,
    PALETTE_FILE_CANDIDATES,
    PALETTE_FILE_NAME,
    THEME_ENV_VAR,
    ThemePaletteError,
    _fallback_palette,
    active_palette,
    load_palettes,
    palette_file_path,
    parse_hex_rgb,
    resolve_palette,
    rich_theme,
)

REPO_ROOT = Path(__file__).resolve().parents[1]


def write_palette_file(tmp_path: Path, payload: object) -> Path:
    path = tmp_path / "theme_palettes.json"
    path.write_text(json.dumps(payload), encoding="utf-8")
    return path


def run_cli(
    *arguments: str, env_extra: dict[str, str] | None = None
) -> subprocess.CompletedProcess[str]:
    environment = os.environ.copy()
    environment.pop(THEME_ENV_VAR, None)
    if env_extra:
        environment.update(env_extra)
    return subprocess.run(
        [sys.executable, "-m", "vibemux.cli", *arguments],
        capture_output=True,
        text=True,
        env=environment,
        check=False,
    )


def test_repository_palette_file_loads_seven_themes() -> None:
    palettes = load_palettes()
    assert list(palettes) == [
        "claude",
        "github",
        "vscode",
        "nord",
        "gruvbox",
        "studio",
        "claude_light",
    ]
    assert palettes["nord"].accent == "#88C0D0"
    assert palettes["nord"].terminal_bg == "#242933"
    assert palettes["gruvbox"].accent == "#FE8019"
    assert palettes["gruvbox"].terminal_bg == "#1D2021"
    assert palettes["claude_light"].bg == "#FAF9F5"


def test_fallback_palette_matches_the_exported_default() -> None:
    assert _fallback_palette() == load_palettes()[DEFAULT_THEME_NAME]


def test_palette_file_path_prefers_first_existing_candidate(tmp_path: Path) -> None:
    packaged = tmp_path / "package" / PALETTE_FILE_NAME
    checkout = tmp_path / "config" / PALETTE_FILE_NAME
    assert palette_file_path((packaged, checkout)) == checkout
    checkout.parent.mkdir()
    checkout.write_text("{}", encoding="utf-8")
    assert palette_file_path((packaged, checkout)) == checkout
    packaged.parent.mkdir()
    packaged.write_text("{}", encoding="utf-8")
    assert palette_file_path((packaged, checkout)) == packaged


def test_wheel_force_includes_the_palette_the_package_expects() -> None:
    pyproject = tomllib.loads((REPO_ROOT / "pyproject.toml").read_text(encoding="utf-8"))
    force_include = pyproject["tool"]["hatch"]["build"]["targets"]["wheel"]["force-include"]
    assert force_include["config/theme_palettes.json"] == f"vibemux/{PALETTE_FILE_NAME}"
    assert (REPO_ROOT / "config" / "theme_palettes.json").is_file()
    assert PALETTE_FILE_CANDIDATES[0].parent.name == "vibemux"
    assert PALETTE_FILE_CANDIDATES[-1] == REPO_ROOT / "config" / PALETTE_FILE_NAME


def test_parse_hex_rgb_accepts_and_rejects() -> None:
    assert parse_hex_rgb("#88C0D0") == (0x88, 0xC0, 0xD0)
    assert parse_hex_rgb("#d97757") == (0xD9, 0x77, 0x57)
    assert parse_hex_rgb("88C0D0") is None
    assert parse_hex_rgb("#88C0D") is None
    assert parse_hex_rgb("#88C0DG") is None
    assert parse_hex_rgb("") is None


def test_load_rejects_missing_file(tmp_path: Path) -> None:
    with pytest.raises(ThemePaletteError, match="not found"):
        load_palettes(tmp_path / "missing.json")


def test_load_rejects_wrong_schema_version(tmp_path: Path) -> None:
    path = write_palette_file(
        tmp_path,
        {
            "schema_version": 9999,
            "palettes": [
                {
                    "name": "claude",
                    **{
                        field: "#000000"
                        for field in (
                            "bg",
                            "surface",
                            "surface_alt",
                            "border",
                            "text_primary",
                            "text_muted",
                            "accent",
                            "accent_alt",
                            "success",
                            "warning",
                            "danger",
                            "terminal_bg",
                            "terminal_fg",
                            "terminal_cursor",
                        )
                    },
                }
            ],
        },
    )
    with pytest.raises(ThemePaletteError, match="schema_version"):
        load_palettes(path)


def test_load_rejects_invalid_hex(tmp_path: Path) -> None:
    base = {
        field: "#000000"
        for field in (
            "bg",
            "surface",
            "surface_alt",
            "border",
            "text_primary",
            "text_muted",
            "accent",
            "accent_alt",
            "success",
            "warning",
            "danger",
            "terminal_bg",
            "terminal_fg",
            "terminal_cursor",
        )
    }
    base["accent"] = "not-a-color"
    path = write_palette_file(
        tmp_path, {"schema_version": 1, "palettes": [{"name": "claude", **base}]}
    )
    with pytest.raises(ThemePaletteError, match="accent"):
        load_palettes(path)


def test_load_rejects_duplicate_names(tmp_path: Path) -> None:
    base = {
        field: "#000000"
        for field in (
            "bg",
            "surface",
            "surface_alt",
            "border",
            "text_primary",
            "text_muted",
            "accent",
            "accent_alt",
            "success",
            "warning",
            "danger",
            "terminal_bg",
            "terminal_fg",
            "terminal_cursor",
        )
    }
    path = write_palette_file(
        tmp_path,
        {"schema_version": 1, "palettes": [{"name": "claude", **base}, {"name": "claude", **base}]},
    )
    with pytest.raises(ThemePaletteError, match="duplicate"):
        load_palettes(path)


def test_resolve_palette_prefers_explicit_name_over_environment(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv(THEME_ENV_VAR, "gruvbox")
    palettes = load_palettes()
    assert resolve_palette(palettes, "nord").name == "nord"
    assert resolve_palette(palettes, None).name == "gruvbox"


def test_resolve_palette_falls_back_on_unknown_environment(
    monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    monkeypatch.setenv(THEME_ENV_VAR, "neon-does-not-exist")
    palettes = load_palettes()
    assert resolve_palette(palettes, None).name == DEFAULT_THEME_NAME
    assert "neon-does-not-exist" in capsys.readouterr().err


def test_active_palette_falls_back_to_builtin_on_broken_file(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    broken = tmp_path / "broken.json"
    broken.write_text("not json", encoding="utf-8")
    palette = active_palette(broken)
    assert palette.name == DEFAULT_THEME_NAME
    assert palette.accent == "#D97757"
    assert "using built-in" in capsys.readouterr().err


def test_rich_theme_maps_semantic_roles() -> None:
    palette = load_palettes()["nord"]
    styles = rich_theme(palette).styles
    assert styles["vm.error"] == Style.parse(f"bold {palette.danger}")
    assert styles["vm.success"] == Style.parse(palette.success)
    assert styles["vm.accent"] == Style.parse(palette.accent)
    assert styles["vm.muted"] == Style.parse(palette.text_muted)
    assert styles["vm.border"] == Style.parse(palette.border)


def test_xterm256_palette_has_256_entries_with_exact_nearest() -> None:
    palette = xterm256_palette()
    assert len(palette) == 256
    assert xterm256_nearest((0, 0, 0)) == 0
    assert xterm256_nearest((255, 255, 255)) == 15
    # (135, 175, 135) is exactly cube index 108; (138, 138, 138) is exactly
    # grayscale index 245 (8 + 10 * 13).
    assert xterm256_nearest((135, 175, 135)) == 108
    assert xterm256_nearest((138, 138, 138)) == 245


def test_wezterm_snippet_pins_nord_chrome() -> None:
    nord = load_palettes()["nord"]
    assert render_wezterm_scheme(nord) == (
        "-- VibeMux theme: nord\n"
        "-- Generated by `vibemux theme --backend wezterm --theme nord`.\n"
        "-- Paste as `config.colors = { ... }` or merge it into wezterm.lua.\n"
        "-- The ANSI 16-color ramp is intentionally left to your base scheme.\n"
        "return {\n"
        "  colors = {\n"
        '    foreground = "#ECEFF4",\n'
        '    background = "#242933",\n'
        '    cursor_bg = "#88C0D0",\n'
        '    cursor_fg = "#242933",\n'
        '    selection_bg = "#434C5E",\n'
        '    selection_fg = "#ECEFF4",\n'
        '    split = "#4C566A",\n'
        "  },\n"
        "}\n"
    )


def test_tmux_snippet_pins_nord_and_gruvbox_chrome() -> None:
    palettes = load_palettes()
    nord = render_tmux_theme(palettes["nord"])
    assert 'set -g status-style "bg=colour235,fg=colour255"' in nord
    assert 'set -g pane-border-style "fg=colour240"' in nord
    assert 'set -g pane-active-border-style "fg=colour110"' in nord
    gruvbox = render_tmux_theme(palettes["gruvbox"])
    assert 'set -g status-style "bg=colour234,fg=colour187"' in gruvbox
    assert 'set -g pane-border-style "fg=colour59"' in gruvbox
    assert 'set -g pane-active-border-style "fg=colour208"' in gruvbox


def test_generation_is_deterministic() -> None:
    nord = load_palettes()["nord"]
    assert render_wezterm_scheme(nord) == render_wezterm_scheme(nord)
    assert render_tmux_theme(nord) == render_tmux_theme(nord)


def test_cli_theme_list_lists_every_theme() -> None:
    result = run_cli("theme", "--list")
    assert result.returncode == 0
    for name in ("claude", "github", "vscode", "nord", "gruvbox", "studio", "claude_light"):
        assert name in result.stdout


def test_cli_theme_list_is_theme_independent() -> None:
    baseline = run_cli("theme", "--list").stdout
    themed = run_cli("theme", "--list", env_extra={THEME_ENV_VAR: "gruvbox"}).stdout
    assert baseline == themed


def test_cli_theme_generates_wezterm_snippet() -> None:
    result = run_cli("theme", "--backend", "wezterm", "--theme", "nord")
    assert result.returncode == 0
    assert 'cursor_bg = "#88C0D0"' in result.stdout
    assert 'background = "#242933"' in result.stdout


def test_cli_theme_rejects_unknown_backend() -> None:
    result = run_cli("theme", "--backend", "kitty", "--theme", "nord")
    assert result.returncode == 4
    # main() reports VibeMuxError through the themed console on stdout.
    assert "unknown theme backend" in result.stdout


def test_cli_theme_requires_theme_with_backend() -> None:
    result = run_cli("theme", "--backend", "wezterm")
    assert result.returncode == 4
    assert "requires --theme" in result.stdout


def test_cli_theme_rejects_unknown_theme() -> None:
    result = run_cli("theme", "--backend", "tmux", "--theme", "neon")
    assert result.returncode == 4
    assert "unknown theme" in result.stdout
