# Windows native tool launch checks

VibeMux's acceptance CLI checks its Windows launch context before spawning
native tools. A token with restricting SIDs, or a token query that cannot be
verified, returns `BLOCKED` with a specific diagnostic. It does not retry through
another token or change sandbox permissions. `--validate` remains read-only.

The reported `ruff.exe` popup carried Windows status `0xc0000142`. Paired tests
of the same project Ruff executable and arguments returned that status under the
restricted token, but `ruff 0.16.1` and exit 0 under normal host access. Both
console-creation variants behaved the same. This establishes a launch-context
failure; the exact DLL/subsystem is still unknown. Node 24.18.0 succeeded in the
current paired checks, so earlier Node failures are not automatically assigned
the identical cause. See the [execution evidence](evidence/windows_launch_diagnosis.json).

The CLI uses the supported `TokenRestrictedSids` query with a bounded buffer and
checked group count/alignment. It closes its owned token handle. `TokenIsRestricted`
is reserved and is not used. A result without restricting SIDs is a launch
precondition, not a guarantee of every permission or tool capability.
[Microsoft token query documentation](https://learn.microsoft.com/en-us/windows/win32/api/securitybaseapi/nf-securitybaseapi-gettokeninformation).

For noninteractive acceptance runs, startup preserves existing process error-mode
bits and adds critical-error handling bits so classes of native failures return to
the caller. This is process-local, not a Windows registry or global Error Reporting
change. Children inherit the process error mode; `CREATE_NO_WINDOW` separately
controls console creation. Neither flag is treated as a repair of a failed DLL.
[Microsoft error-mode documentation](https://learn.microsoft.com/en-us/windows/win32/api/errhandlingapi/nf-errhandlingapi-seterrormode).

Use the verified project interpreter for checks, with imports bound to the selected
worktree. Do not use bare `ruff`: the observed shell resolution belongs to a separate
Tailnet Mesh environment. For example, from the integration worktree:

```powershell
$env:PYTHONPATH = Join-Path $PWD 'src'
& 'I:\VibeMux\.venv\Scripts\python.exe' -m ruff check .
& 'I:\VibeMux\.venv\Scripts\python.exe' -m pytest
```

If the caller is restricted, run these through its already-authorized normal Windows
execution path. Do not weaken the host sandbox or silently elevate. The project
helper cannot govern a direct executable launch made by the coding host; such
callers must select the correct execution context themselves.

Native initialization status is retained as `windows_tool_initialization_failed`
with the raw signed exit status and hex value. It is not converted into a claim
that the tool is uninstalled, and no automatic retry is made.
