# Platform support

| Platform / Component | Position | Status |
|---|---|---|
| Windows native | Official product target | Python core/Mock run; the M3 daemon lifecycle is verified within scope |
| WSL2 | Development and optional POSIX backend | Compatible |
| Linux | Development and CI | Compatible |
| macOS | best effort | Not promised |
| Windows Terminal | shell/launcher entry point | Not a pane backend |
| WezTerm | Windows primary backend | Command contract implemented; live requires local dependencies |
| tmux | POSIX backend | Command contract implemented |
| ConPTY | Future native backend | Not implemented |
| `vibemuxd` / `vibemuxctl` | Windows named pipe / POSIX UDS | Real-machine Windows process smoke and Linux-container process smoke passed for M3 |
| Windows control runtime | Protected per-user DACL + hashed project leaf | Effective ACL verified for current SID / `SYSTEM` / Administrators, re-verified natively (read-only Win32, no helper process) in two phases on every trusted daemon start — before and after descriptor publication (ADR-020/ADR-025) |
