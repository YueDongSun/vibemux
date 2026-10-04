# Track A development tests

Owner: Track A (backend: `src/server.mjs`, `src/store.mjs`).

Put Track A's own `node:test` files for the HTTP server here, for example
`tests/worker_a/server_api.test.mjs`, and run them with:

```text
node --test tests/worker_a/
```

Rules:

- Use only the Node.js standard library; do not add dependencies.
- Start the server exactly as `CONTRACT.md` section 2 describes, read the
  listening receipt from stdout, and connect only to the receipt host and port.
- Put data files in a fresh directory under `os.tmpdir()` and remove it, and
  stop every server process you start, in `finally` blocks.

These tests are worker evidence only. Acceptance is decided by the trusted
verifier, which is not part of this repository copy.
