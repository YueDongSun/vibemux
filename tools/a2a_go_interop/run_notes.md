# Go SDK interoperability fixture

This module verifies a narrow, real-network interoperability contract between VibeMux's Rust A2A adapter and the official Go SDK. It is a custom cross-language integration test, **not an official ITK or TCK result**.

The official dependency is [`github.com/a2aproject/a2a-go/v2` v2.5.0](https://github.com/a2aproject/a2a-go/releases/tag/v2.5.0), resolved to commit `9d95b95445f4208ba77f48a137a278067937adb7`. The SDK exposes A2A protocol `1.0`; its module checksum is `h1:ZdcFoxv+nZTUV0i2ue5hES76YCANFPG9vjqd7vK8yWM=`. The accepted local run used Go `1.26.4` on native Windows `amd64`. Dependency versions and checksums are retained in `go.mod` and `go.sum`.

## Verified scope

The development run recorded in [verification.json](verification.json) passed all three tests without skips. The final integrated rerun is recorded separately in [supervisor validation evidence](../../docs/evidence/a2a_supervisor_validation.json), with updated Rust binary identities and the same passing cases:

- Official Go SDK client to the Rust `TaskServer` fixture: Agent Card discovery, message submission, matching task and context IDs on GET, terminal cancellation, and matching canceled state on a subsequent GET.
- Rust `TaskClient` / `GrpcTaskClient` to an independent Go SDK fixture: submission, task/context identity on GET, and terminal cancellation. HTTP/JSON-RPC clients discover the Agent Card; the gRPC client uses the supplied loopback endpoint.
- Rejection of non-loopback, credential-bearing, query-bearing, or non-root bootstrap URLs.

Both interoperability directions exercised **HTTP+JSON, JSONRPC, and GRPC**. The tests also wait for owned server/process shutdown and verify that their listening ports close. `go vet .` and `go mod verify` passed. The receipt binds these observations to exact source files and Rust executable SHA-256 hashes; later source or binary changes require a fresh run.

The Go agent uses `a2asrv.NewHandler`, official REST/JSON-RPC handlers, and `a2agrpc/v1.NewHandler`. Its deterministic executor returns an input-required task, and the SDK's cancellation path transitions that task to canceled. It does not represent an independently released official reference-agent product. All protocol message encoding, decoding, and task methods use the official SDK; the small HTTP adapter only restricts origins, adds a synthetic bearer value, and bounds response reads.

## Reproduce on Windows

Run from this module directory. Build the `task_tck_fixture` and `task_interop_client` Rust examples first using the [conformance procedure](../../docs/a2a_conformance_validation.md). This Go module does not build or modify the Rust workspace automatically.

```powershell
$go_executable = (Get-Command go -ErrorAction Stop).Source
$cache_root = Join-Path ([System.IO.Path]::GetTempPath()) ('vibemux_go_interop_' + [Guid]::NewGuid().ToString('N'))
$env:GOPATH = Join-Path $cache_root 'gopath'
$env:GOMODCACHE = Join-Path $cache_root 'modules'
$env:GOCACHE = Join-Path $cache_root 'build'
$env:GOENV = 'off'
$env:GOTOOLCHAIN = 'local'
$env:VIBEMUX_RUST_SUT = '<absolute_path_to_task_tck_fixture.exe>'
$env:VIBEMUX_RUST_INTEROP_CLIENT = '<absolute_path_to_task_interop_client.exe>'
& $go_executable version
& $go_executable mod download
& $go_executable test -count=1 -v .
& $go_executable vet .
& $go_executable mod verify
```

The environment variables affect this shell and its children only. Do not use `go env -w`, a global module cache override, or a global Go configuration change. Build/runtime caches and executable output belong outside the source directory and must not be published with this module.

If either Rust executable variable is absent, its corresponding test explicitly skips. **A skipped interoperability test is not a pass.** Acceptance requires both fixtures, three passing tests, and no skips. The tests use the public synthetic credential from the dedicated Rust fixture; no provider credentials or real model calls are required.

## Standalone modes

`go run . client` consumes one newline-terminated JSON object from stdin containing `http_url`, `grpc_url`, and `bearer_token`, then emits an allowlisted result. It permits literal loopback HTTP origins only and refuses redirects or Agent Card interfaces outside the supplied origins.

`go run . server` consumes one JSON line containing a synthetic `bearer_token`. It binds HTTP and gRPC listeners to OS-assigned loopback ports and prints one JSON readiness line containing `http_url` and `grpc_url`. Keep stdin open; send `shutdown` followed by a newline to stop and join the listeners. The server is test-only, request/body bounded, and never invokes models, tools, Git, or the canonical database.

## Limits

- This run establishes the listed unary interoperability cases only. It does not establish complete SDK conformance, streaming/subscription/push behavior, all error mappings, authentication adversarial coverage, or an official ITK/TCK pass.
- The fixture is ephemeral and synthetic; it does not prove the daemon's canonical Task/Run workflow, workspace mutation, model behavior, or production readiness.
- Remote endpoints, TLS, Linux execution, and arbitrary peer implementations were not tested here.
- The recorded Rust binary hashes identify the tested SUT snapshots, not an assertion that every later repository checkout contains identical code.
