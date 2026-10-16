# ADR 035: GUI language selection and rounded controls

Status: Accepted for the user-requested GUI enhancement.

## Decision

The native GUI defaults to English and offers Simplified Chinese through an
EN/CN control in the header and a Language setting. The choice is immediate
and persisted as `language: "en" | "zh_cn"` in the existing frontend settings.
The additive field keeps schema version 1, matching ADR 030's settings policy.
Missing or unknown language values select English without resetting the theme,
window size, or sidebar preference. Older binaries ignore the language field.

A frontend-owned catalog translates interface labels, help, tooltips, state
labels, and empty states. English is the fallback. Parameter substitution is
single-pass so IDs, paths, and text containing braces remain literal. User task
titles, drafts, event content, identifiers, and raw diagnostic values are data;
the locale setting does not rewrite them or call a translation service.
Synthetic preview content is English by default, including task titles and
example paths. Unicode rendering fixtures remain in tests.

The language is passed with the GUI presentation context, including detached
task windows. Localized window titles keep stable explicit widget identities.
The quick switcher finds local actions by either their English or displayed
labels. Locale changes preserve the draft, selected task, and open windows.

## Visual treatment

Extend ADR 030's warm light/dark palettes and serif headings with consistent
12 px button corners, 16 px cards/popups, and a 24 px composer. Task actions
and compact controls use pill outlines, restrained surface fills, larger hit
areas, and clear hover/focus states. The primary New task control receives an
accent-tinted surface. Rounded shapes do not alter disabled-action semantics.

Typography follows the content region: English display headings retain serif
type, Chinese display headings use the interface sans family, and task update
prose uses 13 px sans (12 px in panels below 380 px), with 19 px line height.
The 2026-10-14 density revision uses 24 px greetings, 18 px page headings,
15 px task titles, 13 px default body text, and 12 px controls. Headings in compact
panels reduce by 2 px with a 13 px minimum. Text wraps or truncates according to
its role; technical identifiers retain monospace. Shortcut rows scroll within
the window instead of enlarging the modal beyond its bounds. Button sizing
does not globally increase informational table row heights.

The [official Claude desktop guide](https://academy.claude.com/tutorials/navigating-the-claude-desktop-app)
is a navigation reference. The visual treatment is VibeMux's interpretation;
no proprietary assets, logos, or additional Claude capabilities are copied.

## Compatibility and verification

No daemon, IPC, database, plugin, execution policy, or shared palette schema
changes. Rollback may discard the optional locale preference but preserves
the other frontend settings. Test legacy/unknown-language configuration,
catalog completeness and placeholders, EN/CN search, draft preservation,
independent window presentation, and English preview fixtures. Capture both
languages in light/dark themes, including the minimum window size.

The TUI and Python CLI remain their existing English diagnostic surfaces.
Documentation, source comments, and validation records remain in English.
