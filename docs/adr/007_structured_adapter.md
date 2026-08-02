# ADR 007: Structured adapter first, PTY fallback

Context: PTY cannot reliably signal completion or approval. Decision: adapters advertise capabilities; current Generic/Mock only. Consequences: no fake ACP/RPC. Alternative: parse keywords, rejected.

