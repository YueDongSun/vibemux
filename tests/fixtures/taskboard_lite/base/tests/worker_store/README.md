# Store development tests

Owner: whoever owns `src/store.mjs` (Track A unless the integrator assigns a
dedicated store worker).

Put `node:test` files that exercise the store module interface from
`CONTRACT.md` section 8 directly here, for example
`tests/worker_store/store_interface.test.mjs`, and run them with:

```text
node --test tests/worker_store/
```

Rules:

- Use only the Node.js standard library; do not add dependencies.
- Import `../../src/store.mjs` only; the store must not depend on the HTTP
  server or the UI.
- Use a fresh data file in a fresh directory under `os.tmpdir()` for every
  test and remove the directory in a `finally` block.

These tests are worker evidence only. Acceptance is decided by the trusted
verifier, which is not part of this repository copy.
