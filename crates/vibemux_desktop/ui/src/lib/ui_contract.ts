/// The commands the Rust side exposes to the webview. The list mirrors
/// `crates/vibemux_desktop/src/command_names.rs`; a command that is not in
/// this list is denied by the app capability file.
export const COMMAND_NAMES = [
  'projects_list',
  'project_pick',
  'project_select',
  'project_forget',
  'daemon_start',
  'daemon_stop',
  'tasks_snapshot',
  'supervisor_action',
  'task_window_open',
  'ask_targets',
  'ask_setup',
  'ask_confirm',
  'ask_submit',
  'ask_cancel',
  'ask_status',
  'ask_reply',
  'console_section',
  'probe_rerun',
  'quick_search',
  'open_link',
  'settings_get',
  'settings_set',
  'provider_profiles_get',
  'provider_profiles_set',
  'about_info',
] as const;

export type CommandName = (typeof COMMAND_NAMES)[number];

export interface CommandError {
  code: string;
  message: string;
}

export interface DesktopSettings {
  schema_version: 1;
  appearance: 'light' | 'dark' | 'system';
  text_size: 'small' | 'default' | 'large';
  density: 'comfortable' | 'compact';
  reduce_motion: boolean;
  sidebar_collapsed: boolean;
  start_page: 'chat' | 'tasks' | 'console';
}

export type SettingsPatch = Partial<Pick<DesktopSettings,
  'appearance' | 'text_size' | 'density' | 'reduce_motion' | 'sidebar_collapsed' | 'start_page'
>>;

export interface ProviderProfile {
  id: string;
  name: string;
  api_type: 'anthropic_messages' | 'openai_chat_completions';
  base_url: string;
  model: string;
  credential_env: string | null;
  auth_type: 'api_key' | 'bearer' | 'none';
}

export interface ProviderProfilesConfig {
  schema_version: 1;
  profiles: ProviderProfile[];
  orchestrator_profile_id: string | null;
}

export interface ProviderProfilesView {
  config: ProviderProfilesConfig;
  readiness: 'configuration_only';
}

// Implemented native calls only. The rest of COMMAND_NAMES remains reserved;
// neither a configured profile nor loading the UI enables those operations.
export interface NativeCommands {
  settings_get: { args: undefined; result: DesktopSettings };
  settings_set: { args: { patch: SettingsPatch }; result: DesktopSettings };
  provider_profiles_get: { args: undefined; result: ProviderProfilesView };
  provider_profiles_set: { args: { config: ProviderProfilesConfig }; result: ProviderProfilesView };
  about_info: { args: undefined; result: { app_version: string; webview_version: string } };
}

/// Events the Rust side emits to the webview. They carry no vendor text.
export type UiEventName =
  | 'projects_changed'
  | 'snapshot_changed'
  | 'ask_changed'
  | 'console_changed';
