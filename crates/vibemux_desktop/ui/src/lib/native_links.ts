import type { OpenLinkRequest } from './link_contract.ts';
import { command_code } from './native_state.ts';

const MAX_URL_BYTES = 2048;

/** Explicit link clicks only; disposal cannot recall an accepted OS handoff. */
export class NativeLinks {
  private disposed = false;
  private pending = false;
  private readonly perform: (request: Extract<OpenLinkRequest, { kind: 'https' }>) => Promise<unknown>;
  private readonly notify: (notice: string) => void;
  private readonly inert: boolean;
  constructor(perform: (request: Extract<OpenLinkRequest, { kind: 'https' }>) => Promise<unknown>,
    notify: (notice: string) => void = () => {}, inert = false) {
    this.perform = perform; this.notify = notify; this.inert = inert;
  }

  async open(value: string): Promise<boolean> {
    if (this.disposed || this.inert || this.pending) return false;
    if (!valid_https(value)) { this.notify('Open a complete HTTPS link without credentials or whitespace.'); return false; }
    this.pending = true;
    try {
      await Promise.resolve();
      if (this.disposed) return false;
      const reply = await this.perform({ kind: 'https', url: value });
      if (this.disposed) return false;
      if (reply === null || typeof reply !== 'object' || Array.isArray(reply) || Reflect.ownKeys(reply).length !== 2
        || !Object.hasOwn(reply, 'kind') || !Object.hasOwn(reply, 'outcome')
        || (reply as Record<string, unknown>).kind !== 'https' || (reply as Record<string, unknown>).outcome !== 'open_requested') {
        throw { code: 'link_response_invalid' };
      }
      this.notify('Link open requested.');
      return true;
    } catch (error: unknown) {
      if (!this.disposed) this.notify(`Link opening is unconfirmed (${command_code(error)}).`);
      return false;
    } finally { this.pending = false; }
  }

  dispose(): void { this.disposed = true; }
}

function valid_https(value: string): boolean {
  if (!value || new TextEncoder().encode(value).length > MAX_URL_BYTES || /[\s\u0000-\u001f\u007f-\u009f]/.test(value)) return false;
  try {
    const url = new URL(value);
    return url.protocol === 'https:' && Boolean(url.host) && !url.username && !url.password;
  } catch { return false; }
}
