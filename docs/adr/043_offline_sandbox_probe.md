# ADR 043: Explicit offline sandbox evidence

Status: Accepted for bounded implementation (2026-10-26).

## Context

ADR 029 process-tree ownership and an independent Git worktree do not enforce
filesystem or network isolation. ADR 036 correctly keeps live writable harnesses
unqualified. We need executable evidence of a candidate native Windows boundary
before adding it to workflow admission.

The supported Codex sandbox command can execute a fixed native probe with explicit
MXC selection and managed requirements, without requesting a model or changing
saved settings. Parent-run disposable experiments observed denied outside reads,
writes, junction access, descendant writes and IPv4 host-loopback access with
network disabled. A managed-network experiment reached an unlisted loopback
listener. This agrees with the documented MXC loopback limitation. Node script
startup also failed under root-deny because its loader queried drive-root metadata;
an inline program worked. Neither observation qualifies ordinary Node or Claude.

## Decision

Add an explicit `vibemux_sandbox_probe` command in `vibemux_probe`, using a narrow
offline command adapter in `vibemux_platform`. The command creates disposable
fixtures beneath an operator-selected scratch parent, runs only its own fixed
native probe, validates all observations and independently checks file effects,
listener activity and cleanup. It emits a closed, versioned evidence report.

The adapter requests `windows.sandbox=mxc`, includes managed requirements, uses a
fresh named per-command profile, denies root access, permits minimal runtime reads,
grants one exact workspace, protects named subpaths, and disables networking. It
clears ambient provider/proxy/runtime injection variables. It has no online mode,
legacy fallback, installation step, arbitrary configuration passthrough or model
call. A caller owns the selected executable, paths and lifecycle; command
construction alone is not proof of enforcement or a TOCTOU guarantee.

The standalone command is an explicit diagnostic effect owner. It does not write
the daemon database, probe cache, capability catalog or workflow slot evidence.
Its report is not a reusable qualification credential. No Control or database
schema changes are needed. A later daemon-owned qualification operation must bind
artifact identities, expiry/revocation, policy template and actual launch facts
through the single writer before a workflow may consume evidence.

## Threat model and acceptance

Broken fixtures, missing runtime support, wrong nonces, incomplete/duplicate
observations, unrelated I/O errors, forbidden file effects and unresolved cleanup
must prevent a pass. Only recognized permission errors count as denials. Positive
controls establish that the fixture and runtime work. The parent owns the loopback
listener and checks a challenge before testing denial. Output and execution are
bounded. Fixture cleanup touches only the newly created owned directory.

The initial suite measures fixed native filesystem operations, a synchronous
descendant and IPv4 host loopback. It does not establish arbitrary-program,
service-mediated process, IPv6, remote-network, hardware, authenticated harness,
provider attribution or cross-platform qualification. The live Claude profile
remains `Declared`; its Write/Edit overlap and command grant semantics require
separate installed-version negative controls.

Keep the current Claude route executable and profile unchanged. An external
model client with offline MCP tools is a possible later design, with a different
trust contract; it is not implicitly introduced here.

## Sources and alternatives

- [Codex Windows sandbox](https://learn.chatgpt.com/docs/windows/windows-sandbox)
  documents explicit MXC selection, prerequisites, child lifetime and loopback limits.
- [Codex permissions](https://learn.chatgpt.com/docs/permissions) documents named
  profiles, exact filesystem rules, network enforcement and glob timing.
- [Codex source](https://github.com/openai/codex) supplies the mature native sandbox
  implementation; the installed command grammar is checked independently.
- [Anthropic sandbox runtime](https://github.com/anthropics/sandbox-runtime) has a
  Windows alpha requiring account, ACL and firewall setup. Such machine-wide
  provisioning is not part of this increment.
- [AgentDojo](https://arxiv.org/abs/2406.13352) motivates adversarial end-to-end
  agent evaluation. Its benchmark does not certify this OS boundary.

Reusing a supported isolation command avoids new unsafe Windows security code.
The tradeoff is an explicit installed-backend prerequisite and a narrow verified
scope. Online isolation, installed Claude qualification and daemon admission
remain separate work; a passing diagnostic does not remove those gates.
