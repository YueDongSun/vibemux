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
  'about_info',
] as const;

export type CommandName = (typeof COMMAND_NAMES)[number];

/// Events the Rust side emits to the webview. They carry no vendor text.
export type UiEventName =
  | 'projects_changed'
  | 'snapshot_changed'
  | 'ask_changed'
  | 'console_changed';
