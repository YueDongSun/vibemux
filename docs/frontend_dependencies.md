# Frontend dependency security overrides

The monitor, desktop UI and browser-local chat have independent npm lockfiles.
Use `npm ci` in each package and retain the CI audit gate at `--audit-level=low`.
The overrides below were reviewed on 2026-11-06 with Node 22.22.0. They affect
build tooling, not canonical state, IPC, browser storage or provider protocols.

| Consumer | Pinned override | Reason |
| --- | --- | --- |
| All three UI packages: Svelte; monitor: Rollup's node resolver | `deepmerge` → `npm:@oss-cve-patches/deepmerge@4.3.2` | Fix returned-object prototype poisoning, GHSA-gcqx-w48m-w2h4 |
| Monitor: `workbox-build@7.4.1` | `glob@14.0.4` | Fix GHSA-3g3m-2fjr-6pw4 and select patched `minimatch@11.0.5` for GHSA-g3hv-75wg-m249 |

## Deepmerge fork review

Upstream `deepmerge` remains at 4.3.1 without a published fix. The pinned package
is an **interim third-party fork**, not an upstream release. Its
[targeted patch](https://github.com/oss-cve-patches/deepmerge/pull/1) rejects
`__proto__` keys in both the source and target copying loops. This prevents a
null-prototype target, primitive placeholder or target-owned key from invoking
the destination object's prototype setter. The documented advisory concerns
the returned object's prototype, not a write to global `Object.prototype`.

The published 4.3.2 tag and npm `gitHead` identify commit
`097ac210ab291868a2126afb77c5152023e7bc5f`. Source and shipped CJS/UMD code were
compared with upstream 4.3.1: the executable change is the two key guards.
Rebuilding the tagged source reproduced both shipped bundles byte for byte.
The MIT license and type declarations are unchanged; there are no runtime
dependencies or installation hooks. All 144 upstream assertions, documentation
examples and TypeScript checks passed against the fork. An isolated npm signature
check succeeded, and the npm tarball SHA-512 is pinned in each lockfile.

The fork has a recent single-maintainer publication history and no npm SLSA
attestation metadata. Its exact version and reviewed content are deliberate;
do not replace the pin with a range or treat an audit's package-name lookup as
proof of safety. Each UI's `npm test` resolves the merge library from its actual
installed consumers and checks source/target poisoning plus config compatibility.
The poisoning assertions were also executed against the original 4.3.1 and failed.

## Workbox compatibility

Workbox 7.4.1's declared glob 11 range has no patched in-range release. The
override is limited to that Workbox version. Its CommonJS `globSync` call uses
`cwd`, `follow` and `ignore`, all retained by glob 14.0.4. That release selects
minimatch 11.0.5 and path-scurry 3.0.0 without a blanket minimatch override.
The affected/new glob subtree packages use BlueOak-1.0.0, already present in
this dependency chain; the new chain removes the old CLI dependencies.
The packages support the pinned Node 22.22.0 runtime.

The shared regression fixture checks real Workbox manifest discovery, extension
filters, ignored files, spaces/Unicode, byte sizes and revision hashes. An
isolated before/after build produced identical monitor assets, including the
generated service worker; built-worker cache-boundary checks also passed.

## Removing or updating an override

Review published upstream fixes before removing these pins. For every affected
UI, run `npm ci`, `npm test`, `npm audit --audit-level=low` and `npm run build`;
also run `npm run typecheck` for the monitor and desktop UI. Keep the shared
security/compatibility tests when removing the fork. A Workbox upgrade must
revalidate precache discovery and the generated worker rather than copying the
override to a new major version without evidence. Record results in `PROGRESS.md`.
