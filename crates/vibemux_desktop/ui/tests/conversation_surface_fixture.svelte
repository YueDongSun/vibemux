<script lang="ts">
  import { tick, type Component } from 'svelte';
  import PersistentChat from '../src/components/persistent_chat.svelte';
  import WorkspaceChats from '../src/components/workspace_chats.svelte';
  import ChatExport from '../src/components/chat_export.svelte';
  import type { ChatSession, ChatTurn } from '../src/lib/chat_contract';
  import type { ChatState, ChatView } from '../src/lib/chat_state';
  import type { WorkspaceChatView } from '../src/lib/workspace_chat_state';
  import type { WorkspaceOwner } from '../src/lib/workspace_chat_contract';
  import type { WorkspaceExecutionState, WorkspaceExecutionView } from '../src/lib/workspace_execution_state';
  import type { TextAttachment } from '../src/lib/text_attachments';
  import { find_text_matches, range_for_match, transcript_text } from '../src/lib/transcript_search';

  const PROJECT_KEY = 'synthetic_surface_project';
  const PROJECT_ID = '00000000-0000-4000-8000-000000000001';
  const OBSERVED_AT = '2026-11-03T00:00:00Z';
  const MARKERS = { saved: 'DEMO_SAVED_SURFACE_FIND_TARGET', workspace: 'DEMO_WORKSPACE_SURFACE_FIND_TARGET' } as const;
  const REFERENCE_VIEWPORT = { width: 1280, height: 850 } as const;
  const MIN_TRANSCRIPT_HEIGHT = 240;
  const SMALL_TRANSCRIPT_HEIGHT = 120;
  const DIFF_LINES = 300;
  const MARKER_LINE = 270;
  type Surface = keyof typeof MARKERS;

  let surface = $state<Surface>('saved');
  let find_open = $state(false);
  let consent_accepted = $state(false);
  let saved_draft = $state('');
  let workspace_draft = $state('');
  let saved_attachments = $state<TextAttachment[]>([]);
  let workspace_files = $state<TextAttachment[]>([]);
  let mutation_calls = $state(0);
  let mutation_methods = $state<string[]>([]);
  let read_calls = $state(0);
  let report = $state('Ready · synthetic closed conversations · reference viewport 1280 × 850');
  let checking = $state(false);
  let fixture_root = $state<HTMLElement>();

  // Runtime prop forwarding intentionally tolerates the pre-fix wrapper, which
  // ignores the two Find props. Both real components still mount before the
  // patch; the Workspace check then fails because its Find input is absent.
  const WorkspaceSurface = WorkspaceChats as unknown as Component<Record<string, unknown>>;
  const bytes = (text: string) => new TextEncoder().encode(text).length;
  function mutation(name: string): Promise<boolean> {
    mutation_calls++; mutation_methods = [...mutation_methods, name];
    return Promise.resolve(false);
  }
  function read(): Promise<void> { read_calls++; return Promise.resolve(); }
  function local_edit(): void { /* Synthetic draft changes never leave this fixture. */ }
  const saved_controller = {
    refresh: read, poll: read, load_more_sessions: read, load_more_history: read, load_more_turns: read,
    set_archived: read, set_draft: (value: string) => { saved_draft = value; }, set_harness: local_edit,
    remove_file: (id: string) => { saved_attachments = saved_attachments.filter((file) => file.attachment_id !== id); },
    pick_files: () => mutation('saved_pick_files'), check_pending: read,
    send: () => mutation('saved_send'), cancel: () => mutation('saved_cancel'),
    manage: () => mutation('saved_manage'), retry_pending: () => mutation('saved_retry'),
  } as unknown as ChatState;
  const execution_controller = {
    refresh: read, load_more: read, set_draft: (value: string) => { workspace_draft = value; },
    remove_file: (id: string) => { workspace_files = workspace_files.filter((file) => file.attachment_id !== id); },
    pick_files: () => mutation('workspace_pick_files'), retry_pending: () => mutation('workspace_retry'),
    start: () => mutation('workspace_start_or_resume'), send: () => mutation('workspace_send'),
    cancel: () => mutation('workspace_cancel'), close: () => mutation('workspace_close'),
  } as unknown as WorkspaceExecutionState;

  function session(chat_id: string): ChatSession {
    return { schema_version: 1, chat_id, project_id: PROJECT_ID, harness: 'codex', state: 'closed',
      generation: 1, version: 10, active_request_id: null, record_count: 4, last_record_sequence: 4,
      content_purged: false, has_title: true, created_at: OBSERVED_AT, updated_at: OBSERVED_AT };
  }
  function bodies(target: Surface): { kind: 'user' | 'tool' | 'assistant'; text: string }[] {
    const command = 'synthetic reported command';
    const output = 'Synthetic command output; reported failure does not establish a Task outcome.';
    const diff = Array.from({ length: DIFF_LINES }, (_, index) => index === MARKER_LINE
      ? `+${MARKERS[target]}` : `+DEMO ${target} surface line ${index}`).join('\n');
    return [
      { kind: 'user', text: `DEMO ${target} retained prompt 中文` },
      { kind: 'tool', text: JSON.stringify({ format: 'vibemux_command_tool_record', schema_version: 1,
        call_id: 'same_reported_command_id', status: 'failed', command, command_original_bytes: bytes(command),
        command_truncated: false, working_directory: 'C:\\DEMO\\synthetic', output, output_original_bytes: bytes(output),
        output_truncated: false, exit_code: 7, duration_ms: 1 }) },
      { kind: 'tool', text: JSON.stringify({ format: 'vibemux_file_change_tool_record', schema_version: 1,
        call_id: 'same_reported_file_id', status: 'completed', original_change_count: 3, changes_truncated: false,
        changes: [
          { path: `DEMO_${target}.txt`, kind: 'update', move_path: `DEMO_${target}_moved.txt`, diff,
            diff_original_bytes: bytes(diff), diff_truncated: false },
          { path: 'DEMO_added.txt', kind: 'add', move_path: null, diff: '+DEMO added\n', diff_original_bytes: 12, diff_truncated: false },
          { path: 'DEMO_deleted.txt', kind: 'delete', move_path: null, diff: '-DEMO deleted\n', diff_original_bytes: 14, diff_truncated: false },
        ] }) },
      { kind: 'assistant', text: `Synthetic ${target} retained answer. No process is running.` },
    ];
  }
  const saved_session = session('saved_chat');
  const workspace_session = session('workspace_chat');
  const saved_turn: ChatTurn = { schema_version: 1, chat_id: saved_session.chat_id, turn_id: 'saved_turn',
    request_id: 'saved_request', task_id: 'saved_task', run_id: 'saved_run', generation: 1, version: 3,
    phase: 'completed', admitted_sequence: 1, outcome: 'completed', error_code: null,
    correlated_terminal: true, output_complete: true, created_at: OBSERVED_AT, updated_at: OBSERVED_AT };
  const owner: WorkspaceOwner = { schema_version: 1, chat_id: workspace_session.chat_id, project_id: PROJECT_ID,
    task_id: 'workspace_task', run_id: 'workspace_run', harness: 'codex', phase: 'open', version: 5,
    run_status: 'stopped', admitted_tools: [], has_title: true, workspace_planned: true,
    workspace_branch: 'codex/run_synthetic', workspace_effect: 'settled', workspace_verified_at: OBSERVED_AT,
    provisioning_active: false, error_code: null, created_at: OBSERVED_AT, updated_at: OBSERVED_AT };
  const saved_records = bodies('saved').map(({ kind, text }, index) => ({
    record: { record_id: `saved_record_${index}`, chat_id: saved_session.chat_id, turn_id: saved_turn.turn_id,
      sequence: index + 1, kind, original_byte_count: bytes(text), retained_byte_count: bytes(text),
      content_available: true, truncated: false, created_at: OBSERVED_AT }, text, complete: true, error: null,
  }));
  const saved_view = $derived<ChatView>({ revision: 1, project_key: PROJECT_KEY, confirmed: consent_accepted,
    selected_chat: saved_session.chat_id, session: saved_session, sessions: new Map([[PROJECT_KEY, [saved_session]]]),
    titles: new Map([[saved_session.chat_id, 'DEMO Saved surface']]), draft: saved_draft, attachments: saved_attachments, picking_files: false,
    new_title: '', harness: 'codex', archived: false, more_sessions: false, turns: [saved_turn], records: saved_records,
    live: null, content_more: false, turns_more: false, history_capped: false, loading: false, busy: false,
    available: true, error: '', notice: '', pending: null });
  const workspace_view: WorkspaceChatView = { project_key: PROJECT_KEY, owners: [owner], selected_owner: owner.chat_id,
    owner, catalogue: new Map([[PROJECT_KEY, { owners: [owner], titles: new Map([[owner.chat_id, 'DEMO Workspace surface']]) }]]),
    title: 'DEMO Workspace surface', loading: false, pending: null, error: null, more: false };
  const execution_view = $derived<WorkspaceExecutionView>({ revision: 1, project_key: PROJECT_KEY, chat_id: owner.chat_id,
    owner, session: workspace_session, process_claimed: false, runtime_known: true, draft: workspace_draft, files: workspace_files,
    attachment_rejections: [], attempts: [{ schema_version: 1, chat_id: owner.chat_id, attempt_id: 'workspace_attempt',
      request_id: 'workspace_request', task_id: owner.task_id, run_id: owner.run_id, owner_version: owner.version,
      generation: 1, version: 3, phase: 'completed', admitted_sequence: 1, outcome: 'completed', error_code: null,
      correlated_terminal: true, output_complete: true, created_at: OBSERVED_AT, updated_at: OBSERVED_AT }],
    records: bodies('workspace').map(({ kind, text }, index) => ({
      record: { record_id: `workspace_record_${index}`, chat_id: owner.chat_id, attempt_id: 'workspace_attempt',
        sequence: index + 1, kind, original_byte_count: bytes(text), retained_byte_count: bytes(text),
        content_available: true, truncated: false, created_at: OBSERVED_AT }, text, complete: true, error: null,
    })), live: null, attempts_more: false, content_more: false, history_capped: false, loading: false,
    busy: false, error: '', notice: '', pending: null, can_start: true, can_send: false, can_cancel: false, can_close: false });
  const workspace_props = $derived({ view: workspace_view, project_name: 'DEMO synthetic project', execution_view,
    execution_controller, presentation: 'codex', notify: local_edit, on_artifact: local_edit,
    on_refresh: read, on_load_more: read, on_select: local_edit,
    on_create: () => mutation('workspace_create'), on_reconcile: () => mutation('workspace_reconcile'),
    on_stop: () => mutation('workspace_stop'), on_retry: () => mutation('workspace_owner_retry'),
    find_open, on_find_close: close_find });

  export async function select_surface(next: Surface): Promise<void> {
    if (next !== 'saved' && next !== 'workspace') throw new Error('Unknown synthetic surface');
    find_open = false; surface = next; await tick();
    report = `Selected ${surface}; Find closed; mutation calls ${mutation_calls}.`;
  }
  export async function open_find(): Promise<void> { find_open = true; await tick(); }
  export function close_find(): void { find_open = false; }
  function seed_attachment(): void {
    const files = [{ attachment_id: 'synthetic_local_attachment', name: 'DEMO_local.txt', content: 'local', byte_count: 5 }];
    if (surface === 'saved') saved_attachments = files; else workspace_files = files;
  }
  function current_transcript(): HTMLElement {
    const roots = fixture_root?.querySelectorAll<HTMLElement>('[data-fixture-current] [data-transcript-scroll]');
    if (roots?.length !== 1) throw new Error('Expected one actual selected transcript');
    return roots[0];
  }
  export function inspect_geometry() {
    const root = current_transcript();
    const header = fixture_root?.querySelector<HTMLElement>('[data-fixture-header]');
    if (!header) throw new Error('Missing synthetic app header');
    const outer_scroll: { element: string; top: number; left: number }[] = [];
    for (let ancestor = root.parentElement; ancestor; ancestor = ancestor.parentElement) {
      outer_scroll.push({ element: ancestor.className || ancestor.tagName, top: ancestor.scrollTop, left: ancestor.scrollLeft });
    }
    return { surface, viewport: { width: innerWidth, height: innerHeight }, reference_viewport: REFERENCE_VIEWPORT,
      transcript_height: root.clientHeight,
      usable_transcript_height: root.clientHeight - parseFloat(getComputedStyle(root).paddingTop) - parseFloat(getComputedStyle(root).paddingBottom),
      transcript_scroll: root.scrollTop, header_top: header.getBoundingClientRect().top,
      page_outer_scroll: document.scrollingElement?.scrollTop ?? 0, outer_scroll, mutation_calls,
      mutation_methods: [...mutation_methods], read_calls, find_inputs: fixture_root?.querySelectorAll('.transcript_find input').length ?? 0 };
  }
  function require_check(condition: unknown, message: string): asserts condition { if (!condition) throw new Error(message); }
  export async function check_target_find() {
    checking = true;
    try {
      find_open = false; await tick();
      const root = current_transcript();
      for (const details of root.querySelectorAll('details')) details.open = false;
      root.scrollTop = 0;
      const before = inspect_geometry();
      await open_find();
      const input = fixture_root?.querySelector<HTMLInputElement>('.transcript_find input');
      require_check(input, `${surface} did not mount the actual transcript Find input`);
      input.value = MARKERS[surface]; input.dispatchEvent(new Event('input', { bubbles: true }));
      await tick(); await new Promise<void>((resolve) => requestAnimationFrame(() => resolve()));
      await tick(); await new Promise<void>((resolve) => requestAnimationFrame(() => resolve()));
      const rows = [...root.querySelectorAll<HTMLElement>('[data-search-record]')];
      const selected = rows.filter((row) => transcript_text(row).text.includes(MARKERS[surface]));
      require_check(selected.length === 1, 'Expected exactly one target-specific retained file record');
      require_check(selected[0].querySelector('summary')?.textContent?.includes('File changes · reported completed'), 'Expected the real structured file card');
      const snapshot = transcript_text(selected[0]);
      const matches = find_text_matches(snapshot.text, MARKERS[surface]).matches;
      require_check(matches.length === 1, 'Expected exactly one late diff marker');
      const range = range_for_match(snapshot.parts, matches[0]);
      require_check(range, 'Expected a real retained-text range');
      const rect = range.getBoundingClientRect(), viewport = root.getBoundingClientRect();
      const disclosures: HTMLDetailsElement[] = [];
      for (let element = range.startContainer.parentElement; element && selected[0].contains(element); element = element.parentElement) {
        if (element.tagName === 'DETAILS') disclosures.push(element as HTMLDetailsElement);
      }
      const details_open = disclosures.length > 0 && disclosures.every((details) => details.open);
      const range_visible = rect.top >= Math.max(viewport.top + root.clientTop - 1, 0)
        && rect.bottom <= Math.min(viewport.top + root.clientTop + root.clientHeight + 1, innerHeight);
      const after = inspect_geometry();
      require_check(details_open, 'The real Find did not open the selected file disclosure');
      require_check(range_visible, 'The late diff match is outside the actual transcript viewport');
      require_check(!root.textContent?.includes(MARKERS[surface === 'saved' ? 'workspace' : 'saved']), 'Another target leaked into this transcript');
      const minimum_height = innerHeight >= REFERENCE_VIEWPORT.height ? MIN_TRANSCRIPT_HEIGHT : SMALL_TRANSCRIPT_HEIGHT;
      require_check(after.usable_transcript_height >= minimum_height, `Actual usable transcript is shorter than ${minimum_height}px`);
      require_check(Math.abs(after.header_top - before.header_top) < 1, 'The app header moved during Find');
      require_check(after.page_outer_scroll === 0 && after.outer_scroll.every((row) => row.top === 0), 'An outer page or surface scrolled');
      require_check(after.mutation_calls === 0, 'A mutation callback was invoked');
      const result = { pass: true, details_open, range_visible, ...after };
      report = JSON.stringify(result, null, 2); return result;
    } catch (error) {
      const result = { pass: false, error: error instanceof Error ? error.message : 'Synthetic fixture failure', ...inspect_geometry() };
      report = JSON.stringify(result, null, 2); return result;
    } finally { checking = false; }
  }
  async function inspect(): Promise<void> { await tick(); report = JSON.stringify(inspect_geometry(), null, 2); }
</script>

<div class="native_shell" bind:this={fixture_root}>
  <aside class="sidebar fixture_panel" aria-label="Synthetic regression controls">
    <strong>DEMO conversation surfaces</strong>
    <p class="small">Actual production components and styles. Synthetic closed history; no daemon or provider calls.</p>
    <div class="button_row"><button disabled={checking} aria-pressed={surface === 'saved'} onclick={() => void select_surface('saved')}>Saved</button><button disabled={checking} aria-pressed={surface === 'workspace'} onclick={() => void select_surface('workspace')}>Workspace</button></div>
    <div class="button_row"><button disabled={checking} onclick={() => void open_find()}>Open Find</button><button disabled={checking} onclick={close_find}>Close Find</button></div>
    <label class="small"><input type="checkbox" disabled={checking} bind:checked={consent_accepted} /> Synthetic saved consent accepted</label>
    <button disabled={checking} onclick={() => void check_target_find()}>Check selected surface</button>
    <button onclick={() => void inspect()}>Inspect geometry</button>
    <button onclick={seed_attachment}>Seed local attachment</button>
    <p class="small">Required usable transcript: 240px at 1280 × 850; 120px at 900 × 600.</p>
    <p class="small">Mutation calls: <strong data-fixture-mutation-count>{mutation_calls}</strong></p>
    <pre class="fixture_report" role="status" aria-label="Conversation surface regression result">{report}</pre>
  </aside>
  <main class="main">
    <header class="top_bar" data-fixture-header><div class="breadcrumbs"><span>DEMO synthetic project</span><strong>Retained {surface} conversation</strong></div><button disabled={checking} aria-label="Find in current transcript" onclick={() => void open_find()}>Find</button></header>
    <div class="workspace_body"><div class="conversation_column">
      <div class="project_status_bar"><span class="status_dot"></span><span>DEMO · closed session · four retained records</span></div>
      <div class="chat_mode_row" role="tablist" aria-label="Synthetic conversation mode"><button role="tab" disabled={checking} aria-selected={surface === 'saved'} onclick={() => void select_surface('saved')}>Chats</button><button role="tab" disabled={checking} aria-selected={surface === 'workspace'} onclick={() => void select_surface('workspace')}>Workspaces</button></div>
      <ChatExport view={{ format: 'text', busy: false, can_export: true, status: 'idle', error: '', saved: null }} on_format={local_edit} on_export={() => void mutation('export')} />
      <div class="fixture_current" data-fixture-current>
        {#if surface === 'saved'}
          <PersistentChat view={saved_view} controller={saved_controller} harness="codex" harness_available={true}
            notify={local_edit} on_artifact={local_edit} on_consent={() => void mutation('saved_consent')}
            on_purge={() => void mutation('saved_purge')} on_new_chat={() => void mutation('saved_new_chat')}
            {find_open} on_find_close={close_find} />
        {:else}
          <WorkspaceSurface {...workspace_props} />
        {/if}
      </div>
    </div></div>
  </main>
</div>

<style>
  .fixture_panel { overflow: auto; }
  .fixture_panel p { margin: 0; }
  .fixture_report { margin: 0; white-space: pre-wrap; overflow-wrap: anywhere; font-size: 10px; }
  .fixture_current { display: contents; }
</style>
