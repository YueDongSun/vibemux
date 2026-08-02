# Security

VibeMux 的 worktree 只提供并发编辑隔离，不是 sandbox。核心安全边界包括 shell=False/argv、stdin 发送、路径 commonpath 校验、symlink/reparse fail-closed、pane ownership 校验、append-only events 和 cleanup dry-run。发现问题请不要公开披露凭据或 exploit payload，先联系维护者。

