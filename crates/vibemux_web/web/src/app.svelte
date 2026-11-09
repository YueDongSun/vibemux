<script lang="ts">
  import { onMount, untrack } from 'svelte';
  import {
    api_error,
    create_enrollment,
    load_console,
    load_health,
    load_task,
    load_tasks,
    load_public_key,
    redeem_enrollment,
    save_push_subscription,
    type console_response,
    type freshness,
    type health_response,
    type task_activity,
    type task_status,
  } from './api_client';
  import { clear_authenticated_state, read_instance_token, save_instance_token } from './auth_state';
  import {
    connection_summary,
    console_is_current,
    type console_view_id,
    is_console_view,
    observed_at_for_view,
    view_for_key,
  } from './console_policy';
  import { snapshots_match } from './presentation_policy';
  import Daemon_view from './components/daemon_view.svelte';
  import Decisions_view from './components/decisions_view.svelte';
  import Sessions_view from './components/sessions_view.svelte';
  import Task_board from './components/task_board.svelte';
  import Task_detail from './components/task_detail.svelte';
  import View_tabs from './components/view_tabs.svelte';

  let token = $state('');
  let token_input = $state('');
  let tasks = $state<task_status[]>([]);
  let selected_id = $state<string | null>(null);
  let selected_task = $state<task_status | null>(null);
  let selected_activity = $state<task_activity | null>(null);
  let console_data = $state<console_response | null>(null);
  let active_view = $state<console_view_id>('tasks');
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

  // Every callback belongs to one authentication generation. Aborting fetch is
  // a resource bound; checking ownership also handles already-settled responses.
  type request_context = { generation: number; token: string; controller: AbortController };
  let auth_generation = 0;
  const active_requests = new Set<request_context>();
  let board_request: request_context | null = null;
  let detail_request: request_context | null = null;
  let enrollment_request: request_context | null = null;
  // One-time enrollment tokens are minted server-side, so a second QR request
  // would leave a second valid token behind. This owner is separate from the
  // redemption owner and is released by its own finalizer.
  let qr_request: request_context | null = null;

  const shown_observed_at = $derived(
    observed_at_for_view(active_view, console_data?.observed_at, data_freshness?.observed_at),
  );

  onMount(() => {
    token = read_instance_token();
    token_input = token;
    const on_update = () => (update_available = true);
    window.addEventListener('vibemux:update_available', on_update);
    window.addEventListener('keydown', on_key);
    return () => {
      window.removeEventListener('vibemux:update_available', on_update);
      window.removeEventListener('keydown', on_key);
      invalidate_requests();
      clear_enrollment_image();
    };
  });

  // Depends on the token only. refresh_board reads and writes `busy`, so
  // calling it tracked would re-run this effect after every refresh and
  // turn the five-second poll into a tight loop.
  $effect(() => {
    if (!token) return;
    untrack(() => void refresh_board());
    const polling_timer = window.setInterval(() => void refresh_board(), 5000);
    return () => window.clearInterval(polling_timer);
  });

  function on_key(event: KeyboardEvent): void {
    if (!token || event.altKey || event.ctrlKey || event.metaKey) return;
    const target = event.target;
    if (target instanceof HTMLElement && (target.isContentEditable || ['INPUT', 'TEXTAREA', 'SELECT'].includes(target.tagName))) return;
    const view = view_for_key(event.key);
    if (view) {
      active_view = view;
      event.preventDefault();
    } else if (event.key === 'r' || event.key === 'R') {
      void refresh_board();
      event.preventDefault();
    }
  }

  function clear_daemon_views(): void {
    tasks = [];
    tasks_page_truncated = false;
    selected_id = null;
    selected_task = null;
    selected_activity = null;
    console_data = null;
    data_freshness = null;
    detail_freshness = null;
  }

  function invalidate_requests(): number {
    auth_generation += 1;
    for (const request of active_requests) request.controller.abort();
    active_requests.clear();
    board_request = null;
    detail_request = null;
    enrollment_request = null;
    qr_request = null;
    busy = false;
    return auth_generation;
  }

  function begin_request(): request_context | null {
    if (!token || enrollment_request) return null;
    const request = { generation: auth_generation, token, controller: new AbortController() };
    active_requests.add(request);
    return request;
  }

  function request_is_current(request: request_context): boolean {
    return request.generation === auth_generation && request.token === token && !request.controller.signal.aborted;
  }

  function finish_request(request: request_context): void {
    request.controller.abort();
    active_requests.delete(request);
  }

  function clear_enrollment_image(): void {
    if (enrollment_image) URL.revokeObjectURL(enrollment_image);
    enrollment_image = '';
  }

  function begin_session_change(): number {
    const generation = invalidate_requests();
    token = '';
    token_input = '';
    clear_daemon_views();
    health = null;
    clear_enrollment_image();
    enrollment_input = '';
    enrollment_notice = '';
    error_message = '';
    return generation;
  }

  async function install_token(next_token: string, notice = ''): Promise<void> {
    const generation = begin_session_change();
    try {
      await clear_authenticated_state();
      if (generation !== auth_generation) return;
      save_instance_token(next_token);
      token = next_token;
      token_input = next_token;
      enrollment_notice = notice;
    } catch {
      if (generation === auth_generation) error_message = 'The authenticated browser state could not be updated.';
    }
  }

  async function use_token(): Promise<void> {
    const next_token = token_input.trim();
    if (!next_token) {
      error_message = 'Enter the instance token of this vibemux_web process.';
      return;
    }
    await install_token(next_token);
  }

  async function sign_out(): Promise<void> {
    const generation = begin_session_change();
    try {
      await clear_authenticated_state();
    } catch {
      if (generation === auth_generation) error_message = 'The authenticated browser cache could not be cleared.';
    }
  }

  async function expire_session(): Promise<void> {
    begin_session_change();
    error_message = 'The instance token was rejected or rotated. Every device must reconnect with the current authenticated bootstrap.';
    // Local credentials and views are already invalidated, even if the worker
    // is unavailable. A late cache-clear result must not change a new session.
    try { await clear_authenticated_state(); } catch { /* Keep the expiry notice. */ }
  }

  async function refresh_board(): Promise<void> {
    if (board_request) return;
    const request = begin_request();
    if (!request) return;
    board_request = request;
    busy = true;
    error_message = '';
    try {
      const [task_pages, health_page, console_page] = await Promise.all([
        load_task_pages(request),
        load_health(request.token, request.controller.signal),
        load_console(request.token, request.controller.signal),
      ]);
      if (!request_is_current(request)) return;
      if (!snapshots_match(task_pages, health_page)) {
        task_pages.stale = true;
        health_page.stale = true;
      }
      tasks = task_pages.tasks;
      tasks_page_truncated = task_pages.truncated;
      health = health_page;
      console_data = console_page;
      data_freshness = task_pages;
      if (!selected_id || !tasks.some((task) => task.task_id === selected_id)) {
        selected_id = tasks[0]?.task_id ?? null;
      }
      if (selected_id) await select_task(selected_id);
      else {
        selected_task = null;
        selected_activity = null;
        detail_freshness = null;
      }
    } catch (error) {
      if (!request_is_current(request)) return;
      mark_views_stale();
      show_error(error, request);
    } finally {
      finish_request(request);
      if (board_request === request) {
        board_request = null;
        busy = false;
      }
    }
  }

  /** A failed refresh leaves the last data on screen, but never as current. */
  function mark_views_stale(): void {
    if (console_data) console_data = { ...console_data, stale: true };
    if (data_freshness) data_freshness = { ...data_freshness, stale: true };
    if (detail_freshness) detail_freshness = { ...detail_freshness, stale: true };
    if (health) health = { ...health, stale: true };
  }

  async function load_task_pages(request: request_context): Promise<{ tasks: task_status[]; truncated: boolean } & freshness> {
    const collected_tasks: task_status[] = [];
    let page = await load_tasks(request.token, null, request.controller.signal);
    request.controller.signal.throwIfAborted();
    const first_snapshot = { generation: page.generation, observed_at: page.observed_at };
    let pages_loaded = 0;
    let is_stale = page.stale;
    let is_truncated = page.truncated;
    let next_cursor = page.next_cursor;
    const seen_cursors = new Set<string>();
    collected_tasks.push(...page.tasks);

    while (next_cursor && pages_loaded < 7 && !seen_cursors.has(next_cursor)) {
      seen_cursors.add(next_cursor);
      page = await load_tasks(request.token, next_cursor, request.controller.signal);
      request.controller.signal.throwIfAborted();
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
    const request = begin_request();
    if (!request) return;
    if (detail_request) finish_request(detail_request);
    detail_request = request;
    selected_id = task_id;
    if (selected_task?.task_id !== task_id) {
      selected_task = null;
      selected_activity = null;
      detail_freshness = null;
    }
    try {
      const response = await load_task(request.token, task_id, request.controller.signal);
      // Request identity also rejects an earlier A after selection A → B → A.
      if (!request_is_current(request) || detail_request !== request) return;
      if (data_freshness && !snapshots_match(data_freshness, response)) response.stale = true;
      selected_task = response.task;
      selected_activity = response.activity ?? null;
      detail_freshness = response;
    } catch (error) {
      if (!request_is_current(request) || detail_request !== request) return;
      if (detail_freshness) detail_freshness = { ...detail_freshness, stale: true };
      show_error(error, request);
    } finally {
      finish_request(request);
      if (detail_request === request) detail_request = null;
    }
  }

  async function enable_push(): Promise<void> {
    const request = begin_request();
    if (!request) return;
    try {
      if (!('serviceWorker' in navigator) || !('PushManager' in window)) {
        throw new Error('Push notifications are not available in this browser.');
      }
      const permission = await Notification.requestPermission();
      if (!request_is_current(request)) return;
      if (permission !== 'granted') throw new Error('Notification permission was not granted.');
      const registration = await navigator.serviceWorker.ready;
      if (!request_is_current(request)) return;
      const { public_key } = await load_public_key(request.token, request.controller.signal);
      if (!request_is_current(request)) return;
      const application_server_key = decode_application_server_key(public_key);
      const subscription = await registration.pushManager.subscribe({
        userVisibleOnly: true,
        applicationServerKey: application_server_key,
      });
      if (!request_is_current(request)) return;
      await save_push_subscription(request.token, subscription, request.controller.signal);
      if (request_is_current(request)) enrollment_notice = 'This browser is subscribed for state notifications.';
    } catch (error) {
      show_error(error, request);
    } finally {
      finish_request(request);
    }
  }

  function begin_qr_request(): request_context | null {
    if (qr_request) return null;
    const request = begin_request();
    if (!request) return null;
    qr_request = request;
    return request;
  }

  async function make_enrollment_qr(): Promise<void> {
    const request = begin_qr_request();
    if (!request) return;
    clear_enrollment_image();
    enrollment_notice = '';
    try {
      const enrollment = await create_enrollment(request.token, request.controller.signal);
      if (!request_is_current(request)) return;
      const svg_blob = new Blob([enrollment.qr_svg], { type: 'image/svg+xml' });
      clear_enrollment_image();
      enrollment_image = URL.createObjectURL(svg_blob);
      enrollment_notice = `One-time token expires in ${enrollment.expires_in_seconds} seconds. A phone still needs an operator-provided authenticated browser bootstrap.`;
    } catch (error) {
      show_error(error, request);
    } finally {
      if (qr_request === request) qr_request = null;
      finish_request(request);
    }
  }

  async function redeem_device_enrollment(): Promise<void> {
    const enrollment_token = enrollment_input.trim();
    if (!token || !enrollment_token || enrollment_request) return;
    // Redemption rotates the bearer. Stop old polls before the server can
    // reject them, and do not start new ones until the exchange settles.
    invalidate_requests();
    const request = begin_request();
    if (!request) return;
    enrollment_request = request;
    clear_daemon_views();
    health = null;
    clear_enrollment_image();
    enrollment_notice = '';
    try {
      const result = await redeem_enrollment(request.token, enrollment_token, request.controller.signal);
      if (!request_is_current(request)) return;
      await install_token(result.instance_token, 'Device enrolled. Instance token rotation signs out every enrolled device.');
    } catch (error) {
      show_error(error, request);
    } finally {
      finish_request(request);
      if (enrollment_request === request) enrollment_request = null;
    }
  }

  function show_error(error: unknown, request: request_context): void {
    if (!request_is_current(request)) return;
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
  <meta name="color-scheme" content="light dark" />
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
        <p class="lead">Enter the instance token of this <span class="mono">vibemux_web</span> process. The page keeps it for this browser tab only.</p>
        <form class="token_form" onsubmit={(event) => { event.preventDefault(); void use_token(); }}>
          <label for="instance_token">Instance bearer token</label>
          <div class="token_controls">
            <input id="instance_token" bind:value={token_input} type="password" autocomplete="off" spellcheck="false" />
            <button class="primary_button" type="submit">Connect</button>
          </div>
        </form>
        <p class="bootstrap_note">
          Every data request sends this token. By default the page, its scripts, and enrollment also require it, so the page loads only through an authenticated browser bootstrap. With <span class="mono">local_page_without_token</span> on, a browser on this computer can load the page itself at the loopback address; the data still needs the token. A QR code cannot bootstrap an ordinary phone browser.
        </p>
      </section>
    {:else}
      <section class="dashboard_intro">
        <div>
          <p class="eyebrow">Project activity</p>
          <h1>Your VibeMux monitor</h1>
          <p class="lead">Read-only status from the local VibeMux daemon. Refreshes every five seconds. Keys 1–5 switch views and R refreshes.</p>
        </div>
        <div class="intro_badges">
          <span class="metric_badge"><strong>{tasks.length}</strong> tasks</span>
          <span class="metric_badge"><strong>{health?.daemon?.store_schema_version ?? '—'}</strong> store schema</span>
        </div>
      </section>

      <View_tabs active={active_view} on_select={(view) => (active_view = view)} />

      {#if is_console_view(active_view) && console_data && !console_is_current(console_data)}
        <div class="stale_banner" role="status">
          <strong>{console_data.stale ? 'These views are not current: the last refresh failed or came from the browser cache.' : connection_summary(console_data.connection)}</strong>
        </div>
      {/if}
      {#if active_view === 'tasks' && (data_freshness?.stale || detail_freshness?.stale || health?.stale)}
        <div class="stale_banner" role="status">
          <strong>Status is stale or unavailable.</strong> Task titles stay hidden until a fresh daemon response arrives.
        </div>
      {/if}
      {#if active_view === 'tasks' && !data_freshness?.stale && tasks_page_truncated}
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

      {#if active_view === 'tasks'}
        <div class="board_grid">
          <Task_board tasks={tasks} selected_id={selected_id} freshness={data_freshness} on_select={(task_id) => void select_task(task_id)} />
          <Task_detail task={selected_task} activity={selected_activity} freshness={detail_freshness} />
        </div>
      {:else if active_view === 'daemon'}
        <Daemon_view console_data={console_data} />
      {:else if active_view === 'sessions'}
        <Sessions_view console_data={console_data} />
      {:else if active_view === 'decisions'}
        <Decisions_view console_data={console_data} />
      {:else}
      <section class="tools_grid" aria-label="Device options">
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
      {/if}
      <p class="updated_at">{shown_observed_at ? `Last observed ${new Date(shown_observed_at).toLocaleString()}` : 'Waiting for the first daemon response'}</p>
    {/if}
  </main>
</div>
