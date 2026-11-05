import { replace_command_progress, visible_command_progress, type CommandProgressView } from './command_progress.ts';
import { CHAT_LIMITS, type ChatBackend, type ChatContentError, type ChatCursor, type ChatLiveChunk, type ChatManagement, type ChatMutation, type ChatPhase, type ChatRecord, type ChatRequest, type ChatResponse, type ChatSession, type ChatTurn } from './chat_contract.ts';
import type { AskTargets, Harness } from './ui_contract.ts';
import { format_attachment_prompt, merge_attachments, remove_attachment, type TextAttachment } from './text_attachments.ts';

export interface ChatRecordView { record: ChatRecord; text: string; complete: boolean; error: ChatContentError | null }
export interface ChatLiveView extends ChatLiveChunk { request_id: string; generation: number }
/** The one uncertain write. `request` is the exact immutable payload to replay;
 * `turn_request_id` is the bound turn id for open/send chains (null for lifecycle). */
interface SubmittedDraft { key: string; text: string; files: TextAttachment[] }
export interface PendingChat { project_key: string; request: ChatMutation; state: 'sending' | 'uncertain'; code: string; prompt: string; turn_request_id: string | null; draft?: SubmittedDraft }
export interface ChatView {
  revision: number; project_key: string | null; confirmed: boolean; selected_chat: string | null;
  session: ChatSession | null; sessions: ReadonlyMap<string, readonly ChatSession[]>; titles: ReadonlyMap<string, string>;
  draft: string; attachments: TextAttachment[]; picking_files: boolean; new_title: string; harness: Harness; archived: boolean; more_sessions: boolean;
  turns: ChatTurn[]; records: ChatRecordView[]; live: ChatLiveView | null; command_progress: CommandProgressView | null; progress_error: string; content_more: boolean; turns_more: boolean;
  history_capped: boolean; loading: boolean; busy: boolean; available: boolean; error: string; notice: string; pending: PendingChat | null;
}
interface ProjectChat {
  confirmed: boolean; project_id: string | null; selected: string | null; search_pinned: string | null; sessions: Map<string, ChatSession>; listed: string[];
  drafts: Map<string, string>; attachments: Map<string, TextAttachment[]>; new_title: string; harness: Harness; archived: boolean; after: string | null; more: boolean;
  available: boolean; error: string; notice: string; loading: number; binding: { operation_id: string; request_id: string } | null;
  reserved: Map<string, string>; turns: Map<string, ChatTurn>; turn_after: number; turns_more: boolean;
  records: ChatRecordView[]; cursor: ChatCursor; content_more: boolean; history_loaded: boolean; history_capped: boolean; live: ChatLiveView | null; command_progress: CommandProgressView | null; progress_error: string; progress_unavailable: boolean;
}
function fresh_project(): ProjectChat {
  return { confirmed: false, project_id: null, selected: null, search_pinned: null, sessions: new Map(), listed: [], drafts: new Map(), attachments: new Map(), new_title: '', harness: 'claude', archived: false, after: null, more: false, available: false, error: '', notice: '', loading: 0, binding: null, reserved: new Map(), turns: new Map(), turn_after: 0, turns_more: false, records: [], cursor: { after_sequence: 0, byte_offset: 0 }, content_more: false, history_loaded: false, history_capped: false, live: null, command_progress: null, progress_error: '', progress_unavailable: false };
}
function require_contract(condition: unknown): asserts condition { if (!condition) throw { code: 'chat_contract_invalid' }; }
function safe_count(value: number, minimum = 0): boolean { return Number.isSafeInteger(value) && value >= minimum; }
/** Only renderer/Rust pre-dispatch validation is definitive: multi-stage
 * service operations can commit metadata before failing (for example Open
 * claims the owner before writing), so every service error keeps the exact-ID
 * binding uncertain for read-only lookup or explicit retry. A null lookup is
 * not proof a queued request cannot still commit, so nothing auto-resends. */
const CHAT_REJECTIONS = new Set(['invalid_arguments', 'chat_consent_required', 'prompt_frame_too_large']);
/** Ready-wait bounds: real Open returns `opening` while the vendor CLI
 * initializes, which can take seconds. The wait is bounded by elapsed time
 * matching the server's 30 s initialize bound — never by RPC count, which a
 * fast loop would exhaust in milliseconds — and yields between reads. */
const CHAT_READY_TIMEOUT_MS = 30_000;
const CHAT_READY_INTERVAL_MS = 250;
/** Injectable clock for deterministic tests; production uses real time. */
export interface ChatTimer { now(): number; sleep(milliseconds: number): Promise<void> }
export function default_chat_timer(): ChatTimer {
  return { now: () => Date.now(), sleep: (milliseconds) => new Promise((resolve) => setTimeout(resolve, milliseconds)) };
}
/** Read-only chat-state evidence that a lifecycle mutation landed. Rename is
 * not observable through content-free status and needs a same-ID retry. */
function chat_mutation_observed(operation: 'close' | 'archive' | 'resume' | 'purge', session: ChatSession): boolean {
  if (operation === 'close') return ['closed', 'archived'].includes(session.state);
  if (operation === 'archive') return session.state === 'archived';
  if (operation === 'resume') return ['opening', 'ready', 'busy'].includes(session.state);
  return session.content_purged;
}
export function chat_bytes(text: string): number { return new TextEncoder().encode(text).length; }
export function chat_error(error: unknown): string {
  return error && typeof error === 'object' && 'code' in error && typeof error.code === 'string' && /^[a-z][a-z0-9_]{0,95}$/.test(error.code) ? error.code : 'chat_operation_failed';
}
export function chat_phase_label(state: ChatPhase): string {
  return ({ opening: 'Opening', ready: 'Ready', busy: 'Running', suspended: 'Suspended', recovery_required: 'Recovery required', closed: 'Closed', archived: 'Archived' })[state];
}
export function chat_title(session: ChatSession, titles: ReadonlyMap<string, string>): string { return titles.get(session.chat_id) || 'Chat ' + session.chat_id.slice(0, 12); }
export function chat_can_manage(session: ChatSession | null, action: ChatManagement): boolean {
  if (!session) return false;
  if (action === 'rename') return !session.content_purged;
  if (action === 'resume') return !session.content_purged && !session.active_request_id && ['suspended', 'closed'].includes(session.state);
  if (action === 'archive') return !session.active_request_id && ['suspended', 'closed', 'archived'].includes(session.state);
  if (action === 'purge') return !session.content_purged && !session.active_request_id && session.state === 'archived';
  return ['opening', 'ready', 'busy', 'suspended', 'closed'].includes(session.state);
}
export function chat_prompt_error(prompt: string): string { return !prompt.trim() ? 'Enter a message.' : chat_bytes(prompt) > CHAT_LIMITS.prompt_bytes ? 'Keep the message within 32 KiB of UTF-8 text.' : ''; }
/** Route facts for the persistent-chat harness selection. This is derived
 * from the chat's own harness (selected session, else the new-chat choice),
 * never from the independent-Ask picker. */
export function chat_route_profile(harness: Harness, targets: AskTargets | undefined): { available: boolean; reason: string | null; profile: string } {
  const target = targets?.targets.find((row) => row.harness === harness);
  return {
    available: target?.available ?? false,
    reason: target ? target.reason : targets ? null : 'Checking configured routes…',
    profile: harness === 'claude' ? 'Claude Code: tool-free, one turn at a time.' : 'Codex: configured read-only; approvals are denied.',
  };
}

/** Renderer-only controller. All requests are serialized; only explicit user
 * actions call mutations. Unknown writes retain the exact immutable binding. */
export class ChatState {
  private backend: ChatBackend;
  private changed: (view: ChatView) => void;
  private new_id: () => string;
  private timer: ChatTimer;
  private projects = new Map<string, ProjectChat>();
  private titles = new Map<string, string>();
  private project_key: string | null = null;
  private selection = 0;
  private revision = 0;
  private pending: PendingChat | null = null;
  private rpc_tail: Promise<unknown> = Promise.resolve();
  private action_job: Promise<boolean> | null = null;
  private read_jobs = new Map<string, Promise<void>>();
  private disposed = false;
  private picking_files = false;

  constructor(backend: ChatBackend, changed: (view: ChatView) => void = () => {}, new_id: () => string = () => crypto.randomUUID(), timer: ChatTimer = default_chat_timer()) { this.backend = backend; this.changed = changed; this.new_id = new_id; this.timer = timer; }
  private cache(key: string): ProjectChat { let value = this.projects.get(key); if (!value) { value = fresh_project(); this.projects.set(key, value); } return value; }
  snapshot(): ChatView {
    const cache = this.project_key ? this.cache(this.project_key) : fresh_project();
    const session = cache.selected ? cache.sessions.get(cache.selected) ?? null : null;
    return { revision: this.revision, project_key: this.project_key, confirmed: cache.confirmed, selected_chat: cache.selected, session,
      sessions: new Map([...this.projects].map(([key, value]) => [key, value.listed.flatMap((id) => value.sessions.get(id) ?? [])])), titles: new Map(this.titles),
      draft: cache.drafts.get(cache.selected ?? '') ?? '', attachments: structuredClone(cache.attachments.get(cache.selected ?? '') ?? []), picking_files: this.picking_files, new_title: cache.new_title, harness: session?.harness ?? cache.harness, archived: cache.archived, more_sessions: cache.more,
      turns: [...cache.turns.values()].sort((a, b) => a.admitted_sequence - b.admitted_sequence), records: [...cache.records], live: cache.live, command_progress: visible_command_progress(cache.command_progress, cache.records, session?.state === 'busy' && session.active_request_id === cache.command_progress?.request_id && ['admitted', 'running', 'cancel_requested'].includes(cache.turns.get(cache.command_progress?.request_id ?? '')?.phase ?? '')), progress_error: cache.progress_error,
      content_more: cache.content_more, turns_more: cache.turns_more, history_capped: cache.history_capped, loading: cache.loading > 0,
      busy: this.action_job !== null || this.pending?.state === 'sending', available: cache.available, error: cache.error, notice: cache.notice, pending: this.pending };
  }
  private publish(): void { if (!this.disposed) { this.revision++; this.changed(this.snapshot()); } }
  dispose(): void { for (const cache of this.projects.values()) this.clear_progress(cache); this.disposed = true; this.selection++; }
  private enqueue<T>(operation: () => Promise<T>): Promise<T> {
    const result = this.rpc_tail.then(() => { if (this.disposed) throw { code: 'chat_view_disposed' }; return operation(); });
    this.rpc_tail = result.then(() => undefined, () => undefined); return result;
  }
  private rpc(key: string, request: ChatRequest): Promise<ChatResponse> { return this.enqueue(() => this.backend.chat_request(key, request)); }
  private current(key: string, selected: string | null, selection: number, generation?: number): boolean {
    return !this.disposed && this.project_key === key && this.selection === selection && this.cache(key).selected === selected && (generation === undefined || this.cache(key).sessions.get(selected ?? '')?.generation === generation);
  }
  private clear_search_pin(cache: ProjectChat): void {
    const id = cache.search_pinned; cache.search_pinned = null;
    if (id && !cache.archived && cache.sessions.get(id)?.state === 'archived') cache.listed = cache.listed.filter((listed) => listed !== id);
  }
  private clear_progress(cache: ProjectChat): void { cache.command_progress = null; cache.progress_error = ''; cache.progress_unavailable = false; }
  private clear_detail(cache: ProjectChat): void { this.clear_progress(cache); cache.turns = new Map(); cache.turn_after = 0; cache.turns_more = false; cache.records = []; cache.cursor = { after_sequence: 0, byte_offset: 0 }; cache.content_more = false; cache.history_loaded = false; cache.history_capped = false; cache.live = null; }
  private apply_session(key: string, value: ChatSession): boolean {
    const cache = this.cache(key);
    require_contract(value.schema_version === 1 && typeof value.chat_id === 'string' && Boolean(value.chat_id) && typeof value.project_id === 'string' && Boolean(value.project_id)
      && ['claude', 'codex'].includes(value.harness) && ['opening', 'ready', 'busy', 'suspended', 'recovery_required', 'closed', 'archived'].includes(value.state)
      && safe_count(value.generation, 1) && safe_count(value.version, 1) && safe_count(value.record_count) && safe_count(value.last_record_sequence));
    if (cache.project_id && cache.project_id !== value.project_id) { this.clear_progress(cache); throw { code: 'chat_project_identity_changed' }; }
    cache.project_id = value.project_id;
    const previous = cache.sessions.get(value.chat_id);
    if (previous && (value.generation < previous.generation || value.version < previous.version)) return false;
    if (cache.selected === value.chat_id && (previous?.generation !== value.generation || previous?.active_request_id !== value.active_request_id)) cache.live = null;
    if (cache.selected === value.chat_id && (previous?.generation !== value.generation || (value.active_request_id !== null && previous?.active_request_id !== value.active_request_id))) this.clear_progress(cache);
    cache.sessions.set(value.chat_id, value);
    if (cache.archived || value.state !== 'archived' || (cache.search_pinned === value.chat_id && cache.selected === value.chat_id)) {
      if (!cache.listed.includes(value.chat_id)) cache.listed = [value.chat_id, ...cache.listed].slice(0, CHAT_LIMITS.sessions);
    } else cache.listed = cache.listed.filter((id) => id !== value.chat_id);
    if (value.content_purged) { this.titles.delete(value.chat_id); if (cache.selected === value.chat_id) { cache.records = []; cache.live = null; this.clear_progress(cache); cache.content_more = false; } }
    cache.available = true; return true;
  }
  private apply_turn(cache: ProjectChat, value: ChatTurn, chat_id: string): void {
    require_contract(value.chat_id === chat_id && safe_count(value.generation, 1) && safe_count(value.version, 1) && safe_count(value.admitted_sequence) && ['admitted', 'running', 'cancel_requested', 'completed', 'failed', 'cancelled', 'interrupted', 'recovery_required'].includes(value.phase));
    if (cache.selected !== chat_id) return;
    const previous = cache.turns.get(value.request_id);
    if (!previous || value.version >= previous.version) cache.turns.set(value.request_id, value);
  }
  private read_job(key: string, name: string, operation: () => Promise<void>): Promise<void> {
    const id = key + ':' + name + ':' + this.selection, existing = this.read_jobs.get(id);
    if (existing) return existing;
    const cache = this.cache(key); cache.loading++; this.publish();
    const job = Promise.resolve().then(operation).catch((error: unknown) => {
      if (chat_error(error) === 'chat_content_purged') this.clear_progress(cache);
      cache.error = 'Chat read failed (' + chat_error(error) + '). The last observation is retained.';
      if (['chat_project_identity_changed', 'control_unsupported_version'].includes(chat_error(error))) cache.available = false;
    }).finally(() => { cache.loading--; this.read_jobs.delete(id); this.publish(); });
    this.read_jobs.set(id, job); return job;
  }
  select_project(key: string | null, confirmed = false): Promise<void> {
    if (key === this.project_key) { if (key) { this.cache(key).confirmed = confirmed; this.publish(); } return Promise.resolve(); }
    if (this.project_key) this.clear_progress(this.cache(this.project_key));
    if (key) this.clear_progress(this.cache(key));
    this.selection++; this.project_key = key;
    if (!key) { this.publish(); return Promise.resolve(); }
    this.cache(key).confirmed = confirmed; this.publish(); return this.refresh();
  }
  update_consent(key: string, confirmed: boolean): void { if (this.cache(key).confirmed !== confirmed) { this.cache(key).confirmed = confirmed; this.publish(); } }
  set_draft(text: string): void { if (this.project_key) { const cache = this.cache(this.project_key); cache.drafts.set(cache.selected ?? '', text); this.publish(); } }
  async pick_files(): Promise<void> {
    const key = this.project_key;
    if (!key || this.picking_files || this.action_job) return;
    const cache = this.cache(key), selected = cache.selected, selection = this.selection;
    this.picking_files = true; this.publish();
    try {
      const picked = await this.backend.chat_pick_files(key);
      if (!this.current(key, selected, selection) || picked === null) return;
      const result = merge_attachments(cache.attachments.get(selected ?? '') ?? [], picked, cache.drafts.get(selected ?? '') ?? '');
      cache.attachments.set(selected ?? '', structuredClone(result.files));
      cache.error = result.rejected.length ? 'Some files could not be attached (' + result.rejected[0].reason + ').' : '';
    } catch (error) {
      if (this.current(key, selected, selection)) cache.error = 'Files could not be attached (' + chat_error(error) + ').';
    } finally { this.picking_files = false; this.publish(); }
  }
  remove_file(attachment_id: string): void {
    if (!this.project_key) return;
    const cache = this.cache(this.project_key), key = cache.selected ?? '';
    cache.attachments.set(key, remove_attachment(cache.attachments.get(key) ?? [], attachment_id));
    this.publish();
  }
  private carry_draft(cache: ProjectChat, chat_id: string): void {
    const text = cache.drafts.get('') ?? '';
    const files = cache.attachments.get('') ?? [];
    cache.drafts.delete(''); cache.attachments.delete('');
    cache.drafts.set(chat_id, text); cache.attachments.set(chat_id, files);
    if (this.pending?.draft?.key === '') this.pending.draft.key = chat_id;
  }
  private clear_submitted_draft(cache: ProjectChat, draft?: SubmittedDraft): void {
    if (!draft || (cache.drafts.get(draft.key) ?? '') !== draft.text) return;
    const files = cache.attachments.get(draft.key) ?? [];
    if (files.length !== draft.files.length || files.some((file, index) => file.attachment_id !== draft.files[index].attachment_id || file.content !== draft.files[index].content)) return;
    cache.drafts.delete(draft.key); cache.attachments.delete(draft.key);
  }
  set_title(text: string): void { if (this.project_key) { this.cache(this.project_key).new_title = text; this.publish(); } }
  set_harness(harness: Harness): void { if (this.project_key && !this.cache(this.project_key).selected) { this.cache(this.project_key).harness = harness; this.publish(); } }
  new_chat(): void { if (this.project_key) { const cache = this.cache(this.project_key); this.selection++; this.clear_search_pin(cache); cache.selected = null; this.clear_detail(cache); cache.error = ''; cache.notice = ''; this.publish(); } }
  select_chat(chat_id: string): Promise<void> {
    const key = this.project_key; if (!key) return Promise.resolve();
    const cache = this.cache(key);
    if (!cache.sessions.has(chat_id)) { cache.error = 'Load this chat from the selected project first.'; this.publish(); return Promise.resolve(); }
    if (cache.selected !== chat_id) { this.selection++; this.clear_search_pin(cache); cache.selected = chat_id; this.clear_detail(cache); }
    this.publish(); return this.refresh_selected();
  }
  /** Open a registered-project search observation without issuing a chat mutation. */
  async open_search_hit(target: { chat_id: string; generation: number; version: number; record_id?: string; sequence?: number }): Promise<boolean> {
    target = { ...target };
    const key = this.project_key;
    if (!key || this.disposed) return false;
    const cache = this.cache(key), origin = cache.selected, selection = ++this.selection;
    const record_hit = target.record_id !== undefined || target.sequence !== undefined;
    let selected = origin, opened = false, detail_started = false;
    await this.read_job(key, 'search', async () => {
      try {
        require_contract(typeof target.chat_id === 'string' && Boolean(target.chat_id) && safe_count(target.generation, 1) && safe_count(target.version, 1)
          && (!record_hit || (typeof target.record_id === 'string' && Boolean(target.record_id) && safe_count(target.sequence!, 1))));
        if (!this.current(key, selected, selection)) return;
        const result = await this.rpc(key, { operation: 'status', chat_id: target.chat_id });
        if (!this.current(key, selected, selection)) return;
        require_contract(result.operation === 'status' && result.session?.chat_id === target.chat_id);
        const session = result.session;
        if (session.content_purged && cache.selected === target.chat_id) { this.apply_session(key, session); this.clear_detail(cache); }
        require_contract(session.generation === target.generation && session.version === target.version && !session.content_purged
          && (!record_hit || target.sequence! <= session.last_record_sequence));
        const pending_id = this.pending?.project_key === key && 'chat_id' in this.pending.request ? this.pending.request.chat_id : null;
        const evict: string[] = [];
        if (!cache.sessions.has(target.chat_id)) {
          for (const [id, row] of cache.sessions) {
            if (cache.sessions.size - evict.length < CHAT_LIMITS.sessions) break;
            if (id !== origin && id !== pending_id && !row.active_request_id && !cache.drafts.get(id) && !cache.attachments.get(id)?.length) evict.push(id);
          }
          require_contract(cache.sessions.size - evict.length < CHAT_LIMITS.sessions);
        }
        require_contract(this.apply_session(key, session));
        this.clear_search_pin(cache);
        for (const id of evict) { cache.sessions.delete(id); this.titles.delete(id); }
        selected = target.chat_id; cache.selected = selected; cache.search_pinned = selected;
        cache.listed = [selected, ...cache.listed.filter((id) => id !== selected && !evict.includes(id))].slice(0, CHAT_LIMITS.sessions);
        this.clear_detail(cache); detail_started = true;
        if (record_hit) cache.cursor = { after_sequence: target.sequence! - 1, byte_offset: 0 };
        cache.error = ''; cache.notice = record_hit ? 'Showing history from the selected search result; earlier records are not loaded.' : '';
        this.publish();
        if (session.has_title) await this.read_title(key, selected, selection, selected);
        if (!this.current(key, selected, selection, target.generation)) return;
        await this.read_turns(key, selected, selection, target.generation, false);
        if (!this.current(key, selected, selection, target.generation)) return;
        await this.read_history(key, selected, selection, target.generation);
        if (!this.current(key, selected, selection, target.generation)) return;
        if (record_hit) require_contract(cache.records.some((row) => row.record.record_id === target.record_id && row.record.sequence === target.sequence && row.error === null));
        const verified = await this.rpc(key, { operation: 'status', chat_id: selected });
        if (!this.current(key, selected, selection, target.generation)) return;
        require_contract(verified.operation === 'status' && verified.session?.chat_id === selected);
        require_contract(this.apply_session(key, verified.session));
        require_contract(verified.session.generation === target.generation && verified.session.version === target.version && !verified.session.content_purged);
        opened = true;
      } catch {
        if (this.current(key, selected, selection)) {
          if (detail_started) this.clear_detail(cache);
          cache.error = 'This search result could not be opened. Refresh the search and try again.'; cache.notice = ''; this.publish();
        }
      }
    });
    return opened && this.current(key, target.chat_id, selection, target.generation);
  }
  refresh(): Promise<void> {
    const key = this.project_key; if (!key) return Promise.resolve();
    return this.read_job(key, 'list', async () => { await this.read_sessions(key, false, true); if (this.project_key === key && this.cache(key).selected) await this.refresh_selected(); });
  }
  load_more_sessions(): Promise<void> { const key = this.project_key; return key && this.cache(key).more ? this.read_job(key, 'list', () => this.read_sessions(key, true, false)) : Promise.resolve(); }
  set_archived(archived: boolean): Promise<void> { if (!this.project_key) return Promise.resolve(); this.cache(this.project_key).archived = archived; return this.refresh(); }
  private async read_sessions(key: string, append: boolean, titles: boolean): Promise<void> {
    const cache = this.cache(key), selection = this.selection, selected = cache.selected;
    const after = append ? cache.after : null;
    const result = await this.rpc(key, { operation: 'list', after, limit: CHAT_LIMITS.page, archived: cache.archived });
    if (!this.current(key, selected, selection)) return;
    require_contract(result.operation === 'list');
    require_contract(result.sessions.length <= CHAT_LIMITS.page && (!result.more || Boolean(result.next_after)) && (!after || result.next_after !== after));
    const ids = new Set(append ? cache.listed : []);
    for (const row of result.sessions) { this.apply_session(key, row); ids.add(row.chat_id); }
    cache.listed = [...ids].slice(0, CHAT_LIMITS.sessions); cache.after = result.next_after; cache.more = result.more && cache.listed.length < CHAT_LIMITS.sessions; cache.available = true; cache.error = ''; this.publish();
    for (const row of result.sessions) {
      if (!this.current(key, selected, selection)) return;
      if (row.has_title && !row.content_purged && (titles || !this.titles.has(row.chat_id))) await this.read_title(key, row.chat_id, selection, selected);
    }
  }
  private async read_title(key: string, chat_id: string, selection: number, selected: string | null): Promise<void> {
    const version = this.cache(key).sessions.get(chat_id)?.version;
    const result = await this.rpc(key, { operation: 'title', chat_id });
    if (!this.current(key, selected, selection) || this.cache(key).sessions.get(chat_id)?.version !== version) return;
    require_contract(result.operation === 'title' && result.chat_id === chat_id && (result.title === null || chat_bytes(result.title) <= CHAT_LIMITS.title_bytes));
    if (result.title) this.titles.set(chat_id, result.title); else this.titles.delete(chat_id); this.publish();
  }

  private refresh_selected(poll = false): Promise<void> {
    const key = this.project_key; if (!key) return Promise.resolve();
    const cache = this.cache(key), chat_id = cache.selected, selection = this.selection;
    if (!chat_id) return Promise.resolve();
    return this.read_job(key, 'selected', async () => {
      const previous = cache.sessions.get(chat_id);
      const result = await this.rpc(key, { operation: 'status', chat_id });
      if (!this.current(key, chat_id, selection)) return;
      if (result.operation === 'status' && result.session === null) { this.clear_progress(cache); this.publish(); return; }
      require_contract(result.operation === 'status' && result.session?.chat_id === chat_id);
      this.apply_session(key, result.session);
      const session = cache.sessions.get(chat_id)!;
      const generation = session.generation;
      cache.error = ''; this.publish();
      if (!poll || cache.turns.size === 0) await this.read_turns(key, chat_id, selection, generation, false);
      if (!this.current(key, chat_id, selection, generation)) return;
      const request_id = session.active_request_id ?? previous?.active_request_id;
      if (request_id) {
        const turn_generation = session.active_request_id ? generation : previous?.generation ?? generation;
        const submission = await this.rpc(key, { operation: 'submission', chat_id, generation: turn_generation, request_id });
        if (!this.current(key, chat_id, selection, generation)) return;
        require_contract(submission.operation === 'submission' && submission.session.chat_id === chat_id);
        if (submission.turn) { require_contract(submission.turn.request_id === request_id && submission.turn.generation === turn_generation); this.apply_turn(cache, submission.turn, chat_id); }
        this.apply_session(key, submission.session); this.publish();
      }
      if (!this.current(key, chat_id, selection, generation)) return;
      const progress_request = cache.sessions.get(chat_id)?.active_request_id ?? cache.command_progress?.request_id;
      if (!session.content_purged && progress_request) await this.read_progress(key, chat_id, selection, generation, progress_request);
      if (!this.current(key, chat_id, selection, generation)) return;
      if (!session.content_purged && (!cache.history_loaded || (!cache.content_more && session.last_record_sequence > cache.cursor.after_sequence))) await this.read_history(key, chat_id, selection, generation);
      if (!this.current(key, chat_id, selection, generation)) return;
      const current = cache.sessions.get(chat_id)!;
      if (current.state === 'busy' && current.active_request_id) await this.read_live(key, chat_id, selection, generation, current.active_request_id);
    });
  }
  poll(): Promise<void> { return this.refresh_selected(true); }
  load_more_turns(): Promise<void> {
    const key = this.project_key; if (!key) return Promise.resolve();
    const cache = this.cache(key), chat_id = cache.selected, selection = this.selection, generation = cache.sessions.get(chat_id ?? '')?.generation;
    return chat_id && generation && cache.turns_more ? this.read_job(key, 'turns', () => this.read_turns(key, chat_id, selection, generation, true)) : Promise.resolve();
  }
  private async read_turns(key: string, chat_id: string, selection: number, generation: number, append: boolean): Promise<void> {
    const cache = this.cache(key), after_sequence = append ? cache.turn_after : 0;
    const result = await this.rpc(key, { operation: 'turns', chat_id, after_sequence, limit: CHAT_LIMITS.page });
    if (!this.current(key, chat_id, selection, generation)) return;
    require_contract(result.operation === 'turns' && result.chat_id === chat_id && result.turns.length <= CHAT_LIMITS.page && safe_count(result.next_sequence)
      && (!result.more || result.next_sequence > after_sequence));
    for (const turn of result.turns) this.apply_turn(cache, turn, chat_id);
    cache.turn_after = result.next_sequence; cache.turns_more = result.more && cache.turns.size < CHAT_LIMITS.records; this.publish();
  }
  load_more_history(): Promise<void> {
    const key = this.project_key; if (!key) return Promise.resolve();
    const cache = this.cache(key), chat_id = cache.selected, selection = this.selection, generation = cache.sessions.get(chat_id ?? '')?.generation;
    return chat_id && generation && !cache.history_capped ? this.read_job(key, 'history', () => this.read_history(key, chat_id, selection, generation)) : Promise.resolve();
  }
  private async read_history(key: string, chat_id: string, selection: number, generation: number): Promise<void> {
    const cache = this.cache(key);
    for (let index = 0; index < CHAT_LIMITS.chunks_per_load; index++) {
      if (!this.current(key, chat_id, selection, generation) || cache.history_capped) return;
      const cursor = { ...cache.cursor };
      const result = await this.rpc(key, { operation: 'content', chat_id, cursor });
      if (!this.current(key, chat_id, selection, generation)) return;
      require_contract(result.operation === 'content' && result.page.chat_id === chat_id);
      const page = result.page;
      if (page.content_error === 'chat_content_purged') this.clear_progress(cache);
      require_contract(safe_count(page.last_record_sequence) && page.last_record_sequence >= cursor.after_sequence && safe_count(page.next_cursor.after_sequence) && safe_count(page.next_cursor.byte_offset));
      if (!page.record) {
        require_contract(page.text === null && page.complete && !page.more && page.next_cursor.after_sequence === cursor.after_sequence && page.next_cursor.byte_offset === cursor.byte_offset);
        cache.content_more = false; cache.history_loaded = true; this.publish(); return;
      }
      const record = page.record, chunk_bytes = page.text === null ? 0 : chat_bytes(page.text);
      require_contract(record.chat_id === chat_id && safe_count(record.sequence, 1) && record.sequence > cursor.after_sequence && safe_count(record.original_byte_count)
        && (record.retained_byte_count === null || (safe_count(record.retained_byte_count) && record.retained_byte_count <= CHAT_LIMITS.record_bytes)) && chunk_bytes <= CHAT_LIMITS.chunk_bytes);
      const existing = cache.records.at(-1);
      const continuing = cursor.byte_offset > 0;
      require_contract(!continuing || (existing?.record.record_id === record.record_id && existing.record.sequence === record.sequence && chat_bytes(existing.text) === cursor.byte_offset && !existing.complete));
      if (page.text === null) require_contract(page.complete && page.content_error !== null);
      else require_contract(record.content_available && page.content_error === null && record.retained_byte_count !== null && cursor.byte_offset + chunk_bytes <= record.retained_byte_count);
      if (page.complete) require_contract(page.next_cursor.after_sequence === record.sequence && page.next_cursor.byte_offset === 0 && (page.text === null || cursor.byte_offset + chunk_bytes === record.retained_byte_count));
      else require_contract(page.more && chunk_bytes > 0 && page.next_cursor.after_sequence === cursor.after_sequence && page.next_cursor.byte_offset === cursor.byte_offset + chunk_bytes);
      const loaded_bytes = cache.records.reduce((total, item) => total + chat_bytes(item.text), 0);
      if (loaded_bytes + chunk_bytes > CHAT_LIMITS.display_bytes || (!continuing && cache.records.length >= CHAT_LIMITS.records)) { cache.history_capped = true; cache.content_more = true; this.publish(); return; }
      const view: ChatRecordView = { record, text: (continuing ? existing!.text : '') + (page.text ?? ''), complete: page.complete, error: page.content_error };
      cache.records = continuing ? [...cache.records.slice(0, -1), view] : [...cache.records, view];
      if (record.kind === 'assistant' && view.complete && view.error === null && cache.live?.turn_id === record.turn_id) cache.live = null;
      cache.cursor = page.next_cursor; cache.content_more = page.more; cache.history_loaded = true; this.publish();
      if (!page.more) return;
    }
  }
  /** Optional private reads never interrupt canonical history or assistant reads. */
  private async read_progress(key: string, chat_id: string, selection: number, generation: number, request_id: string): Promise<void> {
    const cache = this.cache(key);
    if (cache.progress_unavailable) return;
    try {
      const bound = cache.turns.get(request_id);
      if (!bound || bound.generation !== generation) return;
      const result = await this.rpc(key, { operation: 'command_progress', chat_id, generation, request_id });
      if (!this.current(key, chat_id, selection, generation)) return;
      const session = cache.sessions.get(chat_id)!;
      if (session.content_purged || (session.active_request_id !== null && session.active_request_id !== request_id)) return;
      require_contract(result.operation === 'command_progress' && result.chat_id === chat_id && result.generation === generation && result.request_id === request_id);
      if (result.snapshot !== null) cache.command_progress = replace_command_progress(cache.command_progress, result.snapshot, bound.turn_id, request_id, generation);
      cache.progress_error = ''; this.publish();
    } catch (error) {
      if (!this.current(key, chat_id, selection, generation)) return;
      const code = chat_error(error);
      if (code === 'chat_content_purged') this.clear_progress(cache);
      else {
        cache.progress_unavailable = code === 'control_unsupported_version';
        cache.progress_error = cache.progress_unavailable ? 'Command observations are unavailable with this daemon version.' : 'Command observations could not be refreshed. The last observation is retained.';
      }
      this.publish();
    }
  }
  private async read_live(key: string, chat_id: string, selection: number, generation: number, request_id: string): Promise<void> {
    const cache = this.cache(key);
    for (let index = 0; index < CHAT_LIMITS.chunks_per_load; index++) {
      const previous = cache.live?.request_id === request_id && cache.live.generation === generation ? cache.live : null;
      const revision = previous?.revision ?? null, byte_offset = previous?.next_byte_offset ?? 0;
      const result = await this.rpc(key, { operation: 'live', chat_id, generation, request_id, revision, byte_offset });
      if (!this.current(key, chat_id, selection, generation) || cache.sessions.get(chat_id)?.active_request_id !== request_id) return;
      require_contract(result.operation === 'live' && result.chat_id === chat_id && result.generation === generation && result.request_id === request_id);
      const chunk = result.snapshot;
      if (!chunk) { cache.live = null; this.publish(); return; }
      require_contract(safe_count(chunk.revision, 1) && safe_count(chunk.byte_offset) && safe_count(chunk.next_byte_offset)
        && safe_count(chunk.retained_byte_count) && chunk.retained_byte_count <= CHAT_LIMITS.record_bytes && safe_count(chunk.original_byte_count)
        && chat_bytes(chunk.text) <= CHAT_LIMITS.chunk_bytes && chunk.next_byte_offset === chunk.byte_offset + chat_bytes(chunk.text)
        && chunk.next_byte_offset <= chunk.retained_byte_count && chunk.complete === (chunk.next_byte_offset === chunk.retained_byte_count) && chunk.more === !chunk.complete);
      const turn = cache.turns.get(request_id); if (turn) require_contract(chunk.turn_id === turn.turn_id);
      if (previous) require_contract(chunk.revision >= previous.revision && (chunk.revision === previous.revision ? !chunk.reset && chunk.byte_offset === byte_offset : chunk.reset && chunk.byte_offset === 0));
      else require_contract(chunk.byte_offset === 0);
      // A stale Busy observation can outlive the durable assistant append.
      if (cache.records.some((row) => row.record.turn_id === chunk.turn_id && row.record.kind === 'assistant' && row.complete && row.error === null)) {
        cache.live = null; this.publish(); return;
      }
      cache.live = { ...chunk, request_id, generation, text: (!previous || chunk.reset ? '' : previous.text) + chunk.text };
      require_contract(chat_bytes(cache.live.text) === chunk.next_byte_offset);
      this.publish(); if (!chunk.more) return;
    }
  }

  /** Records the explicit persistence + inference consent through the daemon. */
  confirm(): Promise<boolean> {
    const key = this.project_key;
    if (!key) return Promise.resolve(false);
    if (this.cache(key).confirmed) return Promise.resolve(true);
    return this.enqueue(async () => {
      if (this.disposed || this.project_key !== key) return false;
      try { await this.backend.chat_confirm(key); }
      catch (error) { this.cache(key).error = 'Chat consent could not be recorded (' + chat_error(error) + ').'; this.publish(); return false; }
      if (this.disposed || this.project_key !== key) return false;
      const cache = this.cache(key); cache.confirmed = true; cache.error = ''; this.publish();
      return true;
    });
  }

  /** One serialized open+send chain. The immutable binding keeps stable
   * operation/request IDs so an uncertain write is only ever replayed by an
   * explicit same-ID retry, never automatically. */
  send(): Promise<boolean> {
    const key = this.project_key;
    if (!key || this.picking_files) return Promise.resolve(false);
    const cache = this.cache(key);
    if (this.pending?.state === 'sending') return this.action_job ?? Promise.resolve(false);
    if (this.pending?.state === 'uncertain') { cache.error = 'The previous chat action is not confirmed yet. Reconcile it or retry the same action before sending again.'; this.publish(); return Promise.resolve(false); }
    const origin = cache.selected, session = origin ? cache.sessions.get(origin) ?? null : null;
    const draft = cache.drafts.get(origin ?? '') ?? '';
    const files = structuredClone(cache.attachments.get(origin ?? '') ?? []);
    const prompt = format_attachment_prompt(draft, files);
    const submitted = { key: origin ?? '', text: draft, files };
    if (!cache.confirmed) { cache.error = 'Accept persistent chat consent for this project before sending.'; this.publish(); return Promise.resolve(false); }
    const validation = chat_prompt_error(prompt);
    if (validation) { cache.error = validation; this.publish(); return Promise.resolve(false); }
    if (origin && (!session || session.active_request_id || !['opening', 'ready'].includes(session.state))) { cache.error = 'This chat cannot accept a new turn in its current state.'; this.publish(); return Promise.resolve(false); }
    const binding = { operation_id: this.new_id(), request_id: this.new_id() };
    cache.binding = binding;
    const open_request = { operation: 'open', operation_id: binding.operation_id, harness: cache.harness, title: null, consent_private_storage: true } as const;
    this.pending = origin && session
      ? { project_key: key, state: 'sending', code: '', prompt, draft: submitted, turn_request_id: binding.request_id, request: { operation: 'send', chat_id: origin, generation: session.generation, request_id: binding.request_id, prompt } }
      : { project_key: key, state: 'sending', code: '', prompt, draft: submitted, turn_request_id: binding.request_id, request: open_request };
    this.publish();
    const job = this.execute_send(key, binding, prompt, origin, origin ? null : open_request, session?.generation, true).finally(() => { if (this.action_job === job) this.action_job = null; this.publish(); });
    this.action_job = job;
    return job;
  }
  /** Open (when needed) → bounded read-only Ready wait → Send. Real Open
   * returns `opening` while the vendor initializes; sending into a not-ready
   * chat would bounce as a service error and strand an uncertain turn, so the
   * exact chat/generation is watched read-only first and nothing is sent when
   * readiness fails. */
  private async execute_send(key: string, binding: { operation_id: string; request_id: string }, prompt: string, origin: string | null, open_request: Extract<ChatRequest, { operation: 'open' }> | null, generation?: number, wait_ready = true): Promise<boolean> {
    const cache = this.cache(key);
    const selection = this.selection;
    let chat_id = origin;
    try {
      if (!chat_id) {
        const opened = await this.rpc(key, open_request!);
        if (this.disposed) throw { code: 'chat_view_disposed' };
        require_contract(opened.operation === 'session');
        this.apply_session(key, opened.session);
        chat_id = opened.session.chat_id; generation = opened.session.generation;
        if (this.current(key, null, selection)) {
          this.selection++; cache.selected = chat_id; this.clear_detail(cache);
          this.carry_draft(cache, chat_id);
        }
        this.pending = { project_key: key, state: 'sending', code: '', prompt, draft: this.pending?.draft, turn_request_id: binding.request_id, request: { operation: 'send', chat_id, generation, request_id: binding.request_id, prompt } };
        this.publish();
      }
      if (wait_ready && !await this.await_ready(key, chat_id!, generation!)) {
        if (this.pending?.turn_request_id === binding.request_id && this.pending.state === 'sending') this.pending = null;
        this.publish();
        return false;
      }
      const sent = await this.rpc(key, { operation: 'send', chat_id, generation: generation!, request_id: binding.request_id, prompt });
      if (this.disposed) throw { code: 'chat_view_disposed' };
      require_contract(sent.operation === 'turn' && sent.turn.chat_id === chat_id && sent.turn.request_id === binding.request_id);
      this.apply_session(key, sent.session); this.apply_turn(cache, sent.turn, chat_id);
      this.clear_submitted_draft(cache, this.pending?.draft);
      this.pending = null; cache.error = ''; this.publish();
      if (this.current(key, chat_id, this.selection)) void this.refresh_selected();
      return true;
    } catch (error) { return this.fail_pending(cache, error); }
  }
  /** Elapsed-time-bounded read-only wait for `ready` on the exact chat and
   * generation. Stops on ready, on any non-opening state (busy, suspended,
   * recovery, closed, archived), on a generation change, or on the deadline —
   * and then sends nothing, keeping the draft for an explicit retry. */
  private async await_ready(key: string, chat_id: string, generation: number): Promise<boolean> {
    const cache = this.cache(key);
    const deadline = this.timer.now() + CHAT_READY_TIMEOUT_MS;
    for (;;) {
      if (this.disposed) return false;
      const session = cache.sessions.get(chat_id);
      if (!session || session.generation !== generation) { cache.error = 'The chat changed while initializing; reload it before sending.'; this.publish(); return false; }
      if (session.state === 'ready') return true;
      if (session.state === 'recovery_required') { cache.error = 'The chat needs recovery before it can accept a turn.'; this.publish(); return false; }
      if (session.state !== 'opening') { cache.error = 'The chat is ' + chat_phase_label(session.state).toLowerCase() + ' and cannot accept a turn right now.'; this.publish(); return false; }
      if (this.timer.now() >= deadline) { cache.error = 'The chat is still initializing; no prompt was sent. Try Send again shortly.'; this.publish(); return false; }
      await this.timer.sleep(CHAT_READY_INTERVAL_MS);
      if (this.disposed) return false;
      const result = await this.rpc(key, { operation: 'status', chat_id });
      if (this.disposed) return false;
      if (result.operation === 'status' && result.session === null) { this.clear_progress(cache); this.publish(); }
      require_contract(result.operation === 'status' && result.session?.chat_id === chat_id);
      this.apply_session(key, result.session);
    }
  }
  private fail_pending(cache: ProjectChat, error: unknown): boolean {
    const code = chat_error(error);
    const keeps_draft = this.pending?.turn_request_id != null;
    if (CHAT_REJECTIONS.has(code)) { this.pending = null; cache.error = 'Chat action rejected (' + code + ').' + (keeps_draft ? ' Your draft is preserved.' : ''); }
    else if (this.pending) { this.pending = { ...this.pending, state: 'uncertain', code }; cache.error = 'Chat acknowledgement is uncertain (' + code + ').' + (keeps_draft ? ' Your draft is preserved.' : '') + ' Reconcile read-only or retry the exact same action and IDs.'; }
    else cache.error = 'Chat action failed (' + code + ').';
    this.publish(); return false;
  }

  /** Read-only reconciliation of the one uncertain write by its exact IDs.
   * A null observation proves nothing about a queued request, so it keeps the
   * binding uncertain; only an explicit same-ID retry can resend. */
  check_pending(): Promise<boolean> {
    const pending = this.pending;
    if (!pending || pending.state !== 'uncertain' || this.action_job) return Promise.resolve(false);
    const job = this.perform_check(pending.project_key, pending.request).finally(() => { if (this.action_job === job) this.action_job = null; this.publish(); });
    this.action_job = job;
    return job;
  }
  private async perform_check(key: string, request: ChatMutation): Promise<boolean> {
    const cache = this.cache(key);
    try {
      if (request.operation === 'open') {
        const looked = await this.rpc(key, { operation: 'lookup_open', operation_id: request.operation_id });
        if (this.disposed) throw { code: 'chat_view_disposed' };
        require_contract(looked.operation === 'status');
        if (!looked.session) { cache.notice = 'Chat creation is still unconfirmed. Nothing was re-sent automatically.'; this.publish(); return false; }
        this.apply_session(key, looked.session);
        if (this.project_key === key && !cache.selected) {
          this.selection++; cache.selected = looked.session.chat_id; this.clear_detail(cache);
          this.carry_draft(cache, looked.session.chat_id);
        }
        this.pending = null; cache.error = ''; cache.notice = 'Chat created. Your draft is retained; nothing was sent automatically.';
        this.publish();
        if (this.project_key === key && cache.selected === looked.session.chat_id) void this.refresh_selected();
        return true;
      }
      if (request.operation !== 'send') {
        // Lifecycle mutations reconcile read-only through chat status; a
        // rename is not observable content-free and needs a same-ID retry.
        if (request.operation !== 'close' && request.operation !== 'archive' && request.operation !== 'resume' && request.operation !== 'purge') {
          cache.notice = 'This action cannot be confirmed read-only. Retry the same action to settle it.';
          this.publish(); return false;
        }
        const observed = await this.rpc(key, { operation: 'status', chat_id: request.chat_id });
        if (this.disposed) throw { code: 'chat_view_disposed' };
        require_contract(observed.operation === 'status' && observed.session?.chat_id === request.chat_id);
        const session = observed.session!;
        if (session.generation !== request.generation) { cache.notice = 'The chat changed generation; retry the same action to settle it.'; this.publish(); return false; }
        this.apply_session(key, session);
        if (chat_mutation_observed(request.operation, session)) {
          this.pending = null; cache.error = ''; cache.notice = 'The chat action is confirmed by the observed chat state.';
          this.publish();
          if (this.project_key === key && cache.selected === request.chat_id) void this.refresh_selected();
          return true;
        }
        cache.notice = 'Read-only state does not confirm this action yet. Retry the same action to settle it.';
        this.publish(); return false;
      }
      const submission = await this.rpc(key, { operation: 'submission', chat_id: request.chat_id, generation: request.generation, request_id: request.request_id });
      if (this.disposed) throw { code: 'chat_view_disposed' };
      require_contract(submission.operation === 'submission' && submission.session.chat_id === request.chat_id);
      if (!submission.turn) { cache.notice = 'The sent turn is still unconfirmed. Nothing was re-sent automatically.'; this.publish(); return false; }
      require_contract(submission.turn.request_id === request.request_id && submission.turn.chat_id === request.chat_id);
      this.apply_session(key, submission.session); this.apply_turn(cache, submission.turn, request.chat_id);
      this.clear_submitted_draft(cache, this.pending?.draft);
      this.pending = null; cache.error = '';
      this.publish();
      if (this.project_key === key && cache.selected === request.chat_id) void this.refresh_selected();
      return true;
    } catch (error) { return this.fail_pending(cache, error); }
  }

  /** Explicit user retry of the one uncertain write: the exact same payload
   * and IDs are replayed (the daemon dedupes by operation/request id), so a
   * pre-admission failure with repeated null lookups can recover without
   * changing keys. Never triggered automatically. */
  retry_pending(): Promise<boolean> {
    const pending = this.pending;
    if (!pending || pending.state !== 'uncertain' || this.action_job) return Promise.resolve(false);
    const job = this.perform_retry(pending).finally(() => { if (this.action_job === job) this.action_job = null; this.publish(); });
    this.action_job = job;
    return job;
  }
  private async perform_retry(pending: PendingChat): Promise<boolean> {
    const key = pending.project_key, request = pending.request;
    this.pending = { ...pending, state: 'sending', code: '' };
    this.publish();
    if (request.operation === 'open') return this.execute_send(key, { operation_id: request.operation_id, request_id: pending.turn_request_id ?? request.operation_id }, pending.prompt, null, request, undefined, true);
    if (request.operation === 'send') return this.execute_send(key, { operation_id: '', request_id: request.request_id }, pending.prompt, request.chat_id, null, request.generation, false);
    if (request.operation === 'cancel') return false;
    const cache = this.cache(key);
    try {
      const result = await this.rpc(key, request);
      if (this.disposed) throw { code: 'chat_view_disposed' };
      return this.settle_manage(key, request, result);
    } catch (error) { return this.fail_pending(cache, error); }
  }

  /** Ask the daemon to stop the active turn; the canonical outcome stays daemon-owned. */
  async cancel(): Promise<void> {
    const key = this.project_key;
    if (!key) return;
    const cache = this.cache(key), chat_id = cache.selected, session = chat_id ? cache.sessions.get(chat_id) : null;
    if (!chat_id || !session?.active_request_id) return;
    try {
      const result = await this.rpc(key, { operation: 'cancel', chat_id, generation: session.generation, request_id: session.active_request_id });
      require_contract(result.operation === 'turn' && result.turn.chat_id === chat_id && result.turn.request_id === session.active_request_id);
      this.apply_turn(cache, result.turn, chat_id); this.apply_session(key, result.session); cache.error = '';
    } catch (error) { cache.error = 'Cancellation is unconfirmed (' + chat_error(error) + '). The turn may still be running.'; }
    this.publish();
  }

  /** Generation-bound lifecycle action on one chat (rename/close/archive/
   * resume/purge). The mutation occupies the pending slot with its exact
   * request, so a lost acknowledgement is retried with the same payload and
   * operation id or reconciled read-only — never re-issued under a new id. */
  manage(action: ChatManagement, title?: string, chat_id?: string, project_key?: string): Promise<boolean> {
    const key = project_key ?? this.project_key;
    if (!key) return Promise.resolve(false);
    const cache = this.cache(key);
    const target = chat_id ?? cache.selected;
    const session = target ? cache.sessions.get(target) ?? null : null;
    if (!target || !session) return Promise.resolve(false);
    if (this.action_job || this.pending) { cache.notice = 'Wait for the current chat action to settle first.'; this.publish(); return Promise.resolve(false); }
    if (!chat_can_manage(session, action)) { cache.error = 'This chat cannot be ' + (action === 'resume' ? 'resumed' : action + 'd') + ' in its current state.'; this.publish(); return Promise.resolve(false); }
    let request: Extract<ChatMutation, { operation: 'rename' | 'close' | 'archive' | 'resume' | 'purge' }>;
    if (action === 'rename') {
      const name = (title ?? '').trim();
      if (!name || chat_bytes(name) > CHAT_LIMITS.title_bytes) { cache.error = 'Provide a chat title within 512 bytes.'; this.publish(); return Promise.resolve(false); }
      request = { operation: 'rename', operation_id: this.new_id(), chat_id: target, generation: session.generation, title: name };
    } else request = { operation: action, operation_id: this.new_id(), chat_id: target, generation: session.generation };
    this.pending = { project_key: key, request, state: 'sending', code: '', prompt: '', turn_request_id: null };
    this.publish();
    const job = this.perform_manage(key, request).finally(() => { if (this.action_job === job) this.action_job = null; this.publish(); });
    this.action_job = job;
    return job;
  }
  private async perform_manage(key: string, request: Extract<ChatMutation, { operation: 'rename' | 'close' | 'archive' | 'resume' | 'purge' }>): Promise<boolean> {
    try {
      const result = await this.rpc(key, request);
      if (this.disposed) throw { code: 'chat_view_disposed' };
      return this.settle_manage(key, request, result);
    } catch (error) { return this.fail_pending(this.cache(key), error); }
  }
  private settle_manage(key: string, request: Extract<ChatMutation, { operation: 'rename' | 'close' | 'archive' | 'resume' | 'purge' }>, result: ChatResponse): boolean {
    const cache = this.cache(key);
    require_contract(result.operation === 'session' && result.session.chat_id === request.chat_id);
    this.apply_session(key, result.session);
    if (request.operation === 'rename') this.titles.set(request.chat_id, request.title);
    if (this.project_key === key && cache.selected === request.chat_id && request.operation === 'archive' && !cache.archived) this.new_chat();
    if (this.pending?.request === request) this.pending = null;
    cache.error = ''; cache.notice = 'Chat ' + request.operation + ' recorded.'; this.publish();
    return true;
  }
}
