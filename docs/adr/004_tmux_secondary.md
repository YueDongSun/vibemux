# ADR 004: tmux secondary development backend

Context: WSL/Linux development commonly has tmux. Decision: tmux adapter implements the same contract. Consequences: no tmux semantics leak into core. Alternative: require tmux everywhere, rejected.

