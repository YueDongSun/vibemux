"""Dual-track acceptance runner (master prompt section 14, ADR 031).

A test driver only: it runs the repository's own test suites and the
TaskBoard Lite calibration, collects the evidence the daemon exported
through Control, and writes a report whose every scenario status is
reconciled with executed tests and digested artifacts. It never writes
VibeMux state and never contacts a model gateway.
"""
