<script lang="ts">
  import { onMount } from 'svelte';
  import TaskDetail from './task_detail.svelte';
  import { load_backend, type Backend } from '../lib/backend';
  import { command_code } from '../lib/native_state';
  import { TaskWindowState, type TaskWindowBinding, type TaskWindowView } from '../lib/task_window_state';
  import type { PublicSettings } from '../lib/ui_contract';

  const READ_REFRESH_MS = 5000;
  let { binding }: { binding: TaskWindowBinding } = $props();
  let view = $state<TaskWindowView | null>(null);
  let settings = $state<PublicSettings | null>(null);
  let startup_error = $state('');
  let settings_error = $state('');
  let fixture_mode = $state(false);
  let request_refresh: () => void = () => {};

  $effect(() => {
    if (settings?.appearance && settings.appearance !== 'system') document.documentElement.dataset.theme = settings.appearance;
    else delete document.documentElement.dataset.theme;
    document.documentElement.dataset.text_size = settings?.text_size ?? 'default';
    document.documentElement.dataset.reduce_motion = String(settings?.reduce_motion ?? false);
  });

  onMount(() => {
    // A native window stays bound to its initial identities until destroyed.
    const window_binding = { ...binding };
    let disposed = false;
    let model: TaskWindowState | null = null;
    let timer: ReturnType<typeof setTimeout> | undefined;

    async function refresh_detail(): Promise<void> {
      if (disposed || !model) return;
      if (timer !== undefined) { clearTimeout(timer); timer = undefined; }
      await model.refresh();
      if (disposed || timer !== undefined) return;
      timer = setTimeout(() => {
        timer = undefined;
        void refresh_detail();
      }, READ_REFRESH_MS);
    }

    async function read_settings(loaded: Backend): Promise<void> {
      try {
        const result = await loaded.invoke('settings_get', {});
        if (!disposed) settings = result;
      } catch (error: unknown) {
        if (!disposed) settings_error = `Appearance settings could not be read (${command_code(error)}). Default appearance is in use.`;
      }
    }

    request_refresh = () => { void refresh_detail(); };
    void (async () => {
      try {
        const loaded = await load_backend();
        if (disposed) return;
        fixture_mode = loaded.mode === 'fixture';
        model = new TaskWindowState(window_binding, {
          tasks_snapshot: (project_key, query) => loaded.invoke('tasks_snapshot', { project_key, query }),
        }, (next) => { if (!disposed) view = next; });
        view = model.snapshot();
        void read_settings(loaded);
        await refresh_detail();
      } catch (error: unknown) {
        if (!disposed) startup_error = `Task window unavailable (${command_code(error)}). Reopen the locally built desktop app.`;
      }
    })();

    return () => {
      disposed = true;
      if (timer !== undefined) clearTimeout(timer);
      model?.dispose();
      request_refresh = () => {};
    };
  });
</script>

<main class="task_window" aria-labelledby="task_window_heading">
  <header class="window_header"><h1 id="task_window_heading">Task</h1></header>
  {#if fixture_mode}<p class="read_notice" role="status">DEMO FIXTURE · No daemon or harness execution</p>{/if}
  {#if settings_error}<p class="read_notice" role="alert">{settings_error}</p>{/if}
  {#if startup_error}<p class="read_notice" role="alert">{startup_error}</p>{/if}
  {#if view}
    <p class="freshness">{view.observed_at ? `Observed ${view.observed_at}` : 'No task observation yet.'}</p>
    <section class="window_detail" aria-label="Task details">
      <TaskDetail project_key={binding.project_key} task_id={view.task_id} detail={view.detail} loading={view.loading} error={view.error} on_refresh={() => request_refresh()} />
    </section>
  {:else if !startup_error}<p class="loading_notice" role="status">Loading task window…</p>{/if}
</main>

<style>
  .task_window{display:flex;flex-direction:column;height:100%;min-width:0;min-height:0;overflow:auto;background:var(--bg);color:var(--text_primary)}
  .window_header{padding:25px 28px 16px}.window_header h1{font:400 2rem/1.2 var(--font_display);margin:0}.freshness{color:var(--muted_text);font-size:.8rem}.freshness{margin:0 28px 16px}.read_notice{margin:12px 28px;padding:12px 14px;border:1px solid var(--border);border-radius:9px;background:var(--surface);font-size:.8rem;overflow-wrap:anywhere}.loading_notice{padding:20px 28px;color:var(--muted_text);font-size:.867rem}.window_detail{flex:1;min-width:0;padding:20px 28px;border-top:1px solid var(--border);background:var(--surface_alt)}
  @media(max-width:600px){.window_header,.window_detail,.loading_notice{padding-left:18px;padding-right:18px}.freshness,.read_notice{margin-left:18px;margin-right:18px}}
</style>
