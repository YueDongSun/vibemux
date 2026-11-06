import { create_workspace_session } from "../../src/services/chat_store.js";
import { app_config } from "../../src/config.js";

export function persistence_fixture(context, raw = null) {
  let value = raw;
  const writes = [];
  const storage = {
    getItem: (key) => key === app_config.storage_key ? value : null,
    setItem: (key, next) => {
      if (key !== app_config.storage_key) throw new Error("Wrong storage key");
      value = next;
      writes.push(next);
    },
    removeItem: (key) => {
      if (key !== app_config.storage_key) throw new Error("Wrong storage key");
      value = null;
    },
  };
  const original = Object.getOwnPropertyDescriptor(globalThis, "localStorage");
  Object.defineProperty(globalThis, "localStorage", { configurable: true, value: storage });
  context.after(() => {
    if (original) Object.defineProperty(globalThis, "localStorage", original);
    else delete globalThis.localStorage;
  });
  // Like Web Locks, callbacks run asynchronously and exclusive ownership lasts
  // until the callback's returned promise settles. There is no lock stealing.
  let held = false;
  const locks = {
    request(name, options, callback) {
      if (name !== `${app_config.storage_key}:writer` ||
          options.mode !== "exclusive" || options.ifAvailable !== true) {
        throw new Error("Unexpected lock request");
      }
      return Promise.resolve().then(() => {
        if (held) return callback(null);
        held = true;
        return Promise.resolve(callback({ name })).finally(() => { held = false; });
      });
    },
  };
  return {
    storage, writes, locks,
    raw: () => value,
    replace: (next) => { value = next; },
    session() {
      const session = create_workspace_session({ locks });
      context.after(() => session.release());
      return session;
    },
  };
}

export async function settle() {
  for (let i = 0; i < 8; i++) await Promise.resolve();
}
