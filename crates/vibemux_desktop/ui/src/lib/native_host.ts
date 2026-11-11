import { default_settings } from '../../../../vibemux_chat/web/src/config.js';
import { create_workspace_session } from '../../../../vibemux_chat/web/src/services/chat_store.js';
import { native_capabilities, native_unavailable } from '../../../../vibemux_chat/web/src/services/workspace_host.js';
import type { Backend } from './backend.ts';
import { create_native_preferences } from './native_settings.ts';
import { create_provider_profiles } from './native_profiles.ts';

const unavailable = (message: string): never => { throw new Error(message); };

export function native_storage_message(error: unknown): string {
  const code = error && typeof error === 'object' && 'code' in error ? error.code : null;
  if (code === 'storage_lock_unavailable') {
    return 'Local history is read-only: Web Locks are unavailable in this WebView. Changes in this window are unsaved. Desktop settings use native storage separately.';
  }
  if (code === 'storage_conflict' || code === 'storage_busy' || code === 'storage_not_owned') {
    return 'Local history is read-only or changed in another window. Nothing was overwritten. Changes here are unsaved; close other windows and retry, or reload.';
  }
  return 'Local history could not be saved. Its original record was preserved; changes in this window are unsaved. Desktop settings use native storage separately.';
}

export function create_native_host(backend: Backend) {
  // This WebView origin is separate from the browser origin. All optional local
  // history writes keep the existing Web Lock + compare-before-write contract;
  // there is deliberately no fallback for WebViews without Web Locks.
  const storage_session = create_workspace_session();
  return {
    kind: 'native' as const,
    fixture: backend.fixture === true,
    capabilities: native_capabilities,
    storage_session,
    // Version-1 records keep their known model/settings vocabulary. Native
    // preferences and provider profiles are never represented in that record.
    workspace_settings: (settings: { interface_style: string }) => ({
      ...default_settings, interface_style: settings.interface_style,
    }),
    storage_message: native_storage_message,
    preferences: create_native_preferences(backend),
    provider_profiles: create_provider_profiles(backend),
    stream_response: async () => unavailable(native_unavailable.send),
    validate_api_key: async () => unavailable('Raw API keys are not accepted by the desktop host. Configure an environment-variable name instead.'),
    prepare_attachment: async () => unavailable(native_unavailable.attachments),
    download_file: () => unavailable(native_unavailable.exports),
    read_original_record: () => unavailable(native_unavailable.exports),
  };
}

export type NativeHost = ReturnType<typeof create_native_host>;
