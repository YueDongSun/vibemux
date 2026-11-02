# VibeMux frontend development

Select this configuration when creating a GitHub Codespace for this branch, or open it with a compatible Dev Containers client. It installs exactly Node 22.22.0 and the locked dependencies for all three frontend packages: the read-only web monitor, the Tauri desktop UI, and the browser-local chat workspace. It starts no daemon, terminal harness or model peer.

From the repository root, run the following checks. Build before the desktop fixture test because that test inspects the generated frontend assets. The chat package has no typecheck script, so it is built and tested only.

```bash
npm run typecheck --prefix crates/vibemux_web/web
npm run typecheck --prefix crates/vibemux_desktop/ui
npm run build --prefix crates/vibemux_web/web
npm run build --prefix crates/vibemux_desktop/ui
npm run build --prefix crates/vibemux_chat/web
npm test --prefix crates/vibemux_web/web
npm test --prefix crates/vibemux_desktop/ui
npm test --prefix crates/vibemux_chat/web
```

Keep provider credentials absent and preserve all three npm lockfiles. Rust, Python, native Tauri libraries and Windows terminal/process acceptance are separate workflows governed by repository instructions and CI. This configuration adds no native-platform support claim.

The image and feature releases are specified in `devcontainer.json`. The JSON, script syntax and setup failure paths are checked locally. A Codespaces/container build has not been executed for this configuration; separate Codex Cloud setup results do not establish that build. The requested 4 CPUs and 16 GB memory are starting requirements, not a cost cap. Keep forwarded ports private.
