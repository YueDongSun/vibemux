import { app_config } from "../config.js";
import {
  create_workspace_session,
  download_file,
  prepare_attachment,
} from "./chat_store.js";
import { stream_response, validate_api_key } from "./provider.js";
import { browser_capabilities } from "./workspace_host.js";

// The standalone entry is the only production importer of this host. Keep the
// provider and its memory-only key contract out of the native dependency graph.
export function create_browser_host() {
  return {
    kind: "browser",
    capabilities: browser_capabilities,
    storage_session: create_workspace_session(),
    workspace_settings: (settings) => settings,
    storage_message: (error) => error.message,
    read_original_record: () => localStorage.getItem(app_config.storage_key) || "",
    stream_response,
    validate_api_key,
    prepare_attachment,
    download_file,
  };
}
