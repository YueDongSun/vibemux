# A2A conformance and interoperability validation

These procedures validate the local Rust A2A gateway using synthetic fixtures. They do not invoke a model, open the authoritative VibeMux database, or establish remote/TLS readiness. Run commands from the VibeMux repository root. The recorded environment was native Windows, Rust 1.85.0, and CPython 3.13.12.

## Source identities

| Component | Verified source |
| --- | --- |
| Official A2A TCK | [`5996b79f9cefa6fc390980e383e358a66fb9e49e`](https://github.com/a2aproject/a2a-tck/tree/5996b79f9cefa6fc390980e383e358a66fb9e49e) |
| TCK specification snapshot | A2A specification `1.0.0`, commit [`173695755607e884aa9acf8ce4feed90e32727a1`](https://github.com/a2aproject/A2A/tree/173695755607e884aa9acf8ce4feed90e32727a1/specification); wire version `1.0` |
| Rust SDK packages | `a2a-lf = 0.3.0`, `a2a-client-lf = 0.2.1`, `a2a-server-lf = 0.4.1`, `a2a-grpc = 0.3.1` |
| Independent Go SDK | Release `v2.5.0`, commit [`9d95b95445f4208ba77f48a137a278067937adb7`](https://github.com/a2aproject/a2a-go/tree/9d95b95445f4208ba77f48a137a278067937adb7) |
| Independent Python SDK | Release `v1.1.3`, commit [`4e71245bf2bf4b31f6429f12d97991f1f3d4b3f4`](https://github.com/a2aproject/a2a-python/tree/4e71245bf2bf4b31f6429f12d97991f1f3d4b3f4) |

The gRPC boundary uses official SDK request/response conversions. Its narrow `tonic-types = 0.14.5` compatibility adapter corrects demonstrated upstream error-code and `google.rpc.ErrorInfo` omissions. It does not define replacement A2A protocol types.

## Prepare an isolated official TCK environment

Set `$python_exe` to the absolute path of an existing CPython 3.13 executable. The following PowerShell commands create a fresh validation directory and do not alter global Python configuration:

```powershell
$repo_path = (Get-Location).Path
$validation_root = Join-Path $env:TEMP ("vibemux_a2a_validation_" + [guid]::NewGuid().ToString("N"))
$tck_root = Join-Path $validation_root "a2a_tck"
git clone https://github.com/a2aproject/a2a-tck.git $tck_root
git -C $tck_root checkout --detach 5996b79f9cefa6fc390980e383e358a66fb9e49e
uv venv --python $python_exe "$tck_root/.venv"
$tck_python = Join-Path $tck_root ".venv/Scripts/python.exe"
uv pip install --python $tck_python --cache-dir "$validation_root/uv_cache" -e $tck_root
cargo build --locked -p vibemux_a2a --example task_tck_fixture --example task_interop_client -j 2
```

Use the actual build output location if `CARGO_TARGET_DIR` is configured. On Linux, virtual environment executables use `.venv/bin/python` and Rust executables have no `.exe` suffix; that platform was not validated by the recorded run.

## Run the official checks

```powershell
& $tck_python "$repo_path/tools/run_a2a_tck.py" `
  --tck-root $tck_root `
  --fixture "$repo_path/target/debug/examples/task_tck_fixture.exe" `
  --output "$validation_root/results"
```

The default checks all requirement levels over HTTP+JSON, JSON-RPC, and gRPC. `--level must` selects only MUST requirements. An intentional SDK/TCK upgrade can supply a different exact SHA with `--tck-commit`; moving branch names are rejected. `--timeout-seconds` is capped at 420 seconds.

The runner:

- checks the expected TCK HEAD and requires a clean checkout;
- invokes the same official `tests/compatibility/` pytest entrypoint used by `run_tck.py`, without changing its tests or assertions;
- starts one owned fixture, validates its bounded readiness record and numeric loopback endpoints, and sends it an explicit shutdown command;
- injects only the documented synthetic fixture credential through [a2a_tck_auth.py](../tools/a2a_tck_auth.py), without disabling gateway authentication;
- changes UTF-8, loopback proxy bypass, and pytest options only in child-process environments;
- writes each attempt into a unique directory, including failed attempts, and never replaces earlier evidence;
- limits collected process logs and records truncation; raw logs and upstream reports remain local diagnostic artifacts and require review before sharing;
- accepts success only when pytest and the fixture both exit zero, the fixture remains alive until requested shutdown, no forced fixture cleanup occurs, and the fresh JUnit report has consistent counters, at least one passing test per requested binding, and no failures or errors.

Each attempt contains `receipt.json`, `pytest.log`, fixture logs, and `reports/` with JUnit XML, compatibility JSON/HTML, and pytest HTML. The sanitized receipt records source and executable hashes, relevant runtime versions, exit codes, JUnit counts, and relative artifact ownership. Do not infer success from `compatibility.json` percentages alone: an observed upstream report displayed 100% despite pytest setup errors.

## Recorded final evidence and exclusions

The final integrated run on 2026-08-30 executed the reusable runner above with all three bindings and all requirement levels: **207 passed, 53 skipped, and 5 xfailed**, with zero failures/errors. Pytest and the fixture both exited zero; the fixture remained alive until explicit shutdown and needed no forced cleanup. The fixture SHA-256 was `2d182eb5fd8cc37f6ce57cbf0b88b248ba13ddacbf0fd1b24b63e237b238efce`; the authentication adapter SHA-256 was `2cc612d38df60ade4fd724235e0ff56b037dfbfa267e6551a00e9af7edc3f24a`. The [sanitized evidence](evidence/a2a_supervisor_validation.json) records source pins, counters and identities.

Earlier development attempts remain separate evidence: the preceding development fixture `95688d50fda863f307a9a0f07bc4101f97a9e5b9464495b0b04f45d72f816465` also reached 207/53/5; a MUST-only run reached 191 passed, 44 skipped, and 30 deselected. These earlier snapshots are not substituted for the final run.

The five expected failures were two unknown-JSON-field compatibility checks and three optional message-history checks. The 53 skips included 30 disabled-push checks, six history-scenario follow-up failures, undeclared extended-card/extension capabilities, unsupported-operation checks that did not apply to a streaming-capable server, one missing-timestamp check, and other unmet test prerequisites. Skips and expected failures do not prove those behaviors are implemented.

The final test-specific passing counts were 54 for HTTP+JSON, 56 for JSON-RPC and 51 for gRPC, plus common checks. Authentication/TLS for non-loopback exposure, signed cards, push delivery, complete history behavior, and full official ITK traversal were not established. A local conformance result with explicit exclusions is not a blanket protocol or production certification.

## Reproduce independent Python SDK interoperability

Prepare the reference environment from the verified official release source:

```powershell
$python_sdk = Join-Path $validation_root "a2a_python"
$peer_env = Join-Path $validation_root "python_peer_env"
git clone --depth 1 --branch v1.1.3 https://github.com/a2aproject/a2a-python.git $python_sdk
if ((git -C $python_sdk rev-parse HEAD).Trim() -ne "4e71245bf2bf4b31f6429f12d97991f1f3d4b3f4") { throw "Python SDK source mismatch" }
uv venv --python $python_exe $peer_env
$peer_python = Join-Path $peer_env "Scripts/python.exe"
uv pip install --python $peer_python --cache-dir "$validation_root/uv_cache" "${python_sdk}[http-server,grpc]" "uvicorn==0.52.4"
```

PowerShell variables do not carry across terminals. In each new terminal, enter the repository root and set `$peer_python` to the absolute interpreter path in the environment created above.

For Python-to-Rust validation, start the Rust `task_tck_fixture` executable in one terminal and keep its stdin open. In another terminal, use the `http_url` printed by that specific live fixture:

```powershell
& $peer_python tools/a2a_python_interop.py client --url "<rust_fixture_http_url>"
```

For Rust-to-Python validation, start the independent Python SDK reference peer in one terminal:

```powershell
& $peer_python tools/a2a_python_interop.py serve
```

Pass the printed `http_url` and `grpc_url` to the Rust probe in another terminal:

```powershell
'{"http_url":"<python_peer_http_url>","grpc_url":"<python_peer_grpc_url>"}' | & ./target/debug/examples/task_interop_client.exe
```

Press Enter in each server terminal to request graceful shutdown and verify its exit code. Never reuse endpoints from a stopped attempt. These checks verified discovery, task/context identity across send/get, and explicit cancellation in both directions over all three bindings. They use official SDKs but are **custom interoperability scenarios, not the official A2A ITK**. The official ITK launcher at the inspected revision relies on POSIX `fcntl` and process-group signaling; native Windows ITK execution was not demonstrated.
## Recorded independent Go SDK interoperability

Follow the [Go fixture procedure](../tools/a2a_go_interop/run_notes.md). The integrated rerun used the same final Rust fixture identified above and Rust client SHA-256 `93eab9d7a482fc10a0b01998231251e5a58e323cc1780de27f96623229294d2b`: three tests passed, zero skipped, with both directions exercised over all three bindings. `go vet .` and `go mod verify` passed. The module-local development receipt remains historical; the final integrated receipt is in [sanitized evidence](evidence/a2a_supervisor_validation.json). Python bilateral validation also passed all six direction/binding cases against these integrated binaries. Neither custom suite is the official ITK.
