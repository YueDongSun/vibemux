<script lang="ts">
  import { onMount } from 'svelte';
  import {
    api_error,
    create_enrollment,
    load_health,
    load_task,
    load_tasks,
    load_public_key,
    redeem_enrollment,
    save_push_subscription,
    type freshness,
    type health_response,
    type task_status,
  } from './api_client';
  import { clear_authenticated_state, read_instance_token, save_instance_token } from './auth_state';
  import { snapshots_match } from './presentation_policy';
  import Task_board from './components/task_board.svelte';
  import Task_detail from './components/task_detail.svelte';

  let token = $state('');
  let token_input = $state('');
  let tasks = $state<task_status[]>([]);
  let selected_id = $state<string | null>(null);
  let selected_task = $state<task_status | null>(null);
  let data_freshness = $state<freshness | null>(null);
  let detail_freshness = $state<freshness | null>(null);
  let health = $state<health_response | null>(null);
  let busy = $state(false);
  let error_message = $state('');
  let enrollment_image = $state('');
  let enrollment_notice = $state('');
  let enrollment_input = $state('');
  let update_available = $state(false);
  let tasks_page_truncated = $state(false);
  let polling_timer: number | undefined;

  onMount(() => {
    token = read_instance_token();
    token_input = token;
    const on_update = () => (update_available = true);
    window.addEventListener('vibemux:update_available', on_update);
    return () => {
      window.removeEventListener('vibemux:update_available', on_update);
      if (polling_timer) window.clearInterval(polling_timer);
    };
  });

  $effect(() => {
    if (polling_timer) window.clearInterval(polling_timer);
    if (!token) return;
    void refresh_board();
    polling_timer = window.setInterval(() => void refresh_board(), 5000);
  });

  async function use_token(): Promise<void> {
    const next_token = token_input.trim();
    if (!next_token) {
      error_message = 'Enter the instance token supplied by your authenticated operator bootstrap.';
      return;
    }
    await clear_authenticated_state();
    save_instance_token(next_token);
    token = next_token;
    selected_id = null;
    selected_task = null;
    tasks = [];
    data_freshness = null;
    detail_freshness = null;
    error_message = '';
  }

  async function sign_out(): Promise<void> {
    await clear_authenticated_state();
    token = '';
    token_input = '';
    tasks = [];
    selected_id = null;
    selected_task = null;
    data_freshness = null;
    detail_freshness = null;
    health = null;
    enrollment_image = '';
    error_message = '';
  }

  async function expire_session(): Promise<void> {
    await clear_authenticated_state();
    token = '';
    token_input = '';
    tasks = [];
    selected_id = null;
    selected_task = null;
    data_freshness = null;
    detail_freshness = null;
    health = null;
    enrollment_image = '';
    error_message = 'The instance token was rejected or rotated. Every device must reconnect with the current authenticated bootstrap.';
  }

  async function refresh_board(): Promise<void> {
    if (!token || busy) return;
    busy = true;
    error_message = '';
    try {
      const [task_pages, health_page] = await Promise.all([load_task_pages(), load_health(token)]);
      if (!snapshots_match(task_pages, health_page)) {
        task_pages.stale = true;
        health_page.stale = true;
      }
      tasks = task_pages.tasks;
      tasks_page_truncated = task_pages.truncated;
      health = health_page;
      data_freshness = task_pages;
      if (!selected_id || !tasks.some((task) => task.task_id === selected_id)) {
        selected_id = tasks[0]?.task_id ?? null;
      }
      if (selected_id) await select_task(selected_id);
      else selected_task = null;
    } catch (error) {
      show_error(error);
    } finally {
      busy = false;
    }
  }

  async function load_task_pages(): Promise<{ tasks: task_status[]; truncated: boolean } & freshness> {
    const collected_tasks: task_status[] = [];
    let page = await load_tasks(token);
    const first_snapshot = { generation: page.generation, observed_at: page.observed_at };
    let pages_loaded = 0;
    let is_stale = page.stale;
    let is_truncated = page.truncated;
    let next_cursor = page.next_cursor;
    const seen_cursors = new Set<string>();
    collected_tasks.push(...page.tasks);

    while (next_cursor && pages_loaded < 7 && !seen_cursors.has(next_cursor)) {
      seen_cursors.add(next_cursor);
      page = await load_tasks(token, next_cursor);
      collected_tasks.push(...page.tasks);
      is_stale ||= page.stale || !snapshots_match(first_snapshot, page);
      is_truncated ||= page.truncated;
      next_cursor = page.next_cursor;
      pages_loaded += 1;
    }

    return {
      tasks: collected_tasks,
      truncated: is_truncated || next_cursor !== null,
      generation: page.generation,
      observed_at: page.observed_at,
      stale: is_stale,
    };
  }

  async function select_task(task_id: string): Promise<void> {
    selected_id = task_id;
    selected_task = null;
    detail_freshness = null;
    try {
      const response = await load_task(token, task_id);
      if (data_freshness && !snapshots_match(data_freshness, response)) response.stale = true;
      selected_task = response.task;
      detail_freshness = response;
    } catch (error) {
      show_error(error);
    }
  }

  async function enable_push(): Promise<void> {
    if (!('serviceWorker' in navigator) || !('PushManager' in window)) {
      error_message = 'Push notifications are not available in this browser.';
      return;
    }
    try {
      const permission = await Notification.requestPermission();
      if (permission !== 'granted') throw new Error('Notification permission was not granted.');
      const registration = await navigator.serviceWorker.ready;
      const { public_key } = await load_public_key(token);
      const application_server_key = decode_application_server_key(public_key);
      const subscription = await registration.pushManager.subscribe({
        userVisibleOnly: true,
        applicationServerKey: application_server_key,
      });
      await save_push_subscription(token, subscription);
      enrollment_notice = 'This browser is subscribed for state notifications.';
    } catch (error) {
      show_error(error);
    }
  }

  async function make_enrollment_qr(): Promise<void> {
    enrollment_image = '';
    enrollment_notice = '';
    try {
      const enrollment = await create_enrollment(token);
      const svg_blob = new Blob([enrollment.qr_svg], { type: 'image/svg+xml' });
      enrollment_image = URL.createObjectURL(svg_blob);
      enrollment_notice = `One-time token expires in ${enrollment.expires_in_seconds} seconds. A phone still needs an operator-provided authenticated browser bootstrap.`;
    } catch (error) {
      show_error(error);
    }
  }

  async function redeem_device_enrollment(): Promise<void> {
    if (!enrollment_input.trim()) return;
    try {
      const result = await redeem_enrollment(token, enrollment_input.trim());
      await clear_authenticated_state();
      save_instance_token(result.instance_token);
      token = result.instance_token;
      token_input = result.instance_token;
      enrollment_input = '';
      enrollment_notice = 'Device enrolled. Instance token rotation signs out every enrolled device.';
      error_message = '';
      await refresh_board();
    } catch (error) {
      show_error(error);
    }
  }

  function show_error(error: unknown): void {
    if (error instanceof api_error && error.status === 401) {
      void expire_session();
      return;
    }
    error_message = error instanceof Error ? error.message : 'The request could not be completed.';
  }

  function decode_application_server_key(value: string): ArrayBuffer {
    const normalized = value.replace(/-/g, '+').replace(/_/g, '/');
    const binary = atob(normalized.padEnd(Math.ceil(normalized.length / 4) * 4, '='));
    const buffer = new ArrayBuffer(binary.length);
    const bytes = new Uint8Array(buffer);
    for (let index = 0; index < binary.length; index += 1) {
      bytes[index] = binary.charCodeAt(index);
    }
    return buffer;
  }

  function apply_update(): void {
    window.dispatchEvent(new Event('vibemux:apply_update'));
  }
</script>

<svelte:head>
  <meta name="color-scheme" content="light" />
</svelte:head>

<div class="app_shell">
  <header class="top_bar">
    <a href="/" class="brand" aria-label="VibeMux Monitor home">
      <span class="brand_mark" aria-hidden="true">V</span>
      <span><strong>VibeMux</strong><small>Read-only monitor</small></span>
    </a>
    <div class="header_actions">
      {#if token}
        <span class="connection_state" class:connected={health?.daemon?.healthy}>
          <span class="state_dot" aria-hidden="true"></span>
          {health?.daemon?.healthy ? 'Daemon online' : 'Checking daemon'}
        </span>
        <button class="quiet_button" type="button" onclick={() => void refresh_board()} disabled={busy}>Refresh</button>
        <button class="quiet_button" type="button" onclick={() => void sign_out()}>Sign out</button>
      {/if}
    </div>
  </header>

  <main>
    {#if !token}
      <section class="welcome_panel panel">
        <p class="eyebrow">Private instance</p>
        <h1>Monitor your VibeMux runs.</h1>
        <p class="lead">Enter the instance token supplied by your operator’s authenticated browser bootstrap.</p>
        <form class="token_form" onsubmit={(event) => { event.preventDefault(); void use_token(); }}>
          <label for="instance_token">Instance bearer token</label>
          <div class="token_controls">
            <input id="instance_token" bind:value={token_input} type="password" autocomplete="off" spellcheck="false" />
            <button class="primary_button" type="submit">Connect</button>
          </div>
        </form>
        <p class="bootstrap_note">
          Every request, including the app shell and enrollment routes, requires bearer authentication. A QR code cannot bootstrap an ordinary phone browser; the operator must provide an authenticated browser/bootstrap mechanism first.
        </p>
      </section>
    {:else}
      <section class="dashboard_intro">
        <div>
          <p class="eyebrow">Project activity</p>
          <h1>Your task board</h1>
          <p class="lead">Read-only status from the local VibeMux daemon. Refreshes every five seconds.</p>
        </div>
        <div class="intro_badges">
          <span class="metric_badge"><strong>{tasks.length}</strong> tasks</span>
          <span class="metric_badge"><strong>{health?.daemon?.store_schema_version ?? '—'}</strong> store schema</span>
        </div>
      </section>

      {#if data_freshness?.stale || detail_freshness?.stale || health?.stale}
        <div class="stale_banner" role="status">
          <strong>Status is stale or unavailable.</strong> Task titles stay hidden until a fresh daemon response arrives.
        </div>
      {/if}
      {#if !data_freshness?.stale && tasks_page_truncated}
        <div class="stale_banner" role="status">
          <strong>Task list is bounded.</strong> The board shows the available snapshot page.
        </div>
      {/if}
      {#if error_message}<div class="error_banner" role="alert">{error_message}</div>{/if}
      {#if update_available}
        <div class="update_banner" role="status">
          <span>A new monitor version is ready.</span>
          <button class="small_button" type="button" onclick={apply_update}>Reload</button>
        </div>
      {/if}

      <div class="board_grid">
        <Task_board tasks={tasks} selected_id={selected_id} freshness={data_freshness} on_select={(task_id) => void select_task(task_id)} />
        <Task_detail task={selected_task} freshness={detail_freshness} />
      </div>

      <section class="tools_grid" aria-label="Device options">
        <article class="panel utility_panel health_panel">
          <div>
            <p class="eyebrow">DeepSeek Harness</p>
            <h2>{health?.dsh_status?.phase ?? 'Status unavailable'}</h2>
            <p>{health?.dsh_status ? `${health.dsh_status.provider ?? 'Provider unknown'} · ${health.dsh_status.turns_started} turns started · ${health.dsh_status.records} records` : 'No DSH session status is available from the daemon.'}</p>
          </div>
          {#if health?.dsh_status}<span class="state_pill" data-state={health.dsh_status.transport_open ? 'active' : 'idle'}>{health.dsh_status.transport_open ? 'Transport open' : 'Transport closed'}</span>{/if}
        </article>
        <article class="panel utility_panel">
          <div>
            <p class="eyebrow">Notifications</p>
            <h2>Stay informed</h2>
            <p>Subscribe this browser to best-effort read-only state notifications delivered by the local service.</p>
          </div>
          <button class="secondary_button" type="button" onclick={() => void enable_push()}>Enable notifications</button>
        </article>
        <article class="panel utility_panel">
          <div>
            <p class="eyebrow">Device access</p>
            <h2>Enroll a device</h2>
            <p>QR redemption still requires the operator’s authenticated browser/bootstrap on the receiving device.</p>
          </div>
          <button class="secondary_button" type="button" onclick={() => void make_enrollment_qr()}>Create one-time QR</button>
          {#if enrollment_image}<img class="enrollment_qr" src={enrollment_image} alt="One-time device enrollment QR code" />{/if}
          <form class="redeem_form" onsubmit={(event) => { event.preventDefault(); void redeem_device_enrollment(); }}>
            <label for="enrollment_token">One-time token</label>
            <div class="redeem_controls">
              <input id="enrollment_token" bind:value={enrollment_input} type="password" autocomplete="off" spellcheck="false" />
              <button class="small_button" type="submit" disabled={!enrollment_input.trim()}>Redeem</button>
            </div>
          </form>
          {#if enrollment_notice}<p class="notice_text" role="status">{enrollment_notice}</p>{/if}
        </article>
      </section>
      <p class="updated_at">{data_freshness?.observed_at ? `Last observed ${new Date(data_freshness.observed_at).toLocaleString()}` : 'Waiting for the first daemon response'}</p>
    {/if}
  </main>
</div>
