"""Shared theme palettes for the Python CLI surfaces.

The canonical palette values live in ``config/theme_palettes.json`` and
mirror the compiled-in GUI palettes in ``crates/vibemux_frontend`` (the
Rust ``palette_parity`` integration test pins that file to the compiled
values). This module loads and validates the file, resolves the active
palette from ``VIBEMUX_THEME``, and maps the semantic roles onto rich
styles for human-facing output. Machine-readable ``--json`` output never
consumes the theme.
"""

from __future__ import annotations

import json
import os
import sys
from dataclasses import dataclass
from pathlib import Path

from rich.theme import Theme as RichTheme

from .errors import VibeMuxError

SCHEMA_VERSION = 1
DEFAULT_THEME_NAME = "claude"
THEME_ENV_VAR = "VIBEMUX_THEME"

_HEX_FIELDS: tuple[str, ...] = (
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


class ThemePaletteError(VibeMuxError):
    """The palette file is missing, unreadable, or does not validate."""


@dataclass(frozen=True)
class ThemePalette:
    """The color roles of one theme; hex values are ``#RRGGBB`` strings.

    Layout-only fields of the on-disk palette (font, radius, density)
    are deliberately not modeled here: the Python surfaces consume
    colors, not GUI geometry.
    """

    name: str
    bg: str
    surface: str
    surface_alt: str
    border: str
    text_primary: str
    text_muted: str
    accent: str
    accent_alt: str
    success: str
    warning: str
    danger: str
    terminal_bg: str
    terminal_fg: str
    terminal_cursor: str


def palette_file_path() -> Path:
    """Resolve the exported palette file relative to this checkout."""
    return Path(__file__).resolve().parents[2] / "config" / "theme_palettes.json"


def parse_hex_rgb(value: str) -> tuple[int, int, int] | None:
    """Parse ``#RRGGBB`` (upper or lower case); ``None`` otherwise."""
    body = value.strip()
    if not body.startswith("#") or len(body) != 7:
        return None
    digits = body[1:]
    try:
        return (
            int(digits[0:2], 16),
            int(digits[2:4], 16),
            int(digits[4:6], 16),
        )
    except ValueError:
        return None


def load_palettes(path: Path | None = None) -> dict[str, ThemePalette]:
    """Load and validate the exported palette file.

    Raises :class:`ThemePaletteError` on a missing file, a wrong schema
    version, duplicate names, or any invalid hex value. Layout-only
    fields are tolerated and ignored.
    """
    palette_path = palette_file_path() if path is None else path
    try:
        raw = json.loads(palette_path.read_text(encoding="utf-8"))
    except FileNotFoundError as exc:
        raise ThemePaletteError(f"theme palette file not found: {palette_path}") from exc
    except OSError as exc:
        raise ThemePaletteError(f"theme palette file unreadable: {exc}") from exc
    except json.JSONDecodeError as exc:
        raise ThemePaletteError(f"theme palette file is not valid JSON: {exc}") from exc
    if not isinstance(raw, dict) or raw.get("schema_version") != SCHEMA_VERSION:
        raise ThemePaletteError(f"theme palette schema_version must be {SCHEMA_VERSION}")
    entries = raw.get("palettes")
    if not isinstance(entries, list) or not entries:
        raise ThemePaletteError("theme palette file must list at least one palette")
    palettes: dict[str, ThemePalette] = {}
    for entry in entries:
        if not isinstance(entry, dict):
            raise ThemePaletteError("every palette entry must be a JSON object")
        name = entry.get("name")
        if not isinstance(name, str) or not name:
            raise ThemePaletteError("every palette needs a non-empty name")
        if name in palettes:
            raise ThemePaletteError(f"duplicate palette name: {name}")
        values: dict[str, str] = {}
        for field in _HEX_FIELDS:
            value = entry.get(field)
            if not isinstance(value, str) or parse_hex_rgb(value) is None:
                raise ThemePaletteError(f"palette {name} field {field} must be a #RRGGBB string")
            values[field] = value
        palettes[name] = ThemePalette(name=name, **values)
    return palettes


def resolve_palette(palettes: dict[str, ThemePalette], requested: str | None) -> ThemePalette:
    """Resolve the active palette from an explicit name or the environment.

    An unknown name is never fatal: it degrades to the default palette
    with a one-line stderr warning, mirroring the Rust frontend's
    fallback semantics for an invalid environment value.
    """
    name = requested if requested is not None else os.environ.get(THEME_ENV_VAR)
    if name is not None and name not in palettes:
        print(
            f"vibemux: unknown theme {name!r}, falling back to {DEFAULT_THEME_NAME!r}",
            file=sys.stderr,
        )
        name = None
    fallback = palettes.get(name or DEFAULT_THEME_NAME)
    if fallback is None:
        raise ThemePaletteError(
            f"theme palette file is missing the default palette {DEFAULT_THEME_NAME!r}"
        )
    return fallback


def active_palette(path: Path | None = None) -> ThemePalette:
    """Load the palette file and resolve the active palette.

    A broken palette file degrades to the built-in default instead of
    breaking every CLI command; the reason is announced on stderr.
    """
    try:
        palettes = load_palettes(path)
    except ThemePaletteError as exc:
        print(f"vibemux: {exc}; using built-in {DEFAULT_THEME_NAME!r} colors", file=sys.stderr)
        return _fallback_palette()
    return resolve_palette(palettes, None)


def _fallback_palette() -> ThemePalette:
    """The built-in copy of the default palette, used when the exported
    file cannot be read (for example in a wheel install without the
    repository ``config/`` directory)."""
    return ThemePalette(
        name=DEFAULT_THEME_NAME,
        bg="#0F0E0C",
        surface="#171512",
        surface_alt="#211F1A",
        border="#2B2822",
        text_primary="#EAE6DC",
        text_muted="#9C958A",
        accent="#D97757",
        accent_alt="#E7C6B4",
        success="#8AB487",
        warning="#D9A85F",
        danger="#CB6D63",
        terminal_bg="#0A0908",
        terminal_fg="#E8E4D8",
        terminal_cursor="#D97757",
    )


def rich_theme(palette: ThemePalette) -> RichTheme:
    """Map the semantic palette roles onto rich style names."""
    return RichTheme(
        {
            "vm.error": f"bold {palette.danger}",
            "vm.success": palette.success,
            "vm.warning": palette.warning,
            "vm.accent": palette.accent,
            "vm.border": palette.border,
            "vm.muted": palette.text_muted,
            "vm.title": f"bold {palette.accent}",
            "vm.text": palette.text_primary,
        }
    )
