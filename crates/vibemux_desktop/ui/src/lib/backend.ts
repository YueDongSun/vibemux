import type { CommandName, UiEventName } from './ui_contract';

/// The only channel the UI has to the Rust side. The production backend
/// calls the fixed Tauri command list; the fixture backend serves demo
/// data and is loaded only in fixture mode.
export interface Backend {
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
