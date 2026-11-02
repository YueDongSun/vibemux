#!/usr/bin/env bash
set -euo pipefail

if [[ "$(node --version)" != 'v22.22.0' ]]; then
  printf '%s\n' 'VibeMux requires exactly Node 22.22.0.' >&2
  exit 1
fi
if [[ ! -f crates/vibemux_web/web/package-lock.json || ! -f crates/vibemux_desktop/ui/package-lock.json ]]; then
  printf '%s\n' 'Run this profile from the VibeMux repository root with both npm lockfiles.' >&2
  exit 1
fi
npm ci --prefix crates/vibemux_web/web
npm ci --prefix crates/vibemux_desktop/ui
