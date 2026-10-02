# Track B development tests

Owner: Track B (frontend: `public/index.html`, `public/app.mjs`,
`public/styles.css`).

Put Track B's own `node:test` files here, for example
`tests/worker_b/frontend_hooks.test.mjs`, and run them with:

```text
node --test tests/worker_b/
```

Rules:

- Use only the Node.js standard library; do not add dependencies.
- The frontend talks to the HTTP API in `CONTRACT.md` section 5 through
  same-origin relative URLs. If Track A's server is not ready, test against a
  small local stand-in that you start and stop inside the test; do not edit
  `src/`.
- Keep the UI hooks in `CONTRACT.md` section 9 exactly as specified; the
  trusted browser verifier locates elements only through them.

These tests are worker evidence only. Acceptance is decided by the trusted
verifier, which is not part of this repository copy.
