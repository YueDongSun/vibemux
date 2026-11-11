import { command_error_message, invoke_native, type Backend } from './backend.ts';
import type { ProviderProfilesConfig, ProviderProfilesView } from './ui_contract.ts';

export const MAX_PROVIDER_PROFILES = 16;
export const MAX_PROVIDER_CONFIG_BYTES = 64 * 1024;
const bytes = (value: string) => new TextEncoder().encode(value).byteLength;

function require_value(value: unknown, message: string): asserts value {
  if (!value) throw new Error(message);
}

function exact_fields(value: unknown, fields: string[], message: string): asserts value is Record<string, unknown> {
  require_value(value !== null && typeof value === 'object' && !Array.isArray(value), message);
  const keys = Object.keys(value);
  require_value(keys.length === fields.length && fields.every((field) => Object.hasOwn(value, field)), message);
}

function bounded_text(value: unknown, limit: number): value is string {
  return typeof value === 'string' && value.length > 0 && value === value.trim() &&
    bytes(value) <= limit && !/[\x00-\x1f\x7f-\x9f]/.test(value);
}

export function validate_profiles(config: unknown): asserts config is ProviderProfilesConfig {
  exact_fields(config, ['schema_version', 'profiles', 'orchestrator_profile_id'], 'Unsupported provider configuration fields.');
  require_value(config.schema_version === 1, 'Unsupported provider configuration version.');
  require_value(Array.isArray(config.profiles) && config.profiles.length <= MAX_PROVIDER_PROFILES, 'Use at most 16 provider profiles.');
  const ids = new Set<string>();
  for (const profile of config.profiles) {
    exact_fields(profile, ['id', 'name', 'api_type', 'base_url', 'model', 'credential_env', 'auth_type'], 'Unsupported provider profile fields.');
    require_value(typeof profile.id === 'string' && /^[A-Za-z0-9_-]{1,64}$/.test(profile.id), 'Profile IDs need 1–64 ASCII letters, digits, underscores or hyphens.');
    require_value(!ids.has(profile.id), 'Profile IDs must be unique.');
    ids.add(profile.id);
    require_value(bounded_text(profile.name, 128), 'Each name needs 1–128 UTF-8 bytes without padding or control characters.');
    require_value(bounded_text(profile.model, 256), 'Each model needs 1–256 UTF-8 bytes without padding or control characters.');
    require_value(typeof profile.api_type === 'string' && ['anthropic_messages', 'openai_chat_completions'].includes(profile.api_type), 'Choose a supported API type.');
    require_value(typeof profile.base_url === 'string' && bytes(profile.base_url) <= 2048 &&
      /^https?:\/\//i.test(profile.base_url) && !/[\s\x00-\x1f\x7f-\x9f?#\\]/.test(profile.base_url),
    'Use an absolute HTTPS base URL without a query, fragment or whitespace, or HTTP at a numeric loopback address.');
    let url: URL;
    try { url = new URL(profile.base_url); }
    catch { throw new Error('Enter a valid provider base URL.'); }
    const authority = profile.base_url.split('/')[2];
    require_value(authority.length > 0 && !authority.includes('@') && !url.username && !url.password,
      'Provider URLs need a host and must not contain credentials.');
    require_value(url.port !== '0', 'Provider URLs cannot use port zero.');
    const loopback = url.hostname === '[::1]' || /^127\.\d+\.\d+\.\d+$/.test(url.hostname);
    require_value(url.protocol === 'https:' || (url.protocol === 'http:' && loopback), 'HTTP is allowed only at a numeric loopback address. Use HTTPS for other providers.');
    require_value(typeof profile.auth_type === 'string' && ['api_key', 'bearer', 'none'].includes(profile.auth_type), 'Choose a supported authentication type.');
    require_value(profile.auth_type === 'none'
      ? profile.credential_env === null
      : typeof profile.credential_env === 'string' && /^[A-Za-z_][A-Za-z0-9_]{0,127}$/.test(profile.credential_env),
    'Use an environment-variable name (not its value), or select no authentication.');
  }
  require_value(config.orchestrator_profile_id === null ||
    (typeof config.orchestrator_profile_id === 'string' && ids.has(config.orchestrator_profile_id)),
  'Select an existing orchestrator profile, or leave it unselected.');
  require_value(bytes(JSON.stringify(config)) <= MAX_PROVIDER_CONFIG_BYTES, 'Provider configuration must fit within 64 KiB.');
}

export function validate_profiles_view(view: unknown): asserts view is ProviderProfilesView {
  exact_fields(view, ['config', 'readiness'], 'Unsupported provider configuration response.');
  require_value(view.readiness === 'configuration_only', 'Unsupported provider readiness.');
  validate_profiles(view.config);
}

// Configuration only. This port never resolves an environment variable, reads a
// credential, contacts a provider, tests connectivity, or starts a workflow.
export function create_provider_profiles(backend: Backend) {
  let loaded = false;
  let reading: Promise<ProviderProfilesView> | null = null;
  let saving = false;
  return {
    max_profiles: MAX_PROVIDER_PROFILES,
    validate: validate_profiles,
    get(): Promise<ProviderProfilesView> {
      if (reading) return reading;
      if (saving) return Promise.reject(new Error('Wait for the provider configuration save to finish.'));
      loaded = false;
      reading = Promise.resolve().then(async () => {
        try {
          const view = await invoke_native(backend, 'provider_profiles_get');
          validate_profiles_view(view);
          loaded = true;
          return structuredClone(view);
        } catch (error) {
          throw new Error(command_error_message(error, 'Provider configuration could not be read. Nothing was changed; fix the configuration or retry.'));
        } finally { reading = null; }
      });
      return reading;
    },
    async save(config: ProviderProfilesConfig): Promise<ProviderProfilesView> {
      require_value(loaded && !reading, 'Read the existing provider configuration successfully before saving.');
      require_value(!saving, 'A provider configuration save is already in progress.');
      validate_profiles(config);
      saving = true;
      try {
        const view = await invoke_native(backend, 'provider_profiles_set', { config: structuredClone(config) });
        validate_profiles_view(view);
        return structuredClone(view);
      } catch (error) {
        throw new Error(command_error_message(error, 'Provider configuration was not confirmed saved. Your edits remain here; retry or reopen settings.'));
      } finally { saving = false; }
    },
  };
}
