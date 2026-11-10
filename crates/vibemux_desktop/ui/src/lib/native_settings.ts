import { command_error_message, invoke_native, type Backend } from './backend.ts';
import type { DesktopSettings, SettingsPatch } from './ui_contract.ts';

export interface PreferenceValues {
  theme: DesktopSettings['appearance'];
  font_size: DesktopSettings['text_size'];
  density: DesktopSettings['density'];
  reduce_motion: boolean;
  sidebar_collapsed: boolean;
}
export interface PreferenceState {
  values: PreferenceValues;
  saved_values: PreferenceValues | null;
  loaded: boolean;
  status: 'loading' | 'saving' | 'saved' | 'error';
  message: string;
}
const defaults: PreferenceValues = {
  theme: 'system', font_size: 'default', density: 'comfortable',
  reduce_motion: false, sidebar_collapsed: false,
};

function values_from(settings: DesktopSettings): PreferenceValues {
  const fields = ['schema_version', 'appearance', 'text_size', 'density', 'reduce_motion', 'sidebar_collapsed', 'start_page'];
  if (!settings || Object.keys(settings).length !== fields.length ||
      !fields.every((field) => Object.hasOwn(settings, field)) ||
      settings.schema_version !== 1 || !['light', 'dark', 'system'].includes(settings.appearance) ||
      !['small', 'default', 'large'].includes(settings.text_size) ||
      !['comfortable', 'compact'].includes(settings.density) ||
      !['chat', 'tasks', 'console'].includes(settings.start_page) ||
      typeof settings.reduce_motion !== 'boolean' || typeof settings.sidebar_collapsed !== 'boolean') {
    throw new Error('Unsupported desktop settings response.');
  }
  return {
    theme: settings.appearance, font_size: settings.text_size, density: settings.density,
    reduce_motion: settings.reduce_motion, sidebar_collapsed: settings.sidebar_collapsed,
  };
}

export function native_settings_patch(values: Partial<PreferenceValues>): SettingsPatch {
  const patch: SettingsPatch = {};
  if (values.theme !== undefined && ['light', 'dark', 'system'].includes(values.theme)) patch.appearance = values.theme;
  if (values.font_size !== undefined && ['small', 'default', 'large'].includes(values.font_size)) patch.text_size = values.font_size;
  if (values.density !== undefined && ['comfortable', 'compact'].includes(values.density)) patch.density = values.density;
  if (typeof values.reduce_motion === 'boolean') patch.reduce_motion = values.reduce_motion;
  if (typeof values.sidebar_collapsed === 'boolean') patch.sidebar_collapsed = values.sidebar_collapsed;
  return patch;
}

function patch_values(patch: SettingsPatch): Partial<PreferenceValues> {
  return {
    ...(patch.appearance !== undefined ? { theme: patch.appearance } : {}),
    ...(patch.text_size !== undefined ? { font_size: patch.text_size } : {}),
    ...(patch.density !== undefined ? { density: patch.density } : {}),
    ...(patch.reduce_motion !== undefined ? { reduce_motion: patch.reduce_motion } : {}),
    ...(patch.sidebar_collapsed !== undefined ? { sidebar_collapsed: patch.sidebar_collapsed } : {}),
  };
}

// At most one IPC write and one five-field pending patch. Rapid changes replace
// pending values, never enqueue per-keystroke promises. Errors require an edit
// or explicit retry; an optimistic preview is never reported as saved.
export function create_native_preferences(backend: Backend, delay = 250) {
  let state: PreferenceState = { values: { ...defaults }, saved_values: null, loaded: false, status: 'loading', message: 'Loading desktop settings…' };
  let pending: SettingsPatch = {};
  let timer: ReturnType<typeof setTimeout> | undefined;
  let loading: Promise<void> | null = null;
  let writing: Promise<void> | null = null;
  let disposed = false;
  const listeners = new Set<(state: PreferenceState) => void>();
  const publish = () => { if (!disposed) for (const listener of listeners) listener(structuredClone(state)); };
  const schedule = () => {
    clearTimeout(timer);
    if (!writing && !disposed) timer = setTimeout(() => { void flush(); }, delay);
  };
  const load = (): Promise<void> => {
    if (disposed || state.loaded) return Promise.resolve();
    if (loading) return loading;
    state = { ...state, status: 'loading', message: 'Loading desktop settings…' };
    publish();
    loading = Promise.resolve().then(async () => {
      try {
        const values = values_from(await invoke_native(backend, 'settings_get'));
        state = { values, saved_values: { ...values }, loaded: true, status: 'saved', message: 'Desktop settings saved on this device.' };
      } catch (error) {
        state = { ...state, status: 'error', message: command_error_message(error, 'Desktop settings could not be loaded. Changes are disabled; retry loading.') };
      } finally { loading = null; publish(); }
    });
    return loading;
  };
  const flush = (): Promise<void> => {
    clearTimeout(timer);
    if (writing) return writing;
    if (disposed || !state.loaded || !Object.keys(pending).length) return Promise.resolve();
    const sent = pending;
    pending = {};
    state = { ...state, status: 'saving', message: 'Saving desktop settings…' };
    publish();
    writing = Promise.resolve().then(async () => {
      let success = false;
      try {
        const settings = await invoke_native(backend, 'settings_set', { patch: sent });
        const values = values_from(settings);
        success = true;
        const more = Object.keys(pending).length > 0;
        state = {
          values: { ...values, ...patch_values(pending) }, saved_values: { ...values }, loaded: true,
          status: more ? 'saving' : 'saved',
          message: more ? 'Saving desktop settings…' : 'Desktop settings saved on this device.',
        };
      } catch (error) {
        pending = { ...sent, ...pending };
        state = { ...state, status: 'error', message: command_error_message(error, 'Desktop settings are not confirmed saved. This window shows an unsaved preview; retry saving.') };
      } finally {
        writing = null;
        publish();
        if (success && Object.keys(pending).length) schedule();
      }
    });
    return writing;
  };
  return {
    get state(): PreferenceState { return structuredClone(state); },
    subscribe(listener: (state: PreferenceState) => void) {
      listeners.add(listener);
      listener(structuredClone(state));
      return () => { listeners.delete(listener); };
    },
    load,
    update(values: Partial<PreferenceValues>) {
      if (disposed || !state.loaded) return false;
      const patch = native_settings_patch(values);
      if (!Object.keys(patch).length) return false;
      pending = { ...pending, ...patch };
      state = { ...state, values: { ...state.values, ...patch_values(patch) }, status: 'saving', message: 'Saving desktop settings…' };
      publish();
      schedule();
      return true;
    },
    flush,
    retry: () => state.loaded ? flush() : load(),
    dispose() {
      disposed = true;
      clearTimeout(timer);
      listeners.clear();
    },
  };
}
