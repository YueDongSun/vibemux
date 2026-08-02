# ADR 005: Git worktree isolation

Context: concurrent runs need independent writes. Decision: each Run owns a managed Git worktree and branch. Consequences: worktree is not a sandbox; cleanup validates ownership. Alternative: shared folder, rejected.

