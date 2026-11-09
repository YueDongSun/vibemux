// Trusted browser locations only; an explicit override never falls back.
// Discovery selects an executable, never a browser profile or running session.
import os from "node:os";
import path from "node:path";
import { is_file } from "./candidate_paths.mjs";

export function browser_search_paths({ platform = process.platform, home_dir = os.homedir() } = {}) {
  if (platform === "win32") {
    return [
      "C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe",
      "C:/Program Files/Google/Chrome/Application/chrome.exe",
    ];
  }
  if (platform === "darwin") {
    const bundles = [
      "Google Chrome.app/Contents/MacOS/Google Chrome",
      "Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
      "Chromium.app/Contents/MacOS/Chromium",
    ];
    const directories = ["/Applications"];
    if (path.posix.isAbsolute(home_dir)) {
      directories.push(path.posix.join(home_dir, "Applications"));
    }
    return directories.flatMap((directory) => bundles.map((bundle) => path.posix.join(directory, bundle)));
  }
  if (platform === "linux") {
    return ["/usr/bin/google-chrome", "/usr/bin/chromium"];
  }
  return [];
}

export async function resolve_browser_executable(requested_browser, { file_exists = is_file, ...locations } = {}) {
  if (requested_browser !== undefined) {
    return (await file_exists(requested_browser))
      ? { executable: requested_browser }
      : { missing: `browser executable not found: ${requested_browser}` };
  }
  const candidates = browser_search_paths(locations);
  for (const candidate of candidates) {
    if (await file_exists(candidate)) {
      return { executable: candidate };
    }
  }
  return {
    missing: candidates.length > 0
      ? `no browser executable found; searched ${candidates.join(", ")}`
      : "no browser search locations for this platform; supply --browser with an absolute executable path",
  };
}
