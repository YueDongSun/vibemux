<script lang="ts">
  import { onMount, tick } from 'svelte';
  import BrandMark from '../../../vibemux_chat/web/src/components/brand_mark.svelte';
  import NativeComposer from './components/native_composer.svelte';
  import NativeTranscript, { type CodeArtifact } from './components/native_transcript.svelte';
  import NativeSettings from './components/native_settings.svelte';
  import CommandPalette from './components/command_palette.svelte';
  import ShortcutHelp from './components/shortcut_help.svelte';
  import NativeSidebar from './components/native_sidebar.svelte';
  import PersistentChat from './components/persistent_chat.svelte';
  import ChatExport from './components/chat_export.svelte';
  import ChatSearch from './components/chat_search.svelte';
  import { SearchState, type SearchView } from './lib/chat_search_state';
  import type { SearchHit } from './lib/chat_search_contract';
  import { ExportState, type ExportView } from './lib/chat_export_state';
  import WorkspaceChats from './components/workspace_chats.svelte';
  import WorkspaceCleanup from './components/workspace_cleanup.svelte';
  import { WorkspaceCleanupState } from './lib/workspace_cleanup_state';
  import { WorkspaceChatState, type WorkspaceChatView } from './lib/workspace_chat_state';
  import { WorkspaceExecutionState, type WorkspaceExecutionView } from './lib/workspace_execution_state';
  import { WorkspaceApprovalState, type WorkspaceApprovalView } from './lib/workspace_approval_state';
  import { SettingsController } from './lib/settings_controller';
  import { dispatch_shortcut, modal_focus } from './lib/keyboard_shortcuts';
  import { reveal_transcript_element } from './lib/transcript_scroll';
  import { selected_transcript_target } from './lib/transcript_target';
  import { build_palette_entries, type PaletteAction } from './lib/palette_entries';
  import { open_palette_conversation, type PaletteConversationTarget } from './lib/palette_navigation';
  import NativeContext from './components/native_context.svelte';
  import TaskView from './components/task_view.svelte';
  import ConsoleView from './components/console_view.svelte';
  import { ReadState, initial_read_view } from './lib/read_state';
  import { ProbeState } from './lib/probe_state';
  import { load_backend, type Backend } from './lib/backend';
  import { NativeState, command_code, harness_label, phase_label } from './lib/native_state';
  import { ChatState, chat_route_profile, type ChatView } from './lib/chat_state';
  import { NATIVE_LIMITS } from './lib/native_config';
  import type { Harness, PublicSettings, SettingsPatch } from './lib/ui_contract';

  const READ_REFRESH_MS = 5000;
  let screen = $state<PublicSettings['start_page']>('chat');
  let read_model: ReadState | null = null;
  let probe_model = $state<ProbeState | null>(null);
  let tasks_paged = false, console_paged = false;
  let opening_task_window = $state(false);
  let read_view = $state(initial_read_view());
  let backend: Backend | null = null;
  let model: NativeState | null = null;
  let chat_model = $state<ChatState | null>(null);
  let chat_view = $state<ChatView | null>(null);
  let chat_tab = $state<'chats' | 'ask' | 'workspaces'>('chats');
  let workspace_model = $state<WorkspaceChatState | null>(null);
  let workspace_view = $state<WorkspaceChatView | null>(null);
  let cleanup_model = $state<WorkspaceCleanupState | null>(null);
  let execution_model = $state<WorkspaceExecutionState | null>(null);
  let execution_view = $state<WorkspaceExecutionView | null>(null);
  let search_model = $state<SearchState | null>(null);
  let search_view = $state<SearchView | null>(null);
  let export_model = $state<ExportState | null>(null);
  let export_view = $state<ExportView | null>(null);
  let approval_model = $state<WorkspaceApprovalState | null>(null);
  let approval_view = $state<WorkspaceApprovalView | null>(null);
  let workspace_key: string | null = null;
  let chat_key: string | null = null;
  let revision = $state(0), startup_error = $state(''), busy = $state(false), context_open = $state(true), toast = $state(''), fixture_mode = $state(false);
  let dialog = $state<'consent' | 'chat_consent' | 'purge_chat' | 'setup' | 'stop' | 'forget' | null>(null);
  let dialog_project: string | null = null;
  let artifact = $state<CodeArtifact | null>(null);
  let harnesses = $state<Record<string, Harness>>({});
  let settings = $state<PublicSettings | null>(null);
  let settings_model: SettingsController | null = null;
  let settings_revision = $state(0);
  let overlay = $state<'settings' | 'palette' | 'help' | null>(null);
  let palette_navigating = $state(false);
  let palette_navigation_epoch = 0;
  let find_open = $state(false);
  const settings_view = $derived.by(() => { settings_revision; return { about: settings_model?.about ?? null, about_error: settings_model?.about_error ?? '', error: settings_model?.error ?? '', saving: Boolean(settings_model?.pending || settings_model?.loading) }; });
  const view = $derived.by(() => {
    revision;
    return model ? { projects: model.projects, project: model.project, selected_project: model.selected_project, selected_ask: model.selected_ask, presentation: model.presentation, draft: model.draft, pending: model.pending, unresolved: [...model.submissions.values()].filter((item) => item.project_key === model?.selected_project && item.state === 'uncertain'), error: model.error, connection: model.connection, statuses: new Map(model.statuses), replies: new Map(model.replies), targets: model.selected_project ? model.targets.get(model.selected_project) : undefined } : null;
  });
  const find_target = $derived(selected_transcript_target({ project_key: view?.selected_project ?? null,
    tab: chat_tab, ask_id: view?.selected_ask ?? null, saved: chat_model ? chat_view : null,
    workspace: workspace_view, execution: execution_model ? execution_view : null }));
  $effect(() => { if (find_target === null) find_open = false; });
  const palette_entries = $derived(build_palette_entries(view?.projects ?? [], view?.statuses ?? new Map(), view?.replies ?? new Map(), read_view.project_key ? { project_key: read_view.project_key, tasks: read_view.tasks } : null, {
    saved: chat_view ? { sessions: chat_view.sessions, titles: chat_view.titles } : null,
    workspaces: workspace_view?.catalogue ?? new Map(),
  }).map((entry) => ({ ...entry, disabled: entry.disabled || busy || palette_navigating || Boolean(dialog)
    || (fixture_mode && ['appearance', 'pick_project', 'toggle_sidebar', 'project', 'ask', 'new_chat', 'saved', 'workspace', 'search_history'].includes(entry.action.kind))
    || (entry.action.kind === 'saved' && !chat_model) || (entry.action.kind === 'workspace' && !workspace_model)
    || (entry.action.kind === 'search_history' && (!search_model || !view?.selected_project || view.connection !== 'ready'))
    || (entry.action.kind === 'new_ask' && !view?.selected_project) || (entry.action.kind === 'new_chat' && (!view?.selected_project || !chat_model))
    || (entry.action.kind === 'find' && (screen !== 'chat' || find_target === null)) })));
  const harness = $derived(view?.selected_project ? harnesses[view.selected_project] ?? 'claude' : 'claude');
  const active_status = $derived(view?.selected_ask ? view.statuses.get(view.selected_ask) : undefined);
  const active_reply = $derived(view?.selected_ask ? view.replies.get(view.selected_ask) : undefined);
  const has_active_work = $derived(Boolean(execution_view && view?.selected_project && execution_model?.has_active(view.selected_project)) || Boolean(workspace_view && view?.selected_project && workspace_model?.has_active(view.selected_project)) || (view?.project?.ask_ids.some((ask_id) => {
    const status = view.statuses.get(ask_id);
    return !status || (!status.terminal_observed && status.phase !== 'rejected');
  }) ?? false) || view?.pending?.state === 'sending' || (view?.unresolved.length ?? 0) > 0);
  const route = $derived(view?.targets?.targets.find((target) => target.harness === harness));
  /** Route facts for persistent chat follow the chat's own harness (selected
   * session, else its new-chat choice), never the independent-Ask picker. */
  const chat_route = $derived(chat_view ? chat_route_profile(chat_view.harness, view?.targets) : null);

  $effect(() => {
    if (settings?.appearance && settings.appearance !== 'system') document.documentElement.dataset.theme = settings.appearance;
    else delete document.documentElement.dataset.theme;
    document.documentElement.dataset.text_size = settings?.text_size ?? 'default';
    document.documentElement.dataset.reduce_motion = String(settings?.reduce_motion ?? false);
  });
  onMount(() => {
    let disposed = false;
    let timer: ReturnType<typeof setInterval> | undefined;
    let read_timer: ReturnType<typeof setInterval> | undefined;
    const unlisten: (() => void)[] = [];
    const keyboard = (event: KeyboardEvent) => {
      if (palette_navigating) return false;
      if (!dialog && !overlay && document.querySelector('dialog[open]')) return false;
      return dispatch_shortcut(event, {
      palette: () => open_overlay('palette'), settings: () => open_overlay('settings'), help: () => open_overlay('help'),
      find: open_find, new_ask: () => { if (!model?.selected_project || dialog) return false; new_ask(); return true; },
      toggle_sidebar: () => { if (!settings_model?.value || fixture_mode || dialog) return false; void settings_model.toggle_sidebar().then((saved) => { if (!saved) toast = settings_model?.error ?? 'Settings unavailable.'; }); return true; },
      dismiss: dismiss_surface,
      });
    };
    window.addEventListener('keydown', keyboard);
    void (async () => {
      try {
        const loaded = await load_backend();
        if (disposed) return;
        backend = loaded; fixture_mode = loaded.mode === 'fixture';
        if (loaded.mode === 'native') probe_model = new ProbeState(args => loaded.invoke('probe_rerun', args));
        read_model = new ReadState({
          tasks_snapshot: (project_key, query) => loaded.invoke('tasks_snapshot', { project_key, query }),
          console_section: (project_key, query) => loaded.invoke('console_section', { project_key, query }),
        }, (next) => { if (!disposed) read_view = next; });
        model = new NativeState(loaded, () => { if (!disposed) { revision++; sync_read_project(); sync_chat_project(); } });
        if (loaded.mode === 'native') chat_model = new ChatState({
          chat_request: (project_key, request) => loaded.invoke('chat_request', { project_key, request }),
          chat_confirm: (project_key) => loaded.invoke('chat_confirm', { project_key }),
          chat_pick_files: (project_key) => loaded.invoke('chat_pick_files', { project_key }),
        }, (next) => { if (!disposed) chat_view = next; });
        if (loaded.mode === 'native') search_model = new SearchState({
          chat_search_request: (project_key, request) => loaded.invoke('chat_search_request', { project_key, request }),
        }, (next) => { if (!disposed) search_view = next; });
        if (loaded.mode === 'native') export_model = new ExportState({
          chat_export_request: (project_key, request) => loaded.invoke('chat_export_request', { project_key, request }),
        }, (next) => { if (!disposed) export_view = next; });
        if (loaded.mode === 'native') approval_model = new WorkspaceApprovalState({
          chat_approval_request: (project_key, request) => loaded.invoke('chat_approval_request', { project_key, request }),
        }, (next) => { if (!disposed) approval_view = next; });
        if (loaded.mode === 'native') execution_model = new WorkspaceExecutionState({
          chat_execution_request: (project_key, request) => loaded.invoke('chat_execution_request', { project_key, request }),
          chat_confirm: (project_key) => loaded.invoke('chat_confirm', { project_key }),
          chat_pick_files: (project_key) => loaded.invoke('chat_pick_files', { project_key }),
        }, (next) => { if (!disposed) { execution_view = next; sync_approval(next); } });
        if (loaded.mode === 'native') workspace_model = new WorkspaceChatState({
          chat_owner_request: (project_key, request) => loaded.invoke('chat_owner_request', { project_key, request }),
        }, (next) => {
          if (disposed) return;
          workspace_view = next;
          void execution_model?.select(next.project_key, next.owner);
        });
        if (loaded.mode === 'native') cleanup_model = new WorkspaceCleanupState(request => loaded.invoke('workspace_cleanup_request', { request }));
        settings_model = new SettingsController(loaded, () => { settings = settings_model?.value ?? null; settings_revision++; });
        await settings_model.load();
        if (disposed) return;
        screen = settings_model.value?.start_page ?? 'chat';
        for (const name of ['projects_changed', 'ask_changed'] as const) {
          const remove = await loaded.listen(name, () => { if (!disposed) void model?.refresh(); });
          if (disposed) remove(); else unlisten.push(remove);
        }
        for (const name of ['snapshot_changed', 'console_changed'] as const) {
          const remove = await loaded.listen(name, (event) => {
            if (disposed || event.project_key !== read_model?.snapshot().project_key) return;
            if ((name === 'snapshot_changed' && screen === 'tasks' && !tasks_paged) || (name === 'console_changed' && screen === 'console' && !console_paged)) void refresh_read_view();
          });
          if (disposed) remove(); else unlisten.push(remove);
        }
        if (disposed) return;
        await model.refresh(true);
        if (disposed) return;
        timer = setInterval(() => {
          if (disposed) return;
          void model?.refresh();
          const live = chat_view?.session;
          if (screen === 'chat' && chat_tab === 'chats' && live && (live.state === 'opening' || live.state === 'busy' || live.active_request_id || chat_view?.live || (chat_view?.pending ?? null) !== null)) void chat_model?.poll();
          if (screen === 'chat' && chat_tab === 'workspaces' && (workspace_view?.owner?.provisioning_active || workspace_view?.pending)) void workspace_model?.refresh();
          const execution = execution_view?.session;
          if (screen === 'chat' && chat_tab === 'workspaces') void approval_model?.refresh();
          if (screen === 'chat' && chat_tab === 'workspaces' && (execution?.state === 'opening' || execution?.active_request_id || execution_view?.pending || execution_view?.live)) void execution_model?.refresh();
        }, NATIVE_LIMITS.refresh_ms);
        read_timer = setInterval(() => {
          if (disposed) return;
          if (screen === 'tasks' && !tasks_paged) void read_model?.refresh_tasks();
          if (screen === 'chat' && chat_tab === 'workspaces') { void workspace_model?.refresh(); void execution_model?.refresh(); }
        }, READ_REFRESH_MS);
      } catch (error) { startup_error = 'Native backend unavailable (' + command_code(error) + '). Reopen the locally built desktop app.'; }
    })();
    return () => { disposed = true; palette_navigation_epoch++; probe_model?.dispose(); cleanup_model?.dispose(); chat_model?.dispose(); search_model?.dispose(); export_model?.dispose(); approval_model?.dispose(); execution_model?.dispose(); workspace_model?.dispose(); window.removeEventListener('keydown', keyboard); if (timer) clearInterval(timer); if (read_timer) clearInterval(read_timer); for (const remove of unlisten) remove(); };
  });
  $effect(() => {
    if (!export_model) return;
    const saved = chat_tab === 'chats';
    const session = saved ? chat_view?.session : execution_view?.session;
    const selected_key = saved ? chat_view?.project_key : execution_view?.project_key;
    const pending = saved ? chat_view?.pending : execution_view?.pending;
    if (!view || screen !== 'chat' || (chat_tab !== 'chats' && chat_tab !== 'workspaces') || !session || !selected_key || selected_key !== view.selected_project) {
      export_model.select(null); return;
    }
    export_model.select({ project_key: selected_key, kind: saved ? 'chat' : 'workspace', chat_id: session.chat_id,
      generation: session.generation, version: session.version, enabled: !busy && view.connection === 'ready' && !view.project?.missing
        && !pending && session.active_request_id === null && ['ready','suspended','closed','archived'].includes(session.state) });
  });
  $effect(() => {
    if (search_view?.is_open && search_view.project_key !== view?.selected_project) search_model?.close();
  });
  function open_history_search() {
    if (!search_model || !view?.selected_project || view.connection !== 'ready' || dialog || overlay) return;
    search_model.open(view.selected_project);
  }
  async function open_search_hit(hit: SearchHit) {
    const project_key = search_view?.project_key;
    const selected = search_model?.take_hit(hit);
    if (!selected || !project_key || view?.selected_project !== project_key) return;
    screen = 'chat'; chat_tab = selected.kind === 'chat' ? 'chats' : 'workspaces';
    const target = { chat_id: selected.chat_id, generation: selected.generation, version: selected.version,
      ...(selected.location.target === 'record' ? { record_id: selected.location.record_id, sequence: selected.location.sequence } : {}) };
    let opened = false;
    if (selected.kind === 'chat') opened = await chat_model?.open_search_hit(target) ?? false;
    else {
      const owner = await workspace_model?.open_search_hit(selected.chat_id);
      if (owner && view?.selected_project === project_key && chat_tab === 'workspaces') {
        await execution_model?.select(project_key, owner);
        opened = await execution_model?.reveal_search_record(target) ?? false;
      }
    }
    if (view?.selected_project !== project_key || screen !== 'chat') return;
    if (!opened) { toast = 'The selected result is no longer available. Refresh the search.'; return; }
    if (selected.location.target === 'record') {
      await tick();
      reveal_transcript_element(document.querySelector('[data-search-record="' + selected.location.record_id + '"]'));
    }
  }
  function sync_read_project() {
    const project_key = model?.selected_project ?? null;
    if (!read_model || read_model.snapshot().project_key === project_key) return;
    tasks_paged = false; console_paged = false;
    read_model.select_project(project_key);
    void refresh_read_view();
  }
  function sync_approval(next: WorkspaceExecutionView) {
    const previous = approval_view;
    const same = previous?.project_key === next.project_key && previous?.chat_id === next.chat_id && previous?.generation === next.session?.generation;
    const cancelled = next.pending?.request.operation === 'cancel' || next.pending?.request.operation === 'close';
    const request_id = cancelled || next.owner?.phase !== 'open' ? null : next.session?.active_request_id
      ?? (same ? previous?.request_id : null) ?? next.attempts.at(-1)?.request_id ?? null;
    void approval_model?.select({ project_key: next.project_key, chat_id: next.chat_id, generation: next.session?.generation ?? null, request_id });
  }
  /** The chat model follows the selected project; consent upgrades come from
   * the daemon's project row and are never revoked from a stale row. */
  function sync_chat_project() {
    const project_key = model?.selected_project ?? null;
    if (project_key !== workspace_key) {
      workspace_key = project_key;
      void approval_model?.select({ project_key, chat_id: null, generation: null, request_id: null });
      void execution_model?.select(project_key, null);
      void workspace_model?.select_project(project_key);
    }
    if (model) {
      const project_keys = model.projects.map((project) => project.project_key);
      workspace_model?.retain_projects(project_keys);
      execution_model?.retain_projects(project_keys);
    }
    if (project_key !== chat_key) {
      chat_key = project_key;
      if (chat_model && project_key !== null) void chat_model.select_project(project_key, model?.project?.chat_confirmed ?? false);
    } else if (chat_model && project_key !== null && model?.project?.chat_confirmed && !chat_view?.confirmed) chat_model.update_consent(project_key, true);
  }
  async function refresh_read_view() {
    if (screen === 'tasks') { tasks_paged = false; await read_model?.refresh_tasks(); }
    else if (screen === 'console') { console_paged = false; await read_model?.refresh_console(); }
  }
  function navigate(next: PublicSettings['start_page']) {
    if (dialog || palette_navigating) return;
    screen = next;
    void refresh_read_view();
  }
  async function select_task(project_key: string, task_id: string) {
    if (!model || busy) return;
    if (model.selected_project !== project_key) await select_project(project_key);
    if (model.selected_project === project_key) { navigate('tasks'); await read_model?.open_task(task_id); }
  }
  async function pick_project() {
    if (!backend || !model || busy) return;
    busy = true;
    try { const picked = await backend.invoke('project_pick', {}); if (picked) await model.select_project(picked.project_key); else await model.refresh(); }
    catch (error) { toast = 'Folder selection failed (' + command_code(error) + ').'; }
    finally { busy = false; }
  }
  async function project_action(action: 'daemon_start' | 'daemon_stop' | 'ask_setup' | 'ask_confirm' | 'project_forget', requested_project?: string) {
    if (!backend || !model || busy) return;
    const project_key = requested_project ?? (dialog ? dialog_project : null) ?? model.selected_project;
    if (!project_key) return;
    busy = true;
    try { await backend.invoke(action, { project_key }); dialog = null; await model.refresh(); }
    catch (error) { toast = 'Action could not finish (' + command_code(error) + '). Check status before trying again.'; }
    finally { busy = false; }
  }
  async function save_settings(patch: SettingsPatch) {
    const saved = await settings_model?.save(patch);
    if (!saved) toast = settings_model?.error ?? 'Settings unavailable.';
    return saved;
  }
  function open_overlay(value: 'settings' | 'palette' | 'help'): boolean {
    if (dialog || palette_navigating) return false;
    overlay = value;
    return true;
  }
  function open_find(): boolean {
    if (screen !== 'chat' || dialog || overlay) return false;
    if (find_target === null) return false;
    find_open = true;
    document.querySelector<HTMLInputElement>('.transcript_find input')?.focus();
    return true;
  }
  function dismiss_surface(): boolean {
    if (overlay) { overlay = null; return true; }
    if (dialog) { if (busy) return false; dialog = null; return true; }
    if (screen === 'chat' && find_open) { find_open = false; return true; }
    if (screen === 'chat' && artifact) { artifact = null; return true; }
    if (screen === 'tasks' && read_view.selected_task) { read_model?.close_detail(); return true; }
    return false;
  }
  function open_dialog(value: typeof dialog, project_key = model?.selected_project ?? null) { dialog_project = project_key; dialog = value; }
  function new_ask() { if (palette_navigating) return; screen = 'chat'; chat_tab = 'ask'; overlay = null; find_open = false; artifact = null; model?.new_ask(); void tick().then(() => document.getElementById('message_composer')?.focus()); }
  function new_chat() { if (palette_navigating) return; screen = 'chat'; chat_tab = 'chats'; overlay = null; find_open = false; artifact = null; chat_model?.new_chat(); void tick().then(() => document.getElementById('message_composer')?.focus()); }
  function switch_chat_tab(tab: 'chats' | 'ask' | 'workspaces') { if (dialog || overlay || palette_navigating) return; chat_tab = tab; find_open = false; artifact = null; if (tab === 'workspaces') void workspace_model?.refresh(); }
  async function select_chat(project_key: string, chat_id: string) {
    if (!chat_model || busy) return;
    if (model?.selected_project !== project_key) await select_project(project_key);
    if (model?.selected_project === project_key) { screen = 'chat'; chat_tab = 'chats'; artifact = null; find_open = false; void chat_model.select_chat(chat_id); }
  }
  async function chat_sidebar_action(project_key: string, chat_id: string, action: 'close' | 'archive' | 'resume' | 'purge') {
    if (!chat_model || busy || fixture_mode) return;
    if (action === 'purge') { await select_chat(project_key, chat_id); open_dialog('purge_chat'); return; }
    // Project-bound: act on the indicated project's chat without silently
    // redirecting to the currently selected project's state.
    await chat_model.manage(action, undefined, chat_id, project_key);
  }
  async function confirm_chat_consent() {
    if (!chat_model || busy) return;
    const confirmed = await chat_model.confirm();
    dialog = confirmed ? null : dialog;
    if (!confirmed) toast = 'Chat consent could not be recorded (' + (chat_view?.error ?? 'unavailable') + ').';
  }
  async function purge_chat() {
    if (!chat_model || busy) return;
    const done = await chat_model.manage('purge');
    if (done) dialog = null; else toast = chat_view?.error ?? 'Chat purge failed.';
  }
  async function select_project(project_key: string) {
    if (!model || busy) return;
    busy = true; artifact = null; find_open = false;
    try { await model.select_project(project_key); } finally { busy = false; }
  }
  async function open_task_window(task_id: string) {
    const project_key = read_view.project_key;
    if (!backend || fixture_mode || !project_key || opening_task_window) return;
    opening_task_window = true;
    try { await backend.invoke('task_window_open', { project_key, task_id }); }
    catch (error) { toast = 'Task window could not be opened (' + command_code(error) + ').'; }
    finally { opening_task_window = false; }
  }
  async function select_ask(project_key: string, ask_id: string) {
    if (!model || busy) return;
    if (model.selected_project !== project_key) await select_project(project_key);
    if (model.selected_project === project_key) { screen = 'chat'; chat_tab = 'ask'; artifact = null; find_open = false; model.select_ask(ask_id); }
  }
  async function request_project_action(project_key: string, action: 'start' | 'stop' | 'forget') {
    if (action === 'start') { await project_action('daemon_start', project_key); return; }
    if (action === 'forget' && model) {
      const project = model.projects.find((row) => row.project_key === project_key);
      const pending = [...model.submissions.values()].some((item) => item.project_key === project_key && (item.state === 'sending' || item.state === 'uncertain'));
      const active = project?.ask_ids.some((ask_id) => { const status = model?.statuses.get(ask_id); return !status || (!status.terminal_observed && status.phase !== 'rejected'); });
      if (pending || active || workspace_model?.has_active(project_key) || execution_model?.has_active(project_key)) { toast = 'Select this project and resolve its active or uncertain work before forgetting it.'; return; }
    }
    overlay = null;
    await tick();
    open_dialog(action, project_key);
  }
  async function select_palette_conversation(action: PaletteConversationTarget) {
    if (!model || busy || dialog || palette_navigating || fixture_mode) return;
    const offered = palette_entries.some((entry) => !entry.disabled && (entry.action.kind === 'saved' || entry.action.kind === 'workspace')
      && entry.action.kind === action.kind && entry.action.project_key === action.project_key && entry.action.chat_id === action.chat_id);
    if (!offered) { toast = 'This conversation is no longer available. Refresh its project.'; return; }
    const epoch = ++palette_navigation_epoch;
    palette_navigating = true; busy = true; overlay = null;
    try {
      await tick();
      const opened = await open_palette_conversation(action, { native: model, saved: chat_model, workspace: workspace_model,
        current: () => palette_navigation_epoch === epoch && !dialog && !fixture_mode });
      if (palette_navigation_epoch !== epoch) return;
      if (opened) { screen = 'chat'; chat_tab = action.kind === 'saved' ? 'chats' : 'workspaces'; artifact = null; find_open = false; }
      else toast = 'This conversation is no longer available. Refresh its project.';
    } catch (error) {
      if (palette_navigation_epoch === epoch) toast = 'Conversation could not be opened (' + command_code(error) + ').';
    } finally { palette_navigating = false; busy = false; }
  }
  async function activate_palette(action: PaletteAction) {
    if (action.kind === 'saved' || action.kind === 'workspace') { await select_palette_conversation(action); return; }
    if (busy || dialog || palette_navigating) return;
    overlay = null;
    await tick();
    if (action.kind === 'project') await select_project(action.project_key);
    else if (action.kind === 'ask') await select_ask(action.project_key, action.ask_id);
    else if (action.kind === 'screen') navigate(action.value);
    else if (action.kind === 'task') await select_task(action.project_key, action.task_id);
    else if (action.kind === 'search_history') { screen = 'chat'; open_history_search(); }
    else if (action.kind === 'appearance') await save_settings({ appearance: action.value });
    else if (action.kind === 'new_ask') new_ask();
    else if (action.kind === 'new_chat') new_chat();
    else if (action.kind === 'settings' || action.kind === 'help') open_overlay(action.kind);
    else if (action.kind === 'toggle_sidebar') await settings_model?.toggle_sidebar();
    else if (action.kind === 'find') { if (!open_find()) toast = 'Select a conversation before using Find.'; }
    else if (action.kind === 'pick_project') await pick_project();
    else if (action.kind === 'refresh') { await model?.refresh(true); await refresh_read_view(); }
  }
  async function refresh_preferences() { await Promise.all([settings_model?.load(), model?.refresh(true)]); }
  async function copy_artifact() {
    if (!artifact) return;
    try { await navigator.clipboard.writeText(artifact.content); toast = 'Code copied.'; }
    catch { toast = 'Clipboard unavailable. Select the code to copy it.'; }
  }
</script>

<div class="native_shell" class:sidebar_collapsed={settings?.sidebar_collapsed} class:codex={view?.presentation === 'codex'} class:compact={settings?.density === 'compact'}>
  <NativeSidebar {screen} on_navigate={navigate} projects={view?.projects ?? []} statuses={view?.statuses ?? new Map()} replies={view?.replies ?? new Map()} chats_by_project={chat_view?.sessions ?? new Map()} chat_titles={chat_view?.titles ?? new Map()} selected_chat={chat_tab === 'chats' ? chat_view?.selected_chat ?? null : null} selected_project={view?.selected_project ?? null} selected_ask={chat_tab === 'ask' ? view?.selected_ask ?? null : null} collapsed={settings?.sidebar_collapsed ?? false} disabled={busy} {fixture_mode} on_select_project={(key) => void select_project(key)} on_select_ask={(key, id) => void select_ask(key, id)} on_select_chat={(key, id) => void select_chat(key, id)} on_chat_action={(key, id, action) => void chat_sidebar_action(key, id, action)} on_pick={() => void pick_project()} on_new={new_ask} on_new_chat={new_chat} on_settings={() => open_overlay('settings')} on_palette={() => open_overlay('palette')} on_shortcuts={() => open_overlay('help')} on_toggle={() => { if (!fixture_mode) void settings_model?.toggle_sidebar(); }} on_project_action={(key, action) => void request_project_action(key, action)} />

  <main class="main" inert={palette_navigating}>
    <header class="top_bar"><div class="breadcrumbs"><span>{view?.project?.name ?? 'Choose a project'}</span><span class="muted">/</span><strong>{screen === 'tasks' ? 'Tasks' : screen === 'console' ? 'Console' : view?.presentation === 'codex' ? 'Code workspace' : 'Conversation'}</strong></div>
      <div class="button_row">{#if screen === 'chat' && search_model}<button aria-label="Search project conversations" disabled={view?.connection !== 'ready' || !view?.selected_project} onclick={open_history_search}>Search history</button>{/if}{#if screen === 'chat'}<button class="icon_button" aria-label="Find in current transcript" title="Find in transcript · Ctrl / ⌘ + F" disabled={find_target === null} onclick={open_find}>⌕</button>{/if}<button class="icon_button" aria-label="Open command palette" title="Search · Ctrl / ⌘ + K" onclick={() => open_overlay('palette')}>⌘</button><span class="connection_label" role="status">{view?.connection === 'ready' ? 'Native IPC ready' : 'Native IPC unavailable'}</span>{#if screen === 'chat' && chat_tab === 'ask'}<label class="presentation_picker"><span class="sr_only">Visual presentation</span><select value={view?.presentation ?? 'claude'} onchange={(event) => model?.set_presentation(event.currentTarget.value as 'claude' | 'codex')}><option value="claude">Claude view</option><option value="codex">Codex view</option></select></label>{/if}{#if screen === 'chat'}<button class="icon_button" aria-label="Toggle context inspector" aria-pressed={context_open} onclick={() => context_open = !context_open}>▤</button>{/if}</div>
    </header>
    {#if fixture_mode}<div class="fixture_banner" role="status">DEMO FIXTURE · No daemon or harness execution</div>{/if}
    {#if startup_error}<div class="notice error" role="alert">{startup_error}</div>{/if}
    {#if view?.error}<div class="notice warning" role="alert">{view.error}<button onclick={() => void model?.refresh(true)}>Refresh status</button></div>{/if}
    {#if toast}<div class="toast" role="status"><span>{toast}</span><button aria-label="Dismiss message" onclick={() => toast = ''}>×</button></div>{/if}
    {#if screen === 'tasks'}<TaskView view={read_view} on_refresh={() => void refresh_read_view()} on_more={() => { tasks_paged = true; void read_model?.load_more_tasks(); }} on_select={(task_id) => void read_model?.open_task(task_id)} on_filter={(filter) => read_model?.set_filter(filter)} on_close_detail={() => read_model?.close_detail()} on_open_window={fixture_mode ? undefined : (task_id) => void open_task_window(task_id)} opening_window={opening_task_window} />
    {:else if screen === 'console'}<ConsoleView view={read_view} probe_controller={probe_model} on_section={(section) => { console_paged = false; void read_model?.select_console(section); }} on_refresh={() => void refresh_read_view()} on_next={() => { console_paged = true; void read_model?.next_console_page(); }} on_task_open={(task_id) => { if (view?.selected_project) void select_task(view.selected_project, task_id); }} />
    {:else}
    <div class="workspace_body"><div class="conversation_column">
      {#if view?.project}
        <div class="project_status_bar"><span class="status_dot" class:running={view.project.daemon === 'running'}></span><span>Daemon {view.project.daemon}</span>{#if view.project.code}<code>{view.project.code}</code>{/if}<div class="button_row">
          {#if view.project.daemon !== 'running'}<button disabled={busy || fixture_mode || view.project.missing} onclick={() => void project_action('daemon_start')}>Start daemon</button>{:else}<button disabled={busy || fixture_mode} onclick={() => open_dialog('stop')}>Stop daemon</button>{/if}
          <button disabled={busy} onclick={() => void model?.refresh(true)}>Refresh</button><button disabled={busy || fixture_mode || has_active_work} title={has_active_work ? 'Resolve active or uncertain work first.' : 'Forget this registered project'} onclick={() => open_dialog('forget')}>Forget</button></div>
        </div>
        <div class="chat_mode_row" role="tablist" aria-label="Conversation mode">
          <button role="tab" aria-selected={chat_tab === 'chats'} class:active={chat_tab === 'chats'} onclick={() => switch_chat_tab('chats')}>Chats</button>
          <button role="tab" aria-selected={chat_tab === 'ask'} class:active={chat_tab === 'ask'} onclick={() => switch_chat_tab('ask')}>Independent Ask</button>
          <button role="tab" aria-selected={chat_tab === 'workspaces'} class:active={chat_tab === 'workspaces'} disabled={fixture_mode} onclick={() => switch_chat_tab('workspaces')}>Workspaces</button>
          <span class="chat_mode_note">Saved chats · preview</span>
        </div>
        {#if export_model && export_view && (chat_tab === 'chats' || chat_tab === 'workspaces')}
          <ChatExport view={export_view} on_format={(format) => export_model?.set_format(format)} on_export={() => void export_model?.export()} />
        {/if}
        {#if chat_tab === 'chats'}
          {#if chat_model && chat_view}
            <PersistentChat view={chat_view} controller={chat_model} disabled={busy || view.connection !== 'ready' || view.project.missing} harness={chat_view.harness} harness_available={chat_route?.available ?? false} route_note={chat_route?.profile ?? ''} route_reason={chat_route?.reason ?? ''} notify={(message) => toast = message} on_artifact={(value) => artifact = value} on_consent={() => open_dialog('chat_consent')} on_purge={() => open_dialog('purge_chat')} on_new_chat={new_chat} {find_open} on_find_close={() => find_open = false} />
          {:else}
            <div class="availability_card"><strong>Persistent chats unavailable</strong><p>{fixture_mode ? 'The demo fixture carries no chat data. Saved chats need the locally built desktop app and a registered project.' : 'Chat state is not available yet. Reopen the app if this persists.'}</p></div>
          {/if}
        {:else if chat_tab === 'workspaces'}
          {#if workspace_view}
            <WorkspaceChats view={workspace_view} project_name={view.project.name}
              {find_open} on_find_close={() => find_open = false}
              {approval_view} approval_controller={approval_model}
              {execution_view} execution_controller={execution_model} presentation={view.presentation}
              notify={(message) => toast = message} on_artifact={(value) => artifact = value}
              on_refresh={() => void workspace_model?.refresh()} on_load_more={() => void workspace_model?.load_more()}
              on_select={(id) => { find_open = false; void workspace_model?.select_owner(id); }}
              on_create={(selected_harness, title) => workspace_model?.create(selected_harness, title) ?? Promise.resolve(false)}
              on_reconcile={() => workspace_model?.reconcile() ?? Promise.resolve(false)}
              on_stop={() => workspace_model?.stop() ?? Promise.resolve(false)}
              on_retry={() => workspace_model?.retry_pending() ?? Promise.resolve(false)} />
            {#if cleanup_model && workspace_view.project_key && workspace_view.project_key === view.selected_project
              && workspace_view.owner?.phase === 'finalized' && workspace_view.owner.workspace_planned
              && workspace_view.owner.workspace_effect === 'settled' && workspace_view.owner.workspace_verified_at !== null
              && workspace_view.owner.error_code === null}
              <WorkspaceCleanup controller={cleanup_model} target={{ registration_key: workspace_view.project_key,
                task_id: workspace_view.owner.task_id, run_id: workspace_view.owner.run_id }} />
            {/if}
          {/if}
        {:else}
        {#if !route?.available}<div class="availability_card"><strong>{harness_label(harness)} unavailable</strong><p>{route?.reason ?? view.targets?.code ?? 'Checking configured routes…'}</p>{#if view.targets?.code === 'control_unsupported_version'}<p>This daemon is too old for Ask (needs Control v5 or newer).</p>{/if}{#if view.targets?.setup_offered}<button disabled={busy || has_active_work || fixture_mode || view.project.daemon === 'running'} title="Stop this project's daemon explicitly before setup." onclick={() => open_dialog('setup')}>Set up supported routes</button>{/if}</div>{/if}
        {#if !view.project.ask_confirmed}<div class="availability_card"><strong>Inference consent required</strong><p>Review the project’s one-turn capabilities before sending a prompt.</p><button class="primary" disabled={busy || fixture_mode} onclick={() => open_dialog('consent')}>Review and accept</button></div>{/if}
        <div class="transcript_scroll" data-transcript-scroll>
          {#each view.unresolved.filter((item) => item.submission_key !== view.pending?.submission_key) as pending (pending.submission_key)}<div class="notice warning"><strong>Earlier submission is still uncertain</strong><p>{pending.prompt.slice(0, 150)}</p><button onclick={() => void model?.retry_submission(pending.submission_key)}>Reconcile earlier submission</button></div>{/each}
          {#if view.selected_ask}<NativeTranscript {find_open} on_find_close={() => find_open = false} ask_id={view.selected_ask} status={active_status} reply={active_reply} notify={(message) => toast = message} on_cancel={() => { if (view.selected_ask) void model?.cancel(view.selected_ask); }} on_copy_draft={(text) => { model?.set_draft(text); new_ask(); }} on_artifact={(value) => artifact = value} />
          {:else}<section class="welcome"><BrandMark /><p class="eyebrow">YOUR NATIVE WORKSPACE</p><h1>{view.presentation === 'codex' ? 'A focused place to think.' : 'What would you like to explore?'}</h1><p>Choose Claude Code or Codex below.<br />Each Send starts a new independent Ask.</p><div class="starter_cards"><button onclick={() => model?.set_draft('Explain this idea clearly: ')}><strong>Explore an idea</strong><span>Bring a question and its context.</span></button><button onclick={() => model?.set_draft('Review this code and suggest a clear improvement:\n\n')}><strong>Review some code</strong><span>Paste the relevant text in your prompt.</span></button></div></section>{/if}
        </div>
        <NativeComposer draft={view.draft} {harness} targets={view.targets} disabled={busy || fixture_mode || view.connection !== 'ready' || !view.project.ask_confirmed || view.project.missing} sending={view.pending?.state === 'sending'} uncertain={view.pending?.state === 'uncertain'} on_draft={(value) => model?.set_draft(value)} on_harness={(value) => { if (view.selected_project) harnesses[view.selected_project] = value; }} on_send={async (explicit_new) => { await model?.send(harness, explicit_new); await model?.refresh(); }} on_retry={async () => { await model?.retry_submission(); await model?.refresh(); }} />
        {/if}
      {:else}<section class="welcome"><BrandMark /><h1>Your projects, one place.</h1><p>Choose a local project folder to inspect its daemon and supported harness routes.</p><button class="primary" disabled={busy || !view || fixture_mode} onclick={() => void pick_project()}>Choose project folder</button></section>{/if}
    </div>
    {#if artifact}<aside class="code_artifact" aria-label="Code artifact"><header><div><h2>{artifact.title}</h2><span class="muted">{artifact.language} · Code only</span></div><button class="icon_button" aria-label="Close code artifact" onclick={() => artifact = null}>×</button></header><button onclick={() => void copy_artifact()}>Copy code</button><pre><code>{artifact.content}</code></pre><p class="muted">HTML and SVG are displayed as code. Active previews are unavailable.</p></aside>{:else if context_open && chat_tab === 'ask' && view?.presentation === 'codex'}<NativeContext project={view.project} status={active_status} on_close={() => context_open = false} />{/if}
    </div>
    {/if}
  </main>
</div>

{#if dialog}<div class="dialog_backdrop" role="presentation"><dialog class="confirmation_dialog" aria-labelledby="dialog_title" use:modal_focus oncancel={(event) => { if (busy) event.preventDefault(); else if (!event.defaultPrevented) dialog = null; }}>
  <p class="dialog_project_name">Project: {view?.projects.find((project) => project.project_key === dialog_project)?.name ?? 'Selected project'}</p><h2 id="dialog_title">{dialog === 'consent' ? 'Allow one-turn inference for this project?' : dialog === 'chat_consent' ? 'Allow persistent chats for this project?' : dialog === 'purge_chat' ? 'Purge this chat’s stored content?' : dialog === 'setup' ? 'Set up available native routes?' : dialog === 'stop' ? 'Stop this project’s daemon?' : 'Forget this project?'}</h2>
  {#if dialog === 'consent'}<p>Sending starts the selected installed harness and may use your configured provider account. Your prompt and permitted project context are handled by that harness.</p><p>Claude Code: tool-free, one turn. Claude cannot open files through tools; it may load applicable CLAUDE.md instructions. Codex: configured read-only, one turn.</p><p>Each Send starts a new independent Ask. There is no shared vendor conversation memory.</p>
  {:else if dialog === 'chat_consent'}<p>Saved chats keep messages and reported tool activity in this project’s private storage and survive app restarts. Sending starts the selected installed harness and may use your configured provider account. The vendor harness may also retain its own session history for this chat under your provider account and its retention settings.</p><p>Claude Code: tool-free turns; Claude cannot open files through tools. Codex: configured read-only, and approval requests are denied automatically.</p><p>Persistent chat is a preview capability; no live-run certification is claimed. Consent for saved chats is separate from independent-Ask consent, and drafts stay on this device until you press Send.</p>
  {:else if dialog === 'purge_chat'}<p>This removes the private text of the selected chat — stored prompts, replies, and retained record content — from this project’s daemon storage. The action cannot be undone, and purged text cannot be shown again. Canonical record metadata (identifiers, timestamps, and content-free bookkeeping) remains under daemon ownership.</p>
  {:else if dialog === 'setup'}<p>This explicit action discovers supported native executables, prepares routes when no configuration exists, records detection, and may restart this project’s daemon. Existing route configuration is preserved. New Claude routes use supported sign-in and provider environment references already present in the app. Credential values stay out of the file.</p><p>Stop this project’s daemon explicitly and resolve active or uncertain work before setup.</p>
  {:else if dialog === 'stop'}<p>This is an explicit lifecycle action for the selected project. Cancellation and process containment remain daemon-owned.</p>{#if has_active_work}<p class="notice warning">This project has active or uncertain work. Stopping its daemon may interrupt it; a lost response does not prove cancellation.</p>{/if}
  {:else}<p>This removes the desktop registration and settled display history. The daemon, saved chats, workspace files and branches remain available when you choose this project again.</p>{/if}
  <div class="button_row"><button disabled={busy} onclick={() => dialog = null}>Back</button><button class="primary" disabled={busy} onclick={() => { if (dialog === 'consent') void project_action('ask_confirm'); else if (dialog === 'chat_consent') void confirm_chat_consent(); else if (dialog === 'purge_chat') void purge_chat(); else void project_action(dialog === 'setup' ? 'ask_setup' : dialog === 'stop' ? 'daemon_stop' : 'project_forget'); }}>{busy ? 'Working…' : dialog === 'consent' ? 'Accept for this project' : dialog === 'chat_consent' ? 'Accept saved chats' : dialog === 'purge_chat' ? 'Purge chat content' : dialog === 'setup' ? 'Set up routes' : dialog === 'stop' ? 'Stop daemon' : 'Forget project'}</button></div>
</dialog></div>{/if}


{#if overlay === 'settings'}<NativeSettings {settings} about={settings_view.about} about_error={settings_view.about_error} error={settings_view.error} saving={settings_view.saving} projects={view?.projects ?? []} connection={view?.connection ?? 'unavailable'} {fixture_mode} on_save={save_settings} on_refresh={() => void refresh_preferences()} on_connection_action={(key, action) => void request_project_action(key, action)} on_close={() => overlay = null} />
{:else if overlay === 'palette'}<CommandPalette entries={palette_entries} on_select={(action) => void activate_palette(action)} on_close={() => overlay = null} />
{:else if overlay === 'help'}<ShortcutHelp on_close={() => overlay = null} />{/if}

{#if search_model && search_view}
  <ChatSearch view={search_view} on_query={(value) => search_model?.set_query(value)} on_scope={(value) => search_model?.set_scope(value)}
    on_case_sensitive={(value) => search_model?.set_case_sensitive(value)} on_include_archived={(value) => search_model?.set_include_archived(value)}
    on_search={() => void search_model?.start()} on_more={() => void search_model?.load_more()} on_close={() => search_model?.close()} on_open={(hit) => void open_search_hit(hit)} />
{/if}
