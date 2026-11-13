import http from 'node:http';
import { createHash, timingSafeEqual } from 'node:crypto';
import { BridgeError, errorBody, object, uuid } from './errors.mjs';
import { MAX_REQUEST_BYTES } from './config.mjs';
import { CliTransport } from './cli.mjs';
import { BridgeService, HTTP_STATUS } from './service.mjs';

const MAX_HTTP_REQUESTS = 24;
const JSON_TYPE = /^application\/json(?:\s*;\s*charset=utf-8)?$/i;

function headerCount(request, wanted) {
  let count = 0;
  for (let index = 0; index < request.rawHeaders.length; index += 2) {
    if (request.rawHeaders[index].toLowerCase() === wanted) count += 1;
  }
  return count;
}

function authenticate(request, config, tokenDigest) {
  const port = request.socket.localPort;
  const hosts = new Set(['127.0.0.1:' + port, 'localhost:' + port, '[::1]:' + port]);
  if (headerCount(request, 'host') !== 1 || !hosts.has(request.headers.host)) {
    throw new BridgeError('host_rejected', 403);
  }
  const origin = request.headers.origin;
  if (headerCount(request, 'origin') > 1
    || (origin !== undefined && !config.allowedOrigins.includes(origin))
    || request.headers['sec-fetch-site'] === 'cross-site') {
    throw new BridgeError('origin_rejected', 403);
  }
  const authorization = request.headers.authorization;
  if (headerCount(request, 'authorization') !== 1 || typeof authorization !== 'string'
    || !authorization.startsWith('Bearer ') || authorization.length > 263) {
    throw new BridgeError('bridge_unauthorized', 401);
  }
  const candidate = createHash('sha256').update(authorization.slice(7)).digest();
  if (!timingSafeEqual(candidate, tokenDigest)) throw new BridgeError('bridge_unauthorized', 401);
}

function readJson(request) {
  if (!JSON_TYPE.test(request.headers['content-type'] ?? '')) {
    throw new BridgeError('json_content_type_required', 415);
  }
  const length = request.headers['content-length'];
  if (length !== undefined && (!/^[0-9]+$/.test(length) || Number(length) > MAX_REQUEST_BYTES)) {
    throw new BridgeError('body_too_large', 413);
  }
  return new Promise((resolve, reject) => {
    const chunks = [];
    let bytes = 0;
    let settled = false;
    const timer = setTimeout(() => finish(new BridgeError('request_timeout', 408)), 10000);
    function finish(error, value) {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      request.removeListener('data', onData);
      request.removeListener('end', onEnd);
      request.removeListener('aborted', onAborted);
      request.removeListener('error', onError);
      if (error) reject(error); else resolve(value);
    }
    function onData(chunk) {
      bytes += chunk.length;
      if (bytes > MAX_REQUEST_BYTES) return finish(new BridgeError('body_too_large', 413));
      chunks.push(chunk);
    }
    function onAborted() { finish(new BridgeError('request_aborted', 400)); }
    function onError() { finish(new BridgeError('request_unavailable', 400)); }
    function onEnd() {
      try {
        const value = JSON.parse(new TextDecoder('utf-8', { fatal: true }).decode(Buffer.concat(chunks)));
        if (!object(value)) throw new Error('shape');
        finish(null, value);
      } catch { finish(new BridgeError('invalid_json', 400)); }
    }
    request.on('data', onData);
    request.once('end', onEnd);
    request.once('aborted', onAborted);
    request.once('error', onError);
  });
}

function send(response, status, body) {
  if (response.destroyed) return;
  response.writeHead(status, {
    'content-type': 'application/json; charset=utf-8',
    'cache-control': 'no-store',
    'referrer-policy': 'no-referrer',
    'x-content-type-options': 'nosniff',
    'content-security-policy': "default-src 'none'; frame-ancestors 'none'",
    'vary': 'Authorization',
    ...(status >= 400 ? { connection: 'close' } : {}),
  });
  response.end(JSON.stringify(body));
}

export function createBridgeServer(config, { transport = new CliTransport(config) } = {}) {
  const service = new BridgeService(config, transport);
  const tokenDigest = createHash('sha256').update(config.token).digest();
  let active = 0;
  let stopping = false;
  const server = http.createServer({
    requestTimeout: 15000, headersTimeout: 10000, maxHeaderSize: 8192,
  }, async (request, response) => {
    let admitted = false;
    let eventId;
    try {
      authenticate(request, config, tokenDigest);
      if (stopping) throw new BridgeError('bridge_stopping', 503);
      if (active >= MAX_HTTP_REQUESTS) throw new BridgeError('bridge_busy', 429);
      active += 1;
      admitted = true;
      if (!['GET', 'POST'].includes(request.method)) throw new BridgeError('method_not_allowed', 405);
      if ((request.url?.length ?? 0) > 2048) throw new BridgeError('invalid_request', 414);
      const url = new URL(request.url, 'http://127.0.0.1');
      if (url.origin !== 'http://127.0.0.1') throw new BridgeError('invalid_request', 400);
      const body = request.method === 'POST' ? await readJson(request) : undefined;
      if (uuid(body?.event_id)) eventId = body.event_id;
      const result = await service.route(request.method, url, body);
      send(response, result[HTTP_STATUS] ?? 200, result);
    } catch (error) {
      send(response, error instanceof BridgeError ? error.status : 500, errorBody(error, eventId));
    } finally {
      if (admitted) active -= 1;
    }
  });
  server.on('clientError', (_error, socket) => socket.end('HTTP/1.1 400 Bad Request\r\nConnection: close\r\n\r\n'));
  return {
    server, service, transport,
    async close() {
      stopping = true;
      const closed = new Promise((resolve) => server.close(resolve));
      await transport.close();
      server.closeIdleConnections();
      await closed;
    },
  };
}
