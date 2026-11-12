export class BridgeError extends Error {
  constructor(code, status = 400, outcome = 'not_started', message) {
    super(message ?? publicMessage(code, outcome));
    this.name = 'BridgeError';
    this.code = code;
    this.status = status;
    this.outcome = outcome;
  }
}

export function publicMessage(code, outcome) {
  if (outcome === 'unknown') {
    return 'The command receipt is unavailable. Check daemon state before deciding whether to retry with the original event ID.';
  }
  if (code === 'operation_unsupported') {
    return 'The installed CLI does not advertise this operation. Keep the draft and use a matching backend.';
  }
  if (code === 'writes_disabled') return 'Writes are disabled in this bridge configuration.';
  if (code === 'intake_import_disabled') return 'This project accepts only configured intake presets.';
  if (code === 'preset_unavailable') return 'Choose an intake preset configured for this project.';
  if (code === 'report_content_disabled') return 'Report content reads are disabled in this bridge configuration.';
  if (code === 'bridge_unauthorized') return 'A valid bridge bearer token is required.';
  if (code === 'origin_rejected' || code === 'host_rejected') return 'This request origin is not allowed.';
  if (code === 'bridge_busy') return 'The bridge is at its command limit. Retry a read after the current command settles.';
  if (code === 'cli_unavailable') return 'The configured VibeMux CLI is unavailable.';
  if (code === 'invalid_json') return 'Send one UTF-8 JSON object.';
  if (code === 'body_too_large') return 'The request exceeds the bridge input limit.';
  if (code === 'unknown_project') return 'Choose a project configured in this bridge.';
  if (code === 'invalid_configuration') return 'The bridge configuration is invalid.';
  return 'The operation was refused. Review its error code and the current daemon state.';
}

export function errorBody(error, eventId) {
  const known = error instanceof BridgeError
    ? error
    : new BridgeError('bridge_internal_error', 500);
  return {
    ok: false,
    error: { code: known.code, message: known.message, outcome: known.outcome },
    ...(eventId ? { event_id: eventId } : {}),
  };
}

export function assert(condition, code = 'invalid_request', status = 400) {
  if (!condition) throw new BridgeError(code, status);
}

export function object(value) {
  return value !== null && typeof value === 'object' && !Array.isArray(value);
}

export function exactKeys(value, allowed, required = []) {
  assert(object(value));
  assert(Object.keys(value).every((key) => allowed.includes(key)));
  assert(required.every((key) => Object.hasOwn(value, key)));
}

export function uuid(value) {
  return typeof value === 'string'
    && /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(value)
    && value !== '00000000-0000-0000-0000-000000000000';
}

export function safeInteger(value, minimum = 0, maximum = Number.MAX_SAFE_INTEGER) {
  return Number.isSafeInteger(value) && value >= minimum && value <= maximum;
}
