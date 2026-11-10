// Presentation is shared; transport and persistence are explicit host inputs.
// This module has no browser transport import (including dynamic imports).
export const browser_capabilities = Object.freeze({
  send: true,
  attachments: true,
  exports: true,
  html_preview: true,
  links: true,
  images: true,
});

export const native_capabilities = Object.freeze({
  send: false,
  attachments: false,
  exports: false,
  html_preview: false,
  links: false,
  images: false,
});

export const native_unavailable = Object.freeze({
  send: "Desktop chat is unavailable. Provider profiles are configuration only; no message will be sent.",
  attachments: "Attachments are unavailable until native chat is implemented.",
  exports: "File export is not implemented in the desktop host. Nothing will be downloaded.",
  html_preview: "Executable HTML and SVG previews are disabled in the desktop host. Use code view.",
  links: "Opening external links is not implemented in the desktop host.",
});

export function require_workspace_host(host) {
  if (!host || !["browser", "native"].includes(host.kind) ||
      !host.storage_session || !host.capabilities ||
      typeof host.workspace_settings !== "function" ||
      typeof host.stream_response !== "function") {
    throw new TypeError("The workspace requires an explicit browser or native host.");
  }
  for (const key of Object.keys(browser_capabilities)) {
    if (typeof host.capabilities[key] !== "boolean" ||
        (host.kind === "native" && host.capabilities[key])) {
      throw new TypeError("The native workspace must not enable browser capabilities.");
    }
  }
  return host;
}
