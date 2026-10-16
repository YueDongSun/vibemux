const token_storage_key = 'vibemux.instance_token';

export function read_instance_token(): string {
  return sessionStorage.getItem(token_storage_key) ?? '';
}

export function save_instance_token(token: string): void {
  sessionStorage.setItem(token_storage_key, token.trim());
}

export async function clear_authenticated_state(): Promise<void> {
  sessionStorage.removeItem(token_storage_key);
  const registrations = await navigator.serviceWorker?.getRegistrations();
  await Promise.all((registrations ?? []).map((registration) =>
    registration.active?.postMessage({ kind: 'clear_authenticated_cache' }),
  ));
}
