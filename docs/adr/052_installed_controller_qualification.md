# ADR 052: Installed Claude qualification through the file broker

Status: Implemented as an explicit Windows test-helper qualification path.
No Live slot promotion or OS sandbox qualification.

Extends ADR 036, ADR 044 and ADR 051.

## Problem and evidence

ADR 051 connected the actual broker to daemon admission using synthetic
controllers. Earlier installed-Claude broker observations did not exercise the
daemon's final owner/import path. The native built-in profile also cannot enforce
independent creation/edit grants on Claude 2.1.304: its Edit tool can create files,
while Write is absent from the tested bare palette.

A fresh installed-controller path must keep the broker as effect authority and
preserve the original worktree, lease, candidate, review and verifier contracts.

## Auto-approved decisions

1. Add a separate opaque InstalledClaudeGuard and the exact
   claude_broker_qualification_v1 profile. This guard is distinct from the
   synthetic-copy guard and is available only with test_helpers. The store
   accepts its profile only for Claude Fixture evidence, never Live evidence.
   Existing attempt replay cannot substitute a profile.
2. Pin the canonical Claude, broker and Git Bash images and revalidate them before
   launch. The observed installed Claude image exceeds 256 MiB, so its separate
   bound is 512 MiB; broker and synthetic-image bounds remain 256 MiB.
3. Use a typed controller enum to choose broker configuration. Do not infer
   authority from argv flags or pass synthetic controller flags to Claude.
4. Freeze an explicit environment in each prepared LaunchPlan. It contains a
   private attempt home/config/temp, necessary Windows runtime values, a fixed
   synthetic key/model and the owner-held numeric loopback API endpoint.
   Ordinary route environment names and credentials are refused. No daemon-global
   environment variable is changed to select a turn's home or provider.
5. Generate one strict MCP configuration from the pinned broker and exact owner
   bootstrap/scratch/output paths. Keep settings and MCP configuration outside
   broker tool scopes. Launch bare/restricted print mode with empty native tools,
   only granted named broker tools, no Chrome and no session persistence.
   Managed policy stays in force.
6. Preserve independent review. The first installed workflow used Claude for both
   worker and reviewer; both turns and candidate collection succeeded, but the
   existing store correctly rejected the review. The corrected qualification uses
   installed Claude for one role and a separately pinned synthetic Codex controller
   for the other. It never weakens that independence check.

The existing contained trampoline, observed Job completion, strict broker
publication, current-input check, inline import ownership, direct lease-loss
checks and durable-completion recovery retention remain authoritative.

## Observations and limitations

Qualification observes actual init/API tool inventories, correlated broker
effects, task-owned customization canaries, cancellation and final daemon gates.
These are during/after-run observations; the current protocol handshake is not a
pre-prompt inventory admission gate.

Project CLAUDE.md, hook and alternate-MCP canaries are in the controller's ancestor
project. The canary program has a positive control before qualification. Its marker
must remain absent, and its context string must not reach any observed message or
token-count request. Token-count requests receive the same exact-key and context
observations as message requests.

A private directory plus closed tool scopes is not OS write protection against
the trusted same-user controller executable. A numeric local fixture API and
nonessential-traffic settings are not proof that every other egress path is
blocked. Managed/global customization, arbitrary vendor versions, paid/live
inference, untrusted command execution and native MXC startup/cleanup are separate
qualification requirements.

The current four installed tests cover nine workflows: worker acceptance,
read-only reviewer acceptance, six refusal cases, and cancellation after a broker
effect. The responses are synthetic; no model-quality conclusion follows.

## Migration and use

No database/schema migration or ordinary Control operation is added. Production
startup and existing synthetic guards retain their behavior. The test-helper
constructor requires explicit installed and synthetic guards; a route JSON label
cannot construct either.

Supply verified absolute VIBEMUX_QUALIFY_CLAUDE and VIBEMUX_QUALIFY_GIT_BASH paths
only when invoking the ignored tests. The test owns its ephemeral API listener and
uses no account credentials:

    cargo test --locked --offline -p vibemuxd --features test_helpers --test workflow_installed_controller -- --ignored --test-threads=1

Without --ignored, only the local API-observation regression runs; the four
installed tests are skipped explicitly. See the evidence record for the exact
version, original failure and separately observed checks.

## Research grounding

The installed 2.1.304 --help output was checked for every selected flag.
Current [bare-mode documentation](https://code.claude.com/docs/en/headless#start-faster-with-bare-mode)
and [settings precedence](https://code.claude.com/docs/en/settings) describe
customization reduction and managed-policy precedence; they do not replace
version-bound installed observations.

GitHub MCP inspection of the
[official filesystem server](https://github.com/modelcontextprotocol/servers/blob/main/src/filesystem/README.md)
confirmed that its write operation combines creation and overwrite and that older
MCP roots can replace directory authority. The closed independent-grant broker
remains the appropriate boundary here; no new external SDK dependency was needed.
A tag-bound Claude changelog fetch returned metadata without file text, so it was
not treated as version-specific behavior evidence.

The arXiv query for [CaMeL](https://arxiv.org/abs/2503.18813) supports keeping
capability decisions outside model interpretation. Its reported AgentDojo results
are research evidence for that design approach, not a benchmark or security proof
of this integration.
