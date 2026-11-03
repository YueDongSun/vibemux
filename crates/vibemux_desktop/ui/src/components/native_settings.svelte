<script lang="ts">
  import { SHORTCUTS, modal_focus } from '../lib/keyboard_shortcuts';
  import type { AboutInfo } from '../lib/settings_controller';
  import type { ProjectRow, PublicSettings, SettingsPatch } from '../lib/ui_contract';
  const sections = ['General', 'Appearance', 'Connection', 'Shortcuts', 'About'] as const;
  let section = $state<(typeof sections)[number]>('General');
  let { settings, about, about_error = '', error = '', saving = false, projects, connection, fixture_mode = false, on_save, on_refresh, on_connection_action, on_close }: {
    settings: PublicSettings | null; about: AboutInfo | null; about_error?: string; error?: string; saving?: boolean;
    projects: ProjectRow[]; connection: string; fixture_mode?: boolean;
    on_save: (patch: SettingsPatch) => Promise<unknown>; on_refresh: () => void;
    on_connection_action: (project_key: string, action: 'start' | 'stop') => void; on_close: () => void;
  } = $props();
  function save_select(event: Event, key: 'appearance' | 'text_size' | 'density' | 'start_page') {
    if (!settings || !(event.currentTarget instanceof HTMLSelectElement)) return;
    const selected = event.currentTarget.value;
    event.currentTarget.value = settings[key];
    void on_save({ [key]: selected } as SettingsPatch);
  }
  function save_toggle(event: Event, key: 'sidebar_collapsed' | 'reduce_motion') {
    if (!settings || !(event.currentTarget instanceof HTMLInputElement)) return;
    const selected = event.currentTarget.checked;
    event.currentTarget.checked = settings[key];
    void on_save({ [key]: selected });
  }
</script>

<dialog class="shell_dialog settings_dialog" aria-labelledby="settings_title" use:modal_focus oncancel={(event) => { if (!event.defaultPrevented) on_close(); }}>
  <header class="dialog_header"><div><h2 id="settings_title">Settings</h2></div><button class="icon_button" aria-label="Close settings" onclick={on_close}>×</button></header>
  <div class="settings_layout">
    <nav class="settings_nav" aria-label="Settings sections">{#each sections as item}<button class:active={section === item} aria-current={section === item ? 'page' : undefined} data-initial-focus={item === 'General' ? '' : undefined} onclick={() => section = item}>{item}</button>{/each}</nav>
    <div class="settings_content">
      {#if error}<p class="notice warning" role="alert">{error}</p>{/if}
      {#if !settings}<p>Settings could not be loaded.</p><button onclick={on_refresh}>Try loading settings again</button>
      {:else if section === 'General'}
        <h3>General</h3>
        <label class="settings_field"><span><strong>Start page</strong></span><select value={settings.start_page} disabled={saving || fixture_mode} onchange={(event) => save_select(event, 'start_page')}><option value="chat">Conversation</option><option value="tasks">Tasks</option><option value="console">Console</option></select></label>
        <label class="settings_field"><span><strong>Collapsed sidebar</strong><small>Keep a compact project rail. Ctrl / ⌘ + B also toggles this.</small></span><input type="checkbox" checked={settings.sidebar_collapsed} disabled={saving || fixture_mode} onchange={(event) => save_toggle(event, 'sidebar_collapsed')} /></label>
        <p class="settings_note">Preferences are saved by desktop Rust. Native prompts, replies, and search terms are not written to browser storage.</p>
      {:else if section === 'Appearance'}
        <h3>Appearance</h3>
        <label class="settings_field"><span><strong>Theme</strong><small>Match system follows your operating system.</small></span><select value={settings.appearance} disabled={saving || fixture_mode} onchange={(event) => save_select(event, 'appearance')}><option value="system">Match system</option><option value="light">Light</option><option value="dark">Dark</option></select></label>
        <label class="settings_field"><span><strong>Text size</strong><small>Scale conversation and interface text together.</small></span><select value={settings.text_size} disabled={saving || fixture_mode} onchange={(event) => save_select(event, 'text_size')}><option value="small">Small</option><option value="default">Default</option><option value="large">Large</option></select></label>
        <label class="settings_field"><span><strong>Density</strong><small>Adjust spacing in project rows and transcripts.</small></span><select value={settings.density} disabled={saving || fixture_mode} onchange={(event) => save_select(event, 'density')}><option value="comfortable">Comfortable</option><option value="compact">Compact</option></select></label>
        <label class="settings_field"><span><strong>Reduce motion</strong><small>The operating system's reduced-motion preference is also honored.</small></span><input type="checkbox" checked={settings.reduce_motion} disabled={saving || fixture_mode} onchange={(event) => save_toggle(event, 'reduce_motion')} /></label>
      {:else if section === 'Connection'}
        <h3>Native connection</h3><p class="settings_note">Desktop IPC: <strong>{connection}</strong>. Harness availability is read from each project's authenticated daemon.</p>
        {#if !projects.length}<p>No projects are registered. Choose a folder from the sidebar.</p>{/if}
        {#each projects as project (project.project_key)}<div class="connection_row"><div><strong>{project.name}</strong><small>{project.missing ? 'Folder missing' : 'Daemon ' + project.daemon}</small>{#if project.code}<code>{project.code}</code>{/if}</div><button disabled={saving || fixture_mode || project.missing} onclick={() => on_connection_action(project.project_key, project.daemon === 'running' ? 'stop' : 'start')}>{project.daemon === 'running' ? 'Stop…' : 'Start'}</button></div>{/each}
        <button onclick={on_refresh}>Refresh connection and versions</button>
        <h4>Available execution profiles</h4><p class="settings_note">Claude Code: tool-free, one turn.<br />Codex: configured read-only, one turn.<br />Each Send starts a new independent Ask. Appearance never changes the selected harness.</p>
      {:else if section === 'Shortcuts'}
        <h3>Keyboard shortcuts</h3><dl class="shortcut_list">{#each SHORTCUTS as shortcut}<div><dt>{shortcut.label}</dt><dd><kbd>{shortcut.keys}</kbd></dd></div>{/each}<div><dt>Send a text prompt</dt><dd><kbd>Enter</kbd></dd></div><div><dt>New line in the prompt</dt><dd><kbd>Shift + Enter</kbd></dd></div></dl><p class="settings_note">Shortcuts are ignored during IME composition. Escape closes UI surfaces and never cancels a running Ask.</p>
      {:else}
        <h3>About VibeMux</h3><dl class="about_list"><dt>App version</dt><dd>{about?.app_version ?? 'Unavailable'}</dd><dt>Webview version</dt><dd>{about?.webview_version ?? 'Unavailable'}</dd><dt>Backend</dt><dd>{fixture_mode ? 'Visible demo fixture · no execution' : 'Native Tauri commands → authenticated daemon'}</dd><dt>Connection</dt><dd>{connection}</dd></dl>{#if about_error}<p class="notice warning" role="status">{about_error}</p>{/if}<button onclick={on_refresh}>Refresh version details</button><p class="settings_note">This shell does not claim persistent conversation memory. Available Ask text survives a webview reload while the desktop process remains open.</p>
      {/if}
    </div>
  </div>
  <footer class="dialog_footer"><span role="status">{saving ? 'Saving preferences…' : fixture_mode ? 'Fixture settings are read-only.' : 'Changes apply after the desktop confirms they were saved.'}</span><button onclick={on_close}>Done</button></footer>
</dialog>
