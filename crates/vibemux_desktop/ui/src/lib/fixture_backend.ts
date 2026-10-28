import type { Backend } from './backend';
import type { CommandName, UiEventName } from './ui_contract';

/// Marker string that must never appear in a production bundle. The
/// fixture guard test fails the build when it does.
export const FIXTURE_MARKER = 'VIBEMUX_FIXTURE_DATA';

/// Every fixture payload carries this label so the UI can show a DEMO tag.
export const FIXTURE_MODE = 'demo';

export interface FixtureEnvelope {
  mode: typeof FIXTURE_MODE;
  marker: string;
  command: string;
  data: unknown;
}

/// Wraps demo data in the labeled envelope.
export function fixture_payload(command: string, data: unknown = null): FixtureEnvelope {
  return { mode: FIXTURE_MODE, marker: FIXTURE_MARKER, command, data };
}

/// The demo backend. It answers every command with labeled demo data and
/// never emits an event.
export function fixture_backend(): Backend {
  return {
    async invoke<T>(command: CommandName): Promise<T> {
      return fixture_payload(command) as unknown as T;
    },
    async listen(_event: UiEventName, _handler: (payload: unknown) => void): Promise<() => void> {
      return () => undefined;
    },
  };
}
