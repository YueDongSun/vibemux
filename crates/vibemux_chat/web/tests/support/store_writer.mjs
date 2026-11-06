import { create_workspace_session } from "../../src/services/chat_store.js";

// Existing format/validation tests need a synchronous leased writer. Cross-tab
// and delayed Web Lock acquisition are exercised separately in persistence tests.
const sessions = new WeakMap();
function writer() {
  const storage = globalThis.localStorage;
  if (!sessions.has(storage)) {
    const session = create_workspace_session({
      locks: { request: (_name, _options, callback) => callback({}) },
    });
    session.acquire().catch(() => {});
    sessions.set(storage, session);
  }
  return sessions.get(storage);
}
export const save_workspace = (workspace) => writer().save(workspace);
export const reset_workspace = () => writer().reset();
