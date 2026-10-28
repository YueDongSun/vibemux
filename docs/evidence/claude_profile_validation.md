# Installed Claude profile observations, 2026-10-27

This diagnostic exercised Claude Code `2.1.304` on native Windows with a synthetic
loopback Anthropic API and disposable files. It is not a live model evaluation,
an OS-sandbox test, or a reusable workflow qualification. The existing
`claude_stream_json_writable_v1` profile remains `Declared`.

## Contract and method

ADR 036 requires five independent tool grants. The fixture worker distinguishes
modifying an existing file from creating a missing file. The native profile maps
these grants to vendor tool names; installed behavior must establish that mapping.

Each case used a cleared environment, a fresh private configuration/home and a new
workspace. A synthetic key and an owned loopback endpoint replaced live provider
configuration. The server returned a predetermined tool call and fixed completion
response. Prompt data, requested paths and credentials were synthetic. Context
exclusion and exclusion of all other outbound destinations were not independently
measured; this fixture must not be presented as proof of either property.

```text
--bare -p --input-format stream-json --output-format stream-json
--verbose --include-partial-messages --permission-mode dontAsk
--tools=<case tools> --allowedTools=<case permissions>
--strict-mcp-config --restricted --no-session-persistence --no-chrome
--model fixture-model
```

`--no-chrome` is an additional explicit diagnostic restriction. Two initial
launches in the outer restricted execution context initialized but made no request
to the fixture and timed out after 45 seconds with unknown transport errors. Those
failures remain transport failures. The same empty-tool case in the separately
approved diagnostic launch context completed with one observed local request.
No machine security setting changed.

## Observed controls

Twelve cases completed with CLI exit code zero. This is **not** twelve passing
independent-grant tests: some expose incompatibility, and a successful vendor turn
can contain a denied or unsuccessful tool call.

| Case | Installed observation | Interpretation |
| --- | --- | --- |
| Empty tools, text response | Init and API inventories empty; one request | Tool-free positive transport control |
| Empty tools, injected Edit | Tool unavailable; missing canary absent | Removed built-in could not be invoked |
| Read inside workspace | Expected canary returned | Positive file-read control |
| Read outside workspace | Explicit restricted-directory permission denial | CLI read scope enforced in this case |
| Write new file | Write absent from both inventories; tool unavailable | Current Write mapping unavailable |
| Write existing file | Write absent; tool unavailable | No overwrite; not edit-denial certification |
| Write outside workspace | Write unavailable | No path-boundary conclusion from an absent tool |
| Edit missing file inside workspace | Edit advertised; missing file created | Edit alone permits creation |
| Edit existing outside file without prior read | Read-before-write error | Inconclusive for directory denial |
| Edit missing file outside workspace | Restricted-directory denial; file absent | CLI creation scope enforced in this case |
| Empty built-ins plus one explicit stdio MCP tool | Only the named marker tool exposed; fixed response returned | Narrow installed MCP transport feasibility |
| Same MCP tool with unsupported argument | Broker rejection and error tool result | Broker can enforce a closed input contract |

The MCP marker server accepts no command or path, has a 20-second bound, and
recorded startup, call or rejection, and stdin closure. It does not implement
development tools or prove production process-tree containment. Reports declare
`reusable_qualification: false`; independent file observations are separate from
assistant text and process exit. The private diagnostic source and original
reports are retained with the coordinating task's recovery evidence.

## Execution-design consequence

Do not promote the native profile. Write is unavailable in this tested bare
profile, and Edit can create files. Renaming grants or treating argv construction
as evidence would erase the distinction required by ADR 036.

The next candidate is a separate profile with native tools removed and a
daemon-owned broker exposing closed operations. The broker must enforce
create-versus-edit and test-command semantics, bind effects to the admitted
attempt, and execute untrusted effects within a qualified OS boundary. The marker
fixture proves only transport feasibility, not a development broker or admission
permit. Networked MXC also cannot supply a gateway-only network claim because its
documented managed-network behavior permits host loopback. A networked trusted
controller with offline tool effects has a different trust boundary and needs its
own reviewed design and negative controls.

## Primary sources and remaining controls

GitHub MCP inspection covered official Claude Code releases
[2.1.304](https://github.com/anthropics/claude-code/releases/tag/v2.1.304),
[2.1.305](https://github.com/anthropics/claude-code/releases/tag/v2.1.305), and
[2.1.309](https://github.com/anthropics/claude-code/releases/tag/v2.1.309).
Later notes describe fixes involving tool-search omission, MCP config mutation,
teammate restrictions and Windows hooks. They identify relevant negative controls;
they do not establish that every defect affects 2.1.304 or authorize an upgrade.

[Bare-mode documentation](https://code.claude.com/docs/en/headless#start-faster-with-bare-mode)
describes a reduced built-in palette. [CLI flags](https://code.claude.com/docs/en/cli-reference)
distinguish tool selection from permission rules, while
[managed MCP precedence](https://code.claude.com/docs/en/managed-mcp#servers-passed-with-mcp-config-or-strict-mcp-config)
and [hook policy](https://code.claude.com/docs/en/hooks#disable-or-remove-hooks)
retain managed controls. These documents do not supply version-specific proof of
the observed empty-old-string creation behavior; the installed canary does.

The arXiv review of [CaMeL](https://arxiv.org/abs/2503.18813) motivates separating
model interpretation from capability-enforced effects. It is not a benchmark
result for VibeMux or proof of this boundary.

Remaining controls include customization exclusion, broker-config protection,
unknown tools, all grant-removal combinations, command/child confinement,
cancellation and cleanup, protocol/output exhaustion, artifact and launch-context
binding, expiry/revocation and per-attempt owner admission. No paid inference,
installation, live configuration change or deployment was performed by this
diagnostic.
