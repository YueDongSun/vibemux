import { app_config } from "../config.js";

/** Owns both timers and page lifecycle listeners; all writes use the same lease. */
export function create_workspace_autosave({
  session,
  snapshot,
  on_error,
  on_saved,
  delay = app_config.save_delay,
  max_delay = app_config.save_max_delay,
}) {
  let latest;
  let dirty = false;
  let debounce;
  let deadline;
  let disposed = false;
  let detach;
  const clear_timers = () => {
    clearTimeout(debounce);
    clearTimeout(deadline);
    debounce = undefined;
    deadline = undefined;
  };
  const capture = (value) => {
    latest = value;
    dirty = true;
  };
  const flush = () => {
    clear_timers();
    if (disposed || !dirty) return true;
    try {
      session.save(latest);
      dirty = false;
      on_saved();
      return true;
    } catch (error) {
      on_error(error);
      return false;
    }
  };
  const start = async () => {
    if (disposed) return false;
    try {
      await session.acquire();
      if (disposed) return false;
      return flush();
    } catch (error) {
      if (!disposed) on_error(error);
      return false;
    }
  };
  return {
    schedule(value) {
      if (disposed) return;
      capture(value);
      clearTimeout(debounce);
      debounce = setTimeout(flush, delay);
      // Do not reset this timer on another token: a live stream must checkpoint.
      if (deadline === undefined) deadline = setTimeout(flush, max_delay);
    },
    async persist(value) {
      if (disposed) return false;
      capture(value);
      clear_timers();
      if (!session.writable) return start();
      return flush();
    },
    flush_now() {
      if (disposed) return false;
      capture(snapshot());
      return flush();
    },
    discard() {
      clear_timers();
      dirty = false;
      latest = undefined;
    },
    attach(window_target, document_target) {
      const hidden = () => {
        if (document_target.visibilityState === "hidden") {
          capture(snapshot());
          flush();
        }
      };
      const hide = () => {
        capture(snapshot());
        flush();
        session.release();
      };
      const show = () => { void start(); };
      window_target.addEventListener("pagehide", hide);
      window_target.addEventListener("pageshow", show);
      document_target.addEventListener("visibilitychange", hidden);
      detach = () => {
        window_target.removeEventListener("pagehide", hide);
        window_target.removeEventListener("pageshow", show);
        document_target.removeEventListener("visibilitychange", hidden);
      };
      void start();
    },
    dispose() {
      if (disposed) return;
      capture(snapshot());
      flush();
      disposed = true;
      clear_timers();
      detach?.();
      session.release();
    },
  };
}
