import type { Backend } from './backend.ts';
import type { AskReply, AskStatus, AskTargets, Harness, ProjectRow, SubmissionReceipt } from './ui_contract.ts';
import { prompt_error, type Presentation } from './native_config.ts';

export interface PendingSubmission {
  project_key: string;
  harness: Harness;
  prompt: string;
  submission_key: string;
  state: 'sending' | 'uncertain' | 'rejected' | 'accepted';
  ask_id?: string;
}

const KNOWN_REJECTIONS = new Set(['ask_consent_required', 'harness_not_available', 'invalid_harness', 'invalid_identifier', 'invalid_arguments', 'prompt_too_large', 'prompt_empty', 'prompt_frame_too_large', 'ask_busy', 'ask_history_busy', 'daemon_job_busy', 'project_folder_missing', 'project_identity_changed', 'project_identity_unavailable', 'unknown_project', 'ask_submission_conflict', 'ask_dedup_capacity', 'desktop_shutting_down']);

export function command_code(error: unknown): string {
  if (error !== null && typeof error === 'object' && 'code' in error && typeof error.code === 'string' && /^[a-z][a-z0-9_]{0,79}$/.test(error.code)) return error.code;
  return 'native_command_failed';
}

export function harness_label(harness: Harness): string {
  return harness === 'claude' ? 'Claude Code' : 'Codex';
}

export function phase_label(status: AskStatus): string {
  if (status.terminal_observed && status.canonical) {
    return ({ completed: 'Completed', cancelled: 'Cancelled', failed: 'Failed', unverified: 'Unverified' } as Record<string, string>)[status.canonical.phase] ?? status.canonical.phase;
  }
  if (status.canonical?.phase === 'recovery_pending') return 'Recovery pending';
  if (status.phase === 'uncertain') return 'Outcome uncertain';
  if (status.cancel_requested || status.phase === 'cancelling') return 'Cancelling';
  if (status.phase === 'rejected') return 'Rejected';
  if (!status.canonical) return 'Awaiting daemon admission';
  return status.canonical.phase === 'running' ? 'Running' : 'Admitted';
}

/** Renderer state only. Rust retains ownership when this object is discarded. */
export class NativeState {
  projects: ProjectRow[] = [];
  selected_project: string | null = null;
  selected_ask: string | null = null;
  presentation: Presentation = 'claude';
  statuses = new Map<string, AskStatus>();
  replies = new Map<string, AskReply>();
  targets = new Map<string, AskTargets>();
  drafts = new Map<string, string>();
  submissions = new Map<string, PendingSubmission>();
  pending_keys = new Map<string, string>();
  error = '';
  connection: 'connecting' | 'ready' | 'unavailable' = 'connecting';
  private refresh_job: Promise<void> | null = null;
  private submission_jobs = new Map<string, Promise<SubmissionReceipt | null>>();
  private backend: Backend;
  private changed: () => void;
  private new_key: () => string;

  constructor(backend: Backend, changed = () => {}, new_key: () => string = () => crypto.randomUUID()) {
    this.backend = backend; this.changed = changed; this.new_key = new_key;
  }

  get project(): ProjectRow | undefined { return this.projects.find((row) => row.project_key === this.selected_project); }
  get draft(): string { return this.drafts.get(this.selected_project ?? '') ?? ''; }
  get pending(): PendingSubmission | undefined {
    const key = this.pending_keys.get(this.selected_project ?? '');
    return key ? this.submissions.get(key) : undefined;
  }
  set_draft(value: string): void { this.drafts.set(this.selected_project ?? '', value); this.changed(); }
  set_presentation(value: Presentation): void { this.presentation = value; this.changed(); }
  select_ask(ask_id: string): void { this.selected_ask = ask_id; this.changed(); }
  new_ask(): void { this.selected_ask = null; this.changed(); }

  apply_status(status: AskStatus): void {
    const minimum = Math.max(this.statuses.get(status.ask_id)?.revision ?? -1, this.replies.get(status.ask_id)?.revision ?? -1);
    if (status.revision < minimum) return;
    this.statuses.set(status.ask_id, status); this.changed();
  }
  apply_reply(reply: AskReply): void {
    const minimum = Math.max(this.replies.get(reply.ask_id)?.revision ?? -1, this.statuses.get(reply.ask_id)?.revision ?? -1);
    if (reply.revision < minimum) return;
    this.replies.set(reply.ask_id, reply); this.changed();
  }

  refresh(reconcile = false): Promise<void> {
    if (this.refresh_job) return reconcile ? this.refresh_job.then(() => this.refresh(true)) : this.refresh_job;
    this.refresh_job = this.refresh_all(reconcile).finally(() => { this.refresh_job = null; });
    return this.refresh_job;
  }

  private async refresh_all(reconcile: boolean): Promise<void> {
    try {
      this.projects = await this.backend.invoke('projects_list', {});
      this.connection = 'ready';
      if (!this.projects.some((row) => row.project_key === this.selected_project)) {
        this.selected_project = this.projects.find((row) => row.active)?.project_key ?? this.projects[0]?.project_key ?? null;
        this.selected_ask = this.project?.ask_ids.at(-1) ?? null;
      }
      const project = this.project;
      if (project) {
        if (!project.missing) {
          try { this.targets.set(project.project_key, await this.backend.invoke('ask_targets', { project_key: project.project_key })); }
          catch (error) { this.targets.set(project.project_key, { targets: [], setup_offered: false, code: command_code(error) }); }
        }
        await Promise.all(project.ask_ids.map((ask_id) => this.refresh_ask(ask_id, reconcile)));
      }
      const retained = new Set(this.projects.flatMap((row) => row.ask_ids));
      for (const ask_id of this.statuses.keys()) {
        if (!retained.has(ask_id)) { this.statuses.delete(ask_id); this.replies.delete(ask_id); }
      }
    } catch (error) {
      this.connection = 'unavailable'; this.error = `Native connection unavailable (${command_code(error)}). Refresh to reconnect; no prompt was replayed.`;
    }
    this.changed();
  }

  async refresh_ask(ask_id: string, reconcile = false): Promise<void> {
    try {
      this.apply_status(await this.backend.invoke('ask_status', { ask_id, reconcile }));
      this.apply_reply(await this.backend.invoke('ask_reply', { ask_id }));
    } catch (error) { this.error = `Could not refresh Ask (${command_code(error)}). Its last observed outcome is retained.`; this.changed(); }
  }

  async select_project(project_key: string): Promise<void> {
    try {
      await this.backend.invoke('project_select', { project_key });
      this.selected_project = project_key; this.selected_ask = this.project?.ask_ids.at(-1) ?? null; await this.refresh(true);
    } catch (error) { this.error = `Project selection failed (${command_code(error)}).`; this.changed(); }
  }

  send(harness: Harness, explicit_new = false): Promise<SubmissionReceipt | null> {
    const project_key = this.selected_project;
    const validation = prompt_error(this.draft);
    if (!project_key || validation) { this.error = validation ?? 'Select a registered project.'; this.changed(); return Promise.resolve(null); }
    const pending = this.pending;
    if (pending?.state === 'sending') return this.submission_jobs.get(pending.submission_key) ?? Promise.resolve(null);
    if (pending?.state === 'uncertain' && !explicit_new) {
      this.error = 'The previous submission is uncertain. Reconcile it using the same key, or explicitly start a new independent Ask.';
      this.changed(); return Promise.resolve(null);
    }
    const submission: PendingSubmission = { project_key, harness, prompt: this.draft, submission_key: this.new_key(), state: 'sending' };
    if (pending && (pending.state === 'accepted' || pending.state === 'rejected')) this.submissions.delete(pending.submission_key);
    this.submissions.set(submission.submission_key, submission);
    this.pending_keys.set(project_key, submission.submission_key);
    return this.submit(submission);
  }

  retry_submission(submission_key?: string): Promise<SubmissionReceipt | null> {
    const pending = submission_key ? this.submissions.get(submission_key) : this.pending;
    if (!pending) return Promise.resolve(null);
    return this.submission_jobs.get(pending.submission_key) ?? this.submit(pending);
  }

  private submit(submission: PendingSubmission): Promise<SubmissionReceipt | null> {
    submission.state = 'sending'; this.error = ''; this.changed();
    const { project_key, harness, prompt, submission_key } = submission;
    const job = this.backend.invoke('ask_submit', { project_key, harness, prompt, submission_key })
      .then((receipt) => {
        submission.state = 'accepted'; submission.ask_id = receipt.ask_id;
        if (!this.replies.has(receipt.ask_id)) this.replies.set(receipt.ask_id, { ask_id: receipt.ask_id, revision: 0, prompt, reply: '', truncated: false, delivery_complete: false });
        const project = this.projects.find((row) => row.project_key === project_key);
        if (project && !project.ask_ids.includes(receipt.ask_id)) project.ask_ids.push(receipt.ask_id);
        if (this.drafts.get(project_key) === prompt) this.drafts.set(project_key, '');
        if (this.selected_project === project_key) this.selected_ask = receipt.ask_id;
        if (this.pending_keys.get(project_key) !== submission_key) this.submissions.delete(submission_key);
        return receipt;
      })
      .catch((error: unknown) => {
        const code = command_code(error);
        submission.state = KNOWN_REJECTIONS.has(code) ? 'rejected' : 'uncertain';
        this.error = submission.state === 'rejected'
          ? `Submission rejected (${code}). Your draft is preserved.`
          : `Submission acknowledgement is uncertain (${code}). Your prompt is preserved. Reconcile the same submission before sending again.`;
        return null;
      })
      .finally(() => { this.submission_jobs.delete(submission_key); this.changed(); });
    this.submission_jobs.set(submission_key, job);
    return job;
  }

  async cancel(ask_id: string): Promise<void> {
    try {
      await this.backend.invoke('ask_cancel', { ask_id });
      const status = this.statuses.get(ask_id);
      if (status && !status.terminal_observed) this.statuses.set(ask_id, { ...status, cancel_requested: true, phase: 'cancelling' });
      await this.refresh_ask(ask_id);
    } catch (error) { this.error = `Cancellation is unconfirmed (${command_code(error)}). The daemon may still be running.`; }
    this.changed();
  }
}
