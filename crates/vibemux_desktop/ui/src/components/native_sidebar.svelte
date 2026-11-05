<script lang="ts">
  import BrandMark from '../../../../vibemux_chat/web/src/components/brand_mark.svelte';
  import { ask_title } from '../lib/palette_entries';
  import { phase_label } from '../lib/native_state';
  import { chat_can_manage, chat_phase_label, chat_title } from '../lib/chat_state';
  import type { ChatManagement, ChatSession } from '../lib/chat_contract';
  import type { AskReply, AskStatus, ProjectRow, PublicSettings } from '../lib/ui_contract';
  let { projects, statuses, replies, chats_by_project = new Map<string, readonly ChatSession[]>(), chat_titles = new Map<string, string>(), selected_chat = null, selected_project, selected_ask, screen = 'chat', on_navigate, collapsed = false, disabled = false, fixture_mode = false, on_select_project, on_select_ask, on_select_chat, on_new_chat, on_chat_action, on_pick, on_new, on_settings, on_palette, on_shortcuts, on_toggle, on_project_action }: {
    projects: ProjectRow[]; statuses: ReadonlyMap<string, AskStatus>; replies: ReadonlyMap<string, AskReply>;
    chats_by_project?: ReadonlyMap<string, readonly ChatSession[]>; chat_titles?: ReadonlyMap<string, string>; selected_chat?: string | null;
    selected_project: string | null; selected_ask: string | null; screen?: PublicSettings['start_page']; on_navigate: (screen: PublicSettings['start_page']) => void; collapsed?: boolean; disabled?: boolean; fixture_mode?: boolean;
    on_select_project: (project_key: string) => void; on_select_ask: (project_key: string, ask_id: string) => void;
    on_select_chat: (project_key: string, chat_id: string) => void; on_new_chat: () => void; on_chat_action: (project_key: string, chat_id: string, action: Extract<ChatManagement, 'close' | 'archive' | 'resume' | 'purge'>) => void;
    on_pick: () => void; on_new: () => void; on_settings: () => void; on_palette: () => void; on_shortcuts: () => void; on_toggle: () => void;
    on_project_action: (project_key: string, action: 'start' | 'stop' | 'forget') => void;
  } = $props();
  let query = $state('');
  let expanded = $state<Record<string, boolean>>({});
  const filtered = $derived.by(() => {
    const needle = query.trim().toLocaleLowerCase();
    return projects.flatMap((project) => {
      const project_matches = !needle || project.name.toLocaleLowerCase().includes(needle);
      const chats = (chats_by_project.get(project.project_key) ?? []).filter((chat) => {
        return project_matches || [chat.chat_id, chat_title(chat, chat_titles), chat_phase_label(chat.state), chat.harness].some((text) => text.toLocaleLowerCase().includes(needle));
      });
      const ask_ids = project.ask_ids.filter((ask_id) => {
        const reply = replies.get(ask_id), status = statuses.get(ask_id);
        return project_matches || chats.length > 0 || [ask_id, ask_title(ask_id, reply), reply?.reply ?? '', status?.harness ?? '', status ? phase_label(status) : ''].some((text) => text.toLocaleLowerCase().includes(needle));
      });
      return project_matches || ask_ids.length || chats.length ? [{ project, chats, ask_ids }] : [];
    });
  });
</script>

<aside class="sidebar native_sidebar" class:collapsed aria-label="Projects and retained Asks">
  <div class="sidebar_brand_row"><div class="brand"><BrandMark />{#if !collapsed}<span>VibeMux</span>{/if}</div><button class="icon_button" aria-label={collapsed ? 'Expand sidebar' : 'Collapse sidebar'} aria-pressed={collapsed} title="Toggle sidebar · Ctrl / ⌘ + B" onclick={on_toggle}>{collapsed ? '›' : '‹'}</button></div>
  <button class="new_ask" title="New saved chat · preview" aria-label="New saved chat" onclick={on_new_chat} disabled={!selected_project || fixture_mode}>{collapsed ? '＋' : '＋ New chat'}</button>
  <button class="new_ask" title="New independent Ask · Ctrl / ⌘ + N" aria-label="New independent Ask" onclick={on_new} disabled={!selected_project}>{collapsed ? '◎' : '◎ New Ask'}</button>
  <button class="sidebar_search_button" title="Search projects and conversations · Ctrl / ⌘ + K" aria-label="Open command palette" onclick={on_palette}>{collapsed ? '⌕' : '⌕ Search everything'}</button>
  {#if !collapsed}<label class="sidebar_filter"><span class="sr_only">Filter projects, saved chats and retained Asks</span><input placeholder="Filter projects, chats and Asks" maxlength="256" bind:value={query} /></label><div class="sidebar_label"><span>PROJECTS · {projects.length}</span><button class="icon_button" aria-label="Choose project folder" disabled={disabled || fixture_mode} onclick={on_pick}>＋</button></div>{/if}
  <nav class="project_list">
    {#each filtered as { project, chats, ask_ids } (project.project_key)}
      {@const is_expanded = Boolean(query) || (expanded[project.project_key] ?? project.project_key === selected_project)}
      <div class="project_row">
        {#if !collapsed}<button class="tree_toggle" aria-label={(is_expanded ? 'Collapse ' : 'Expand ') + project.name} aria-expanded={is_expanded} onclick={() => expanded[project.project_key] = !is_expanded}>{is_expanded ? '⌄' : '›'}</button>{/if}
        <button class="project_button" class:selected={selected_project === project.project_key} title={project.name + ' · daemon ' + project.daemon} aria-label={project.name + (project.missing ? ' · missing folder' : '')} disabled={disabled || project.missing} onclick={() => on_select_project(project.project_key)}><span class="status_dot" class:running={project.daemon === 'running'}></span><span>{collapsed ? project.name.slice(0, 1).toUpperCase() : project.name}</span></button>
        {#if !collapsed}<details class="project_menu"><summary class="icon_button" aria-label={'Actions for ' + project.name}>⋯</summary><div class="project_menu_items"><button disabled={disabled || fixture_mode || project.missing} onclick={() => on_project_action(project.project_key, project.daemon === 'running' ? 'stop' : 'start')}>{project.daemon === 'running' ? 'Stop daemon…' : 'Start daemon'}</button><button disabled={disabled || fixture_mode} onclick={() => on_project_action(project.project_key, 'forget')}>Forget project…</button></div></details>{/if}
      </div>
      {#if !collapsed && project.missing}<p class="project_missing">Folder missing · use project menu to forget</p>{/if}
      {#if !collapsed && is_expanded && chats.length}<div class="chat_list" aria-label="Saved chats">
        {#each chats as chat (chat.chat_id)}
          <div class="chat_row">
            <button class:active={screen === 'chat' && selected_chat === chat.chat_id && selected_project === project.project_key} disabled={disabled || project.missing} title={chat_title(chat, chat_titles) + ' · ' + chat_phase_label(chat.state)} onclick={() => on_select_chat(project.project_key, chat.chat_id)}>
              <span>{chat_title(chat, chat_titles)}</span><small>{chat_phase_label(chat.state)} · {chat.harness === 'claude' ? 'Claude' : 'Codex'} · saved</small>
            </button>
            <details class="project_menu"><summary class="icon_button" aria-label={'Actions for ' + chat_title(chat, chat_titles)}>⋯</summary><div class="project_menu_items">
              <button disabled={disabled || fixture_mode || !chat_can_manage(chat, 'close')} onclick={() => on_chat_action(project.project_key, chat.chat_id, 'close')}>Close chat</button>
              <button disabled={disabled || fixture_mode || !chat_can_manage(chat, 'archive')} onclick={() => on_chat_action(project.project_key, chat.chat_id, 'archive')}>Archive chat</button>
              <button disabled={disabled || fixture_mode || !chat_can_manage(chat, 'resume')} onclick={() => on_chat_action(project.project_key, chat.chat_id, 'resume')}>Resume chat</button>
              <button disabled={disabled || fixture_mode || !chat_can_manage(chat, 'purge')} onclick={() => on_chat_action(project.project_key, chat.chat_id, 'purge')}>Purge chat…</button>
            </div></details>
          </div>
        {/each}
      </div>{/if}
      {#if !collapsed && is_expanded}<div class="ask_list">
        {#each [...ask_ids].reverse() as ask_id (ask_id)}{@const status = statuses.get(ask_id)}<button class:active={screen === 'chat' && selected_ask === ask_id && selected_project === project.project_key} disabled={disabled || project.missing} title={ask_title(ask_id, replies.get(ask_id))} onclick={() => on_select_ask(project.project_key, ask_id)}><span>{ask_title(ask_id, replies.get(ask_id))}</span><small>{status ? phase_label(status) : 'Retained Ask'} · one-shot · not saved</small></button>{/each}
        {#if !ask_ids.length}<p class="sidebar_empty">{query ? 'No matching retained Asks.' : 'No Asks in this app session.'}</p>{/if}
      </div>{/if}
    {/each}
    {#if !filtered.length && !collapsed}<p class="sidebar_empty">{projects.length ? 'No projects or retained text match this filter.' : 'Choose a folder to add your first project.'}</p>{/if}
  </nav>
  <div class="sidebar_bottom">
    {#if collapsed}<button aria-label="Choose project folder" title="Choose project folder" onclick={on_pick} disabled={disabled || fixture_mode}>▣</button>{/if}
    <button class:active={screen === 'chat'} aria-current={screen === 'chat' ? 'page' : undefined} aria-label="Open conversation" title="Conversation" onclick={() => on_navigate('chat')}>{collapsed ? '◌' : 'Conversation'}</button>
    <button class:active={screen === 'tasks'} aria-current={screen === 'tasks' ? 'page' : undefined} aria-label="Open tasks" title="Tasks" onclick={() => on_navigate('tasks')}>{collapsed ? '☷' : 'Tasks'}</button>
    <button class:active={screen === 'console'} aria-current={screen === 'console' ? 'page' : undefined} aria-label="Open console" title="Console" onclick={() => on_navigate('console')}>{collapsed ? '▤' : 'Console'}</button>
    <button aria-label="Open settings" title="Settings · Ctrl / ⌘ + ," onclick={on_settings}>{collapsed ? '⚙' : '⚙ Settings'}</button><button aria-label="Show keyboard shortcuts" title="Keyboard shortcuts · Ctrl / ⌘ + /" onclick={on_shortcuts}>{collapsed ? '?' : '? Keyboard shortcuts'}</button>
  </div>
</aside>

<style>
  .sidebar_bottom button.active { background:var(--accent_tint); color:var(--accent_text); }
</style>
