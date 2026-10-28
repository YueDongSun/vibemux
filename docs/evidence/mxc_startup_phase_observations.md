# MXC startup phase observations

Date: 2026-10-27. This is failed native diagnostic evidence, not qualification.
The production offline policy, command profile and acceptance gates are unchanged.

## Diagnostic contract

The explicit ignored mxc_startup_phases test sends one fixed Node file/stdout
marker through the existing offline MXC command, contained trusted trampoline and
safe output pipes. It records artifact path/size/modification metadata, supplied
environment names, private-directory verification, observed process candidates,
marker/root/EOF timing, Job accounting and cleanup.

An optional explicitly supplied, audited read-only context helper may query the
test process and its own token. Missing/unavailable context is labelled as such;
the test neither installs a helper nor treats missing metadata as verified.
Artifact metadata and context observations do not form a reusable certificate.

The ordinary startup gate remains five seconds. A separate explicit
VIBEMUX_MXC_OBSERVATION_WINDOW=30s permits a timing observation matching the prior
full-probe ceiling. It cannot relax acceptance: marker, successful root exit and
both EOFs must each be observed strictly before 5,000 ms. A regression rejects
exact-boundary, late and missing observations. Shutdown retains the existing
three-second cleanup bound.

Process ancestry is sampled metadata, not Job membership evidence. The root
handle is retained; descendants are explicitly unverified_ancestry_candidate.
Each PPID graph is rebuilt, known older-child edges are discarded, and coarse
creation times do not authenticate the remaining edges. Only the actual Job owner
controls termination. Names/PIDs from ancestry sampling never authorize a kill.

## Observed attempts

| Observation | First five-second control | Expanded ancestry control | Separate 30-second observation |
| --- | --- | --- | --- |
| Observation stop | 5,134 ms | 5,120 ms | 30,129 ms |
| Marker observed | No | No | No |
| Root exit observed before stop | No | No | No |
| Output EOF observed before stop | No | No | No |
| Maximum sampled Job count | 5 | 5 | 5 |
| Root reaped after termination | Yes | Yes | Yes |
| Job observed empty after termination | Yes | Yes | Yes |
| Captured stdout/stderr bytes | 0 / 0 | 0 / 0 | 0 / 0 |
| Both EOFs within cleanup | No | No | No |
| Qualification result | Failed | Failed | Failed |

In the first report, the exact backend image was first observed as a direct
child at approximately 364 ms. Later reports contain PPID candidates for two
backend images and console hosts. They do not establish authenticated membership
for every descendant, identify the backend's internal wait, or prove that Node
never started. A marker was not observed.

The query-only helper and test process reported equal user/integrity identity,
elevated tokens, no restricted token flag and no AppContainer flag in these
attempts. This does not establish the context of earlier binaries or every child.

The longer observation does not support the hypothesis that startup merely takes
slightly more than five seconds. It still does not identify the cause. No complete
native matrix was rerun, no timeout was enlarged for acceptance, and no live slot
was enabled.

All failed fixture directories and private reports were preserved. The first
report predates the strict late-positive regression; that defect cannot explain
its failure because marker/root exit were absent. The second report received an
adjacent interpretation correction clarifying that PPID candidates are not owned
Job evidence. Original records were not rewritten as successes.

After each test process ended, its command wrapper remained present. Each wrapper
was stopped only after checking its exact PID, creation time, parent and task
command. No unrelated process or sandbox service was stopped.

## Checks and reproduction

Set verified absolute VIBEMUX_DEV_TEST_BACKEND and VIBEMUX_DEV_TEST_NODE values.
VIBEMUX_MXC_CONTEXT_PROBE is optional and must name an already-audited query-only
helper. The default command does not launch native qualification:

    cargo test --locked --offline -p vibemux_worker_tools --test mxc_startup_phases

Explicit five-second diagnostic:

    cargo test --locked --offline -p vibemux_worker_tools --test mxc_startup_phases -- --ignored --test-threads=1 --nocapture

For a separately authorized timing observation, set
VIBEMUX_MXC_OBSERVATION_WINDOW=30s before the same explicit command. A result after
five seconds still fails. Do not use this setting to report a passing startup
qualification.

The three native invocations failed their unchanged acceptance predicate.
The default worker suite passed 47 tests with 17 explicit native tests ignored.
Strict all-target Clippy passed. After making the context helper optional, the
focused default regression and strict diagnostic Clippy passed again.

Independent source review closed the late-positive and ancestry-attribution
findings. Its PASS covers the diagnostic contracts, not native startup.

## Sources and remaining work

The [official Windows sandbox guide](https://learn.chatgpt.com/docs/windows/windows-sandbox)
documents explicit MXC testing and preserves managed requirements. The documented
sandbox log was absent at this backend's default state location.

The inspected [MXC wrapper source](https://github.com/openai/codex/blob/970111f6cb62cc02fe36a82ea7d71b37a2889c97/codex-rs/mxc-sandbox/src/native.rs)
buffers SDK diagnostics and intentionally does not publish the buffer because it
can contain command/environment data. It does not provide an inferred public
switch to dump that data. This source observation is not a verified mapping from
the installed binary to every line of that revision.

Startup and output/resource settlement remain unresolved. Further native retries
need a new supported diagnostic or an independently established artifact/context
change. The broader R&D implementation continues on independent requirements;
these failures do not redefine the full goal as complete.


## Replacement backend control, 2026-10-28

A newly installed backend, reporting `codex-cli 0.165.0-alpha.1.2`, justified one
fresh invocation of the existing five-second control. The executable metadata
was 357,330,736 bytes, modified at epoch-milliseconds 1793122740262. The test kept
its 5,000 ms startup and 3,000 ms cleanup bounds and the same Node 24.18.0 fixture.
No managed policy, sandbox permissions or deadline was relaxed.

The control observed 5,164 ms with no marker, root exit or output EOF, and failed
(exit 101). It requested termination, reaped the root and observed an empty owned
Job; workspace removal was not attempted. Zero-byte output files and the original
report remain preserved. One sampled unverified ancestry candidate was present
in a later inventory, then absent in a subsequent query along with the wrapper.
No sampled process was killed. Ancestry remains distinct from Job membership;
these observations do not establish output/resource settlement or internal cause.

The report's backend_preflight_invoked=false refers only to the diagnostic's own
path. The parent ran `--version` separately before this control, so this is not a
cold-start comparison. This fourth native result preserves the earlier failures.
Further unchanged retries are not qualified by this result.
