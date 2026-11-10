import type { Backend } from './backend';
import type { CommandName, DesktopSettings, ProviderProfilesConfig, SettingsPatch, UiEventName } from './ui_contract';
import { validate_profiles } from './native_profiles.ts';

export const FIXTURE_MARKER = 'VIBEMUX_FIXTURE_DATA';

// Configuration-only fixture for rendering the same native host without IPC.
// It never supplies chat replies, projects, tasks, or fake execution readiness.
export function fixture_backend(): Backend {
  let settings: DesktopSettings = {
    schema_version: 1, appearance: 'system', text_size: 'default',
    density: 'comfortable', reduce_motion: false, sidebar_collapsed: false,
    start_page: 'chat',
  };
  let config: ProviderProfilesConfig = {
    schema_version: 1,
    profiles: [{
      id: 'fixture_profile', name: 'Fixture provider', api_type: 'openai_chat_completions',
      base_url: 'https://example.invalid/v1', model: 'fixture-model',
      credential_env: null, auth_type: 'none',
    }],
    orchestrator_profile_id: 'fixture_profile',
  };
  return {
    fixture: true,
    fixture_label: FIXTURE_MARKER,
    async invoke<T>(command: CommandName, args?: Record<string, unknown>): Promise<T> {
      let result: unknown;
      switch (command) {
        case 'settings_get': result = settings; break;
        case 'settings_set':
          settings = { ...settings, ...(args?.patch as SettingsPatch) };
          result = settings;
          break;
        case 'provider_profiles_get': result = { config, readiness: 'configuration_only' }; break;
        case 'provider_profiles_set':
          validate_profiles(args?.config);
          config = structuredClone(args.config);
          result = { config, readiness: 'configuration_only' };
          break;
        case 'about_info': result = { app_version: 'fixture', webview_version: 'fixture' }; break;
        default: throw { code: 'not_implemented', message: 'This fixture has no execution services.' };
      }
      return structuredClone(result) as T;
    },
    async listen(_event: UiEventName, _handler: (payload: unknown) => void): Promise<() => void> {
      return () => undefined;
    },
  };
}
