# Patched deep-merge compatibility package

This private package preserves the deepmerge 4 API used by the frontend build tools while fixing [GHSA-gcqx-w48m-w2h4](https://github.com/advisories/GHSA-gcqx-w48m-w2h4). It is a locally maintained dependency, not a claimed patched upstream release.

`index.cjs` derives from `dist/cjs.js` in the MIT-licensed [deepmerge 4.3.1 npm archive](https://registry.npmjs.org/deepmerge/-/deepmerge-4.3.1.tgz). The adjacent `license` preserves upstream attribution. The published CommonJS distribution includes its mergeability helper, so this package needs no further dependencies. Existing upstream API/helper names remain intact; new patch helpers use project naming conventions.

## Security change

The upstream guard checks the target object's properties, which permits returned-object prototype poisoning for null-prototype targets, nested null placeholders and own `__proto__` keys. The local patch applies the [upstream advisory's remediation](https://github.com/TehShrike/deepmerge/issues/273):

- Reject `__proto__`, `constructor` and `prototype` in both the target-copy and source-copy loops, before reading their values.
- Define copied own properties explicitly, avoiding inherited setters.
- Treat a throwing property-presence check as unsafe.

Rejecting those three key names is the intentional compatibility change. Array concatenation, cloning, `clone: false`, `arrayMerge`, `isMergeableObject`, `customMerge`, enumerable symbols, opaque Date/RegExp/React values and `merge.all` retain their upstream behavior.

Both frontend manifests declare a direct local dependency under the external name `deepmerge`; npm's `$deepmerge` override routes transitive consumers to the same reviewed implementation. Keep the local package name/version honest and regenerate lockfiles through npm.

Registry audit does not assess this local source. Security and compatibility regression tests therefore run as part of both existing frontend `npm test` commands. The exploit cases fail against the original upstream implementation and pass with this patch. Remove the local replacement only after a compatible, reviewed upstream fix is available and the same regression/build checks pass.

```sh
node --test tools/deep_merge_compat/tests/security_test.cjs
```
