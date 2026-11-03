import { EXPORT_LIMITS, type ExportBackend, type ExportFormat, type ExportReceipt, type ExportRequest, type ExportSelection } from './chat_export_contract.ts';

const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
const UNCONFIRMED = 'The export result could not be confirmed. Check the selected destination before another export.';
export interface ExportView {
  format: ExportFormat; busy: boolean; can_export: boolean;
  status: 'idle' | 'saving' | 'saved' | 'cancelled' | 'error'; error: string; saved: ExportReceipt | null;
}
function safe_count(value: unknown, minimum = 0): value is number {
  return typeof value === 'number' && Number.isSafeInteger(value) && value >= minimum;
}
function valid_selection(value: ExportSelection): boolean {
  return typeof value.project_key === 'string' && value.project_key.length > 0
    && !/[\u0000-\u001f\u007f]/.test(value.project_key)
    && (value.kind === 'chat' || value.kind === 'workspace')
    && typeof value.chat_id === 'string' && UUID.test(value.chat_id)
    && safe_count(value.generation, 1) && safe_count(value.version, 1) && typeof value.enabled === 'boolean';
}
function same_selection(a: ExportSelection | null, b: ExportSelection | null): boolean {
  return a === b || !!a && !!b && a.project_key === b.project_key && a.kind === b.kind
    && a.chat_id === b.chat_id && a.generation === b.generation && a.version === b.version && a.enabled === b.enabled;
}
function checked_receipt(value: unknown, request: ExportRequest): ExportReceipt {
  if (!value || typeof value !== 'object') throw new Error('invalid_export_receipt');
  const receipt = value as Partial<ExportReceipt>;
  if ((receipt.status !== 'saved' && receipt.status !== 'cancelled')
    || receipt.kind !== request.kind || receipt.format !== request.format || receipt.chat_id !== request.chat_id
    || receipt.generation !== request.generation || receipt.version !== request.expected_version
    || !safe_count(receipt.record_count) || receipt.record_count > EXPORT_LIMITS.records
    || !safe_count(receipt.byte_count) || typeof receipt.partial !== 'boolean'
    || (receipt.status === 'saved' && receipt.byte_count === 0)
    || (receipt.status === 'cancelled' && (receipt.record_count !== 0 || receipt.byte_count !== 0 || receipt.partial))) {
    throw new Error('invalid_export_receipt');
  }
  return {
    status: receipt.status, kind: request.kind, format: request.format, chat_id: request.chat_id,
    generation: request.generation, version: request.expected_version, record_count: receipt.record_count,
    byte_count: receipt.byte_count, partial: receipt.partial,
  };
}

/** One explicit native save operation at a time, including across navigation.
 * No private content or destination is read, supplied, or retained by this controller. */
export class ExportState {
  private backend: ExportBackend;
  private changed: (view: ExportView) => void;
  private selection: ExportSelection | null = null;
  private format: ExportFormat = 'text';
  private pending = false;
  private epoch = 0;
  private disposed = false;
  private status: ExportView['status'] = 'idle';
  private error = '';
  private saved: ExportReceipt | null = null;

  constructor(backend: ExportBackend, changed: (view: ExportView) => void = () => {}) {
    this.backend = backend; this.changed = changed;
  }
  get view(): ExportView {
    return {
      format: this.format, busy: !this.disposed && this.pending,
      can_export: !this.disposed && !this.pending && this.selection?.enabled === true,
      status: this.status, error: this.error, saved: this.saved ? { ...this.saved } : null,
    };
  }
  private emit(): void { if (!this.disposed) this.changed(this.view); }
  select(selection: ExportSelection | null): void {
    if (this.disposed) return;
    const next = selection && valid_selection(selection) ? { ...selection } : null;
    if (same_selection(this.selection, next)) return;
    this.epoch++; this.selection = next;
    this.status = 'idle'; this.error = ''; this.saved = null; this.emit();
  }
  set_format(format: ExportFormat): void {
    if (this.disposed || (format !== 'text' && format !== 'json') || this.format === format) return;
    this.format = format; this.saved = null;
    if (this.status !== 'error') this.status = this.pending ? 'saving' : 'idle';
    this.emit();
  }
  async export(): Promise<boolean> {
    if (!this.view.can_export || !this.selection) return false;
    const target = { ...this.selection }, epoch = this.epoch;
    const request: ExportRequest = Object.freeze({ kind: target.kind, format: this.format, chat_id: target.chat_id,
      generation: target.generation, expected_version: target.version });
    this.pending = true; this.status = 'saving'; this.error = ''; this.saved = null; this.emit();
    try {
      const receipt = checked_receipt(await this.backend.chat_export_request(target.project_key, request), request);
      if (this.disposed || this.epoch !== epoch) return false;
      this.status = receipt.status;
      this.saved = receipt.status === 'saved' ? receipt : null;
      return receipt.status === 'saved';
    } catch {
      if (!this.disposed && this.epoch === epoch) { this.status = 'error'; this.error = UNCONFIRMED; }
      return false;
    } finally {
      this.pending = false;
      if (this.epoch !== epoch && this.status === 'saving') this.status = 'idle';
      if (!this.disposed && this.selection !== null) this.emit();
    }
  }
  dispose(): void {
    this.disposed = true; this.epoch++; this.selection = null;
    this.status = 'idle'; this.error = ''; this.saved = null;
  }
}
