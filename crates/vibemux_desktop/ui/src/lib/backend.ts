import type { CommandName, NativeCommands, UiEventName } from './ui_contract';

export function invoke_native<K extends keyof NativeCommands>(
  backend: Backend,
  command: K,
  ...args: NativeCommands[K]['args'] extends undefined ? [] : [NativeCommands[K]['args']]
): Promise<NativeCommands[K]['result']> {
  return backend.invoke<NativeCommands[K]['result']>(command, args[0]);
}

// IPC failures may contain platform details. Never render raw exceptions or
// arbitrary returned values (paths, environment contents, or credentials).
export function command_error_message(error: unknown, message: string): string {
  const code = error && typeof error === 'object' && 'code' in error ? error.code : null;
  return typeof code === 'string' && /^[a-z][a-z0-9_]{0,63}$/.test(code)
    ? `${message} (${code})`
    : message;
}

/// The only channel the UI has to the Rust side. The production backend
/// calls the fixed Tauri command list; the fixture backend serves demo
/// data and is loaded only in fixture mode.
export interface Backend {
  readonly fixture?: boolean;
  readonly fixture_label?: string;
  invoke<T>(command: CommandName, args?: Record<string, unknown>): Promise<T>;
  listen(event: UiEventName, handler: (payload: unknown) => void): Promise<() => void>;
}

/// Returns the backend for the current build. The fixture backend is
/// imported dynamically behind a build-time constant so that the fixture
/// data never reaches a production bundle.
export async function load_backend(): Promise<Backend> {
  if (import.meta.env.MODE === 'fixture') {
    const fixture_module = await import('./fixture_backend');
    return fixture_module.fixture_backend();
  }
  const api_module = await import('./api_client');
  return api_module.tauri_backend();
}
