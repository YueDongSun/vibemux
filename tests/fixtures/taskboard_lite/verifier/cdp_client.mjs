// Minimal Chrome DevTools Protocol client over the global WebSocket (Node 22+).
// Supports flattened target sessions, per-command timeouts, and event waits
// that are registered before the triggering command to avoid races.

export const cdp_config = Object.freeze({
  connect_timeout_ms: 10_000,
  command_timeout_ms: 15_000,
  event_timeout_ms: 15_000,
});

export class Cdp_error extends Error {
  constructor(message, details) {
    super(message);
    this.name = "Cdp_error";
    this.details = details;
  }
}

export class Cdp_connection {
  #socket;
  #next_command_id = 1;
  #pending_commands = new Map();
  #event_listeners = new Set();
  #closed = false;
  #close_reason = null;

  constructor(socket) {
    this.#socket = socket;
    socket.addEventListener("message", (event) => this.#handle_message(event.data));
    socket.addEventListener("close", () => this.#handle_close("websocket closed"));
    socket.addEventListener("error", () => this.#handle_close("websocket error"));
  }

  static async connect(websocket_url, { timeout_ms = cdp_config.connect_timeout_ms } = {}) {
    const socket = new WebSocket(websocket_url);
    await new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        socket.close();
        reject(new Cdp_error(`CDP connect timed out after ${timeout_ms} ms`));
      }, timeout_ms);
      socket.addEventListener("open", () => {
        clearTimeout(timer);
        resolve();
      }, { once: true });
      socket.addEventListener("error", () => {
        clearTimeout(timer);
        reject(new Cdp_error(`CDP connect failed for ${websocket_url}`));
      }, { once: true });
    });
    return new Cdp_connection(socket);
  }

  get closed() {
    return this.#closed;
  }

  #handle_message(raw_data) {
    let message;
    try {
      message = JSON.parse(typeof raw_data === "string" ? raw_data : Buffer.from(raw_data).toString("utf8"));
    } catch {
      return;
    }
    if (message.id !== undefined) {
      const pending = this.#pending_commands.get(message.id);
      if (pending === undefined) {
        return;
      }
      this.#pending_commands.delete(message.id);
      clearTimeout(pending.timer);
      if (message.error !== undefined) {
        pending.reject(new Cdp_error(`${pending.method} failed: ${message.error.message}`, message.error));
      } else {
        pending.resolve(message.result ?? {});
      }
      return;
    }
    for (const listener of [...this.#event_listeners]) {
      listener(message);
    }
  }

  #handle_close(reason) {
    if (this.#closed) {
      return;
    }
    this.#closed = true;
    this.#close_reason = reason;
    for (const pending of this.#pending_commands.values()) {
      clearTimeout(pending.timer);
      pending.reject(new Cdp_error(`${pending.method} aborted: ${reason}`));
    }
    this.#pending_commands.clear();
  }

  send(method, params = {}, { session_id, timeout_ms = cdp_config.command_timeout_ms } = {}) {
    if (this.#closed) {
      return Promise.reject(new Cdp_error(`${method} not sent: ${this.#close_reason}`));
    }
    const command_id = this.#next_command_id;
    this.#next_command_id += 1;
    const message = { id: command_id, method, params };
    if (session_id !== undefined) {
      message.sessionId = session_id;
    }
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        this.#pending_commands.delete(command_id);
        reject(new Cdp_error(`${method} timed out after ${timeout_ms} ms`));
      }, timeout_ms);
      this.#pending_commands.set(command_id, { method, resolve, reject, timer });
      this.#socket.send(JSON.stringify(message));
    });
  }

  // Subscribes to every event; returns an unsubscribe function.
  on_event(listener) {
    this.#event_listeners.add(listener);
    return () => this.#event_listeners.delete(listener);
  }

  // Returns a promise for the next matching event. Call before sending the
  // command that triggers it.
  wait_for_event(method, { session_id, predicate = () => true, timeout_ms = cdp_config.event_timeout_ms } = {}) {
    return new Promise((resolve, reject) => {
      const unsubscribe = this.on_event((message) => {
        if (message.method !== method || (session_id !== undefined && message.sessionId !== session_id)) {
          return;
        }
        if (!predicate(message.params ?? {})) {
          return;
        }
        clearTimeout(timer);
        unsubscribe();
        resolve(message.params ?? {});
      });
      const timer = setTimeout(() => {
        unsubscribe();
        reject(new Cdp_error(`event ${method} not received within ${timeout_ms} ms`));
      }, timeout_ms);
    });
  }

  close() {
    if (!this.#closed) {
      this.#socket.close();
      this.#handle_close("closed by client");
    }
  }
}

// A flattened session bound to one page target.
export class Cdp_session {
  constructor(connection, session_id, target_id) {
    this.connection = connection;
    this.session_id = session_id;
    this.target_id = target_id;
  }

  send(method, params = {}, options = {}) {
    return this.connection.send(method, params, { ...options, session_id: this.session_id });
  }

  wait_for_event(method, options = {}) {
    return this.connection.wait_for_event(method, { ...options, session_id: this.session_id });
  }

  on_event(listener) {
    return this.connection.on_event((message) => {
      if (message.sessionId === this.session_id) {
        listener(message);
      }
    });
  }
}
