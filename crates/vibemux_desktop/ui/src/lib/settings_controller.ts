import type { Backend } from './backend.ts';
import { command_code } from './native_state.ts';
import type { PublicSettings, SettingsPatch } from './ui_contract.ts';

const PUBLIC_FIELDS = ['appearance', 'text_size', 'density', 'reduce_motion', 'sidebar_collapsed', 'start_page'] as const;
export type AboutInfo = { app_version: string; webview_version: string };

export function public_patch(value: SettingsPatch): SettingsPatch {
  return Object.fromEntries(PUBLIC_FIELDS.filter((key) => Object.hasOwn(value, key)).map((key) => [key, value[key]])) as SettingsPatch;
}

/** Only confirmed Rust projections become the rendered settings state. */
export class SettingsController {
  value: PublicSettings | null = null;
  about: AboutInfo | null = null;
  error = '';
  about_error = '';
  loading = false;
  pending = 0;
  private backend: Backend;
  private changed: () => void;
  private queue: Promise<void> = Promise.resolve();
  private load_job: Promise<void> | null = null;
  private value_revision = 0;

  constructor(backend: Backend, changed = () => {}) { this.backend = backend; this.changed = changed; }

  load(): Promise<void> {
    if (this.load_job) return this.load_job;
    this.loading = true; this.changed();
    const read_revision = this.value_revision;
    this.load_job = Promise.all([
      this.backend.invoke('settings_get', {}).then((value) => { if (read_revision === this.value_revision) { this.value = value; this.error = ''; } }).catch((error: unknown) => { this.error = 'Settings could not be loaded (' + command_code(error) + ').'; }),
      this.backend.invoke('about_info', {}).then((value) => { this.about = value; this.about_error = ''; }).catch((error: unknown) => { this.about_error = 'Version details unavailable (' + command_code(error) + ').'; }),
    ]).then(() => undefined).finally(() => { this.loading = false; this.load_job = null; this.changed(); });
    return this.load_job;
  }

  save(patch: SettingsPatch): Promise<boolean> { return this.enqueue(() => public_patch(patch)); }
  toggle_sidebar(): Promise<boolean> { return this.enqueue((value) => ({ sidebar_collapsed: !value.sidebar_collapsed })); }

  private enqueue(patch_for: (value: PublicSettings) => SettingsPatch): Promise<boolean> {
    this.pending++; this.changed();
    const operation = this.queue.then(async () => {
      if (!this.value) { this.error = 'Load settings before changing them.'; return false; }
      this.error = '';
      try {
        this.value = await this.backend.invoke('settings_set', { patch: public_patch(patch_for(this.value)) });
        this.value_revision++;
        return true;
      } catch (error) {
        this.error = 'Settings were not saved (' + command_code(error) + '). The last confirmed values are still applied.';
        return false;
      }
    }).finally(() => { this.pending--; this.changed(); });
    this.queue = operation.then(() => undefined);
    return operation;
  }
}
