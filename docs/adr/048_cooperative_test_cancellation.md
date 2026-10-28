# ADR 048: Deferred cooperative test cancellation experiments

Status: Deferred; neither experimental owner is qualified (2026-10-27).
Decision disposition: auto-approved investigation within the requested scope.

The IOCP adapter in ADR 047 fixes the independently reproduced pending-reader
runtime lifetime defect. It did not establish removal of the disposable MXC
workspace in the backpressure control. A retained receipt reports sharing
violation 32; the exact outstanding handle remains unproven.

Two bounded cooperative-owner experiments were explored without increasing
permissions or deadlines. Profile v2 used a fixed stdin cancellation line;
profile v3 used an outside-held exclusive file under a read-only reserved
execution directory. Neither passed normal completion. A subsequent independent
startup-only control, without either owner or latch, also timed out with no
workload marker. Those failures therefore cannot establish a cancellation cause
or qualify either experiment. Their exact sources and failure records are retained
with private task recovery evidence.

Production retains the prior `node_test_offline_v1` fixed Node entrypoint, with
the independently tested IOCP output adapter, truthful cleanup diagnostics and
unknown-outcome retention. No cooperative owner or extra control read grant is
advertised or admitted. Revisit these alternatives only after the installed
backend passes the startup control; retain the original native acceptance
assertions and require fresh profile-specific evidence.

The full execution goal remains open. No alternate sandbox mode, global policy,
mount, driver, account, credential or installation was changed to make a check pass.
