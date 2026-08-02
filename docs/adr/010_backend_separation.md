# ADR 010: Harness/provider/execution separation

Context: harness, model provider and runtime evolve independently. Decision: HarnessProfile, HarnessAdapter and execution_backend are separate fields. Consequences: future ACP/A2A additions do not reshape core. Alternative: single Provider class, rejected.

