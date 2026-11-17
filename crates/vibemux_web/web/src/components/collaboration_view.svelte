<script lang="ts">
  import { onMount } from 'svelte';
  import type { console_response, freshness, task_activity, task_status } from '../api_client';
  import { short_id } from '../console_policy';
  import {
    detected_harnesses,
    flow_layers,
    office_desks,
    project_collaboration,
    type agent_state,
    type collaboration_edge_kind,
    type collaboration_node,
    type office_desk,
  } from '../lib/collaboration';
  import Task_detail from './task_detail.svelte';

  let {
    tasks,
    selected_task,
    selected_activity,
    freshness: data_freshness,
    console_data,
    on_select,
  }: {
    tasks: task_status[];
    selected_task: task_status | null;
    selected_activity: task_activity | null;
    freshness: freshness | null;
    console_data: console_response | null;
    on_select: (task_id: string) => void;
  } = $props();

  let active_tab = $state<'flow' | 'office'>('flow');

  const projection = $derived(project_collaboration(tasks, console_data));
  const selected_id = $derived(selected_task?.task_id ?? null);
  const harness_names = $derived(detected_harnesses(console_data));
  const desks = $derived(office_desks(projection.agents, harness_names));
  const node_by_id = $derived(new Map(projection.nodes.map((node) => [node.id, node])));
  const attention_count = $derived(projection.agents.filter((agent) => agent.attention).length);

  // ---- Flow geometry -------------------------------------------------------
  const NODE_WIDTH = 188;
  const NODE_HEIGHT = 62;
  const NODE_GAP = 88;
  const LAYER_GAP = 252;

  type point = { x: number; y: number };

  const layers = $derived(flow_layers(projection));
  const positions = $derived.by(() => {
    const map = new Map<string, point>();
    layers.forEach((ids, layer_index) => {
      ids.forEach((id, index) => map.set(id, { x: 54 + layer_index * LAYER_GAP, y: 44 + index * NODE_GAP }));
    });
    return map;
  });

  const graph_size = $derived.by(() => {
    let width = 700;
    let height = 380;
    for (const node of projection.nodes) {
      const position = positions.get(node.id);
      if (!position) continue;
      width = Math.max(width, position.x + NODE_WIDTH + 54);
      height = Math.max(height, position.y + NODE_HEIGHT + 44);
    }
    return { width, height };
  });

  type edge_shape = { id: string; kind: collaboration_edge_kind; path: string };

  const edge_shapes = $derived.by(() => {
    const layer_of = new Map<string, number>();
    layers.forEach((ids, index) => ids.forEach((id) => layer_of.set(id, index)));
    const shapes: edge_shape[] = [];
    for (const edge of projection.edges) {
      const from = positions.get(edge.from);
      const to = positions.get(edge.to);
      if (!from || !to) continue;
      const from_layer = layer_of.get(edge.from) ?? 0;
      const to_layer = layer_of.get(edge.to) ?? 0;
      let path: string;
      if (from_layer === to_layer) {
        const start_x = from.x + NODE_WIDTH / 2;
        const start_y = from.y + NODE_HEIGHT + 6;
        const end_x = to.x + NODE_WIDTH / 2;
        const end_y = to.y - 6;
        const bend = Math.max(30, Math.abs(end_y - start_y) * 0.45);
        path = `M ${start_x} ${start_y} C ${start_x + bend} ${start_y + bend}, ${end_x + bend} ${end_y - bend}, ${end_x} ${end_y}`;
      } else {
        const start_x = from.x + NODE_WIDTH;
        const start_y = from.y + NODE_HEIGHT / 2;
        const end_x = to.x;
        const end_y = to.y + NODE_HEIGHT / 2;
        const control = Math.max(42, (end_x - start_x) / 2);
        path = `M ${start_x} ${start_y} C ${start_x + control} ${start_y}, ${end_x - control} ${end_y}, ${end_x} ${end_y}`;
      }
      shapes.push({ id: edge.id, kind: edge.kind, path });
    }
    return shapes;
  });

  let zoom = $state(1);
  let pan = $state({ x: 0, y: 0 });
  let dragging = $state(false);
  let drag_origin = { pointer_x: 0, pointer_y: 0, pan_x: 0, pan_y: 0 };

  function zoom_by(factor: number): void {
    zoom = Math.min(2.5, Math.max(0.4, Math.round(zoom * factor * 100) / 100));
  }

  function reset_view(): void {
    zoom = 1;
    pan = { x: 0, y: 0 };
  }

  function on_pointer_down(event: PointerEvent): void {
    if (event.button !== 0) return;
    const target = event.target as Element | null;
    if (target?.closest('[data-node]')) return;
    dragging = true;
    drag_origin = { pointer_x: event.clientX, pointer_y: event.clientY, pan_x: pan.x, pan_y: pan.y };
    (event.currentTarget as SVGSVGElement).setPointerCapture?.(event.pointerId);
  }

  function on_pointer_move(event: PointerEvent): void {
    if (!dragging) return;
    pan = {
      x: drag_origin.pan_x + (event.clientX - drag_origin.pointer_x),
      y: drag_origin.pan_y + (event.clientY - drag_origin.pointer_y),
    };
  }

  function on_pointer_up(event: PointerEvent): void {
    if (!dragging) return;
    dragging = false;
    (event.currentTarget as SVGSVGElement).releasePointerCapture?.(event.pointerId);
  }

  function on_wheel(event: WheelEvent): void {
    if (!event.ctrlKey && !event.metaKey && event.deltaY === 0) return;
    event.preventDefault();
    zoom_by(event.deltaY < 0 ? 1.12 : 1 / 1.12);
  }

  function choose_node(node: collaboration_node): void {
    if (node.task_id) on_select(node.task_id);
  }

  function node_aria_label(node: collaboration_node): string {
    return `${node.kind} ${node.label}, state ${node.state}${node.task_id ? ', open task detail' : ''}`;
  }

  // ---- Office drawing ------------------------------------------------------
  type office_hit = { x: number; y: number; width: number; height: number; task_id: string };

  const DESK_WIDTH = 150;
  const DESK_HEIGHT = 112;
  const DESK_GAP_X = 34;
  const DESK_GAP_Y = 128;
  const DESK_MARGIN = 30;

  let canvas_element = $state<HTMLCanvasElement | null>(null);
  let canvas_width = $state(960);
  let pulse = $state(0);
  let theme_revision = $state(0);
  let reduced_motion = $state(
    typeof window !== 'undefined' && window.matchMedia('(prefers-reduced-motion: reduce)').matches,
  );
  let office_hits: office_hit[] = [];
  let office_logical_size = { width: 960, height: 420 };

  type office_palette = {
    bg: string;
    surface: string;
    surface_alt: string;
    border: string;
    text: string;
    muted: string;
    accent: string;
    accent_tint: string;
    accent_text: string;
    success: string;
    success_tint: string;
    warning: string;
    warning_tint: string;
    danger: string;
    danger_tint: string;
  };

  function read_palette(element: HTMLCanvasElement): office_palette {
    const style = getComputedStyle(element);
    const value = (name: string, fallback: string) => style.getPropertyValue(name).trim() || fallback;
    return {
      bg: value('--bg', '#FAF9F5'),
      surface: value('--surface', '#F5F4ED'),
      surface_alt: value('--surface_alt', '#FFFFFF'),
      border: value('--border', '#E6E3DA'),
      text: value('--text_primary', '#141413'),
      muted: value('--muted_text', '#6B6960'),
      accent: value('--accent', '#C96442'),
      accent_tint: value('--accent_tint', '#F2E0D7'),
      accent_text: value('--accent_text', '#9C5036'),
      success: value('--success', '#3F7A4A'),
      success_tint: value('--success_tint', '#E0E7DD'),
      warning: value('--warning', '#8F6414'),
      warning_tint: value('--warning_tint', '#EBE4D6'),
      danger: value('--danger', '#B03A2E'),
      danger_tint: value('--danger_tint', '#F0DED9'),
    };
  }

  function character_colors(state: agent_state, palette: office_palette): { body: string; tint: string } {
    switch (state) {
      case 'reading': return { body: palette.accent_text, tint: palette.accent_tint };
      case 'typing': return { body: palette.accent, tint: palette.accent_tint };
      case 'running': return { body: palette.success, tint: palette.success_tint };
      case 'waiting': return { body: palette.warning, tint: palette.warning_tint };
      case 'blocked': return { body: palette.warning, tint: palette.warning_tint };
      case 'done': return { body: palette.success, tint: palette.success_tint };
      case 'failed': return { body: palette.danger, tint: palette.danger_tint };
      default: return { body: palette.muted, tint: palette.surface };
    }
  }

  function rounded_rect(ctx: CanvasRenderingContext2D, x: number, y: number, width: number, height: number, radius: number): void {
    const r = Math.min(radius, width / 2, height / 2);
    ctx.beginPath();
    ctx.moveTo(x + r, y);
    ctx.arcTo(x + width, y, x + width, y + height, r);
    ctx.arcTo(x + width, y + height, x, y + height, r);
    ctx.arcTo(x, y + height, x, y, r);
    ctx.arcTo(x, y, x + width, y, r);
    ctx.closePath();
  }

  function draw_character(
    ctx: CanvasRenderingContext2D,
    seat_x: number,
    seat_y: number,
    desk: office_desk,
    palette: office_palette,
    selected_task_id: string | null,
    phase: number,
    still: boolean,
    index: number,
  ): void {
    const agent = desk.agents[index];
    const colors = character_colors(agent.state, palette);
    const bobbing = !still && (agent.state === 'typing' || agent.state === 'running');
    const bob = bobbing && phase % 2 === 0 ? -1 : 0;
    const head_y = seat_y + bob;

    ctx.fillStyle = palette.border;
    ctx.beginPath();
    ctx.ellipse(seat_x + 8, seat_y + 30, 9, 3, 0, 0, Math.PI * 2);
    ctx.fill();

    ctx.fillStyle = colors.tint;
    rounded_rect(ctx, seat_x - 1, head_y + 10, 18, 15, 5);
    ctx.fill();
    ctx.fillStyle = colors.body;
    rounded_rect(ctx, seat_x + 1, head_y + 11, 14, 8, 3);
    ctx.fill();

    ctx.fillStyle = '#E7C39C';
    rounded_rect(ctx, seat_x + 3, head_y, 10, 9, 3);
    ctx.fill();
    ctx.fillStyle = palette.text;
    ctx.fillRect(seat_x + 5, head_y + 3, 2, 2);
    ctx.fillRect(seat_x + 9, head_y + 3, 2, 2);

    if (agent.state === 'typing') {
      const hand_offset = still ? 0 : phase % 2;
      ctx.fillStyle = palette.text;
      ctx.fillRect(seat_x - 3, head_y + 14 + hand_offset, 3, 2);
      ctx.fillRect(seat_x + 16, head_y + 14 + hand_offset, 3, 2);
    }

    if (agent.task_id === selected_task_id) {
      ctx.strokeStyle = palette.accent;
      ctx.lineWidth = 2;
      ctx.beginPath();
      ctx.ellipse(seat_x + 8, seat_y + 34, 13, 4, 0, 0, Math.PI * 2);
      ctx.stroke();
    }

    if (agent.attention) {
      ctx.fillStyle = palette.danger;
      rounded_rect(ctx, seat_x + 11, head_y - 11, 13, 13, 3);
      ctx.fill();
      ctx.fillStyle = '#FFFFFF';
      ctx.font = 'bold 10px system-ui, sans-serif';
      ctx.textAlign = 'center';
      ctx.textBaseline = 'middle';
      ctx.fillText('!', seat_x + 17.5, head_y - 4);
    }

    office_hits.push({ x: seat_x - 7, y: head_y - 13, width: 30, height: 48, task_id: agent.task_id });
  }

  function draw_desk(
    ctx: CanvasRenderingContext2D,
    x: number,
    y: number,
    desk: office_desk,
    palette: office_palette,
    selected_task_id: string | null,
    phase: number,
    still: boolean,
  ): void {
    // Monitor, stand, keyboard.
    ctx.fillStyle = palette.surface;
    ctx.strokeStyle = palette.border;
    ctx.lineWidth = 1;
    rounded_rect(ctx, x + DESK_WIDTH / 2 - 25, y + 4, 50, 28, 3);
    ctx.fill();
    ctx.stroke();
    ctx.fillStyle = palette.accent_tint;
    rounded_rect(ctx, x + DESK_WIDTH / 2 - 20, y + 8, 40, 20, 2);
    ctx.fill();
    ctx.fillStyle = palette.border;
    ctx.fillRect(x + DESK_WIDTH / 2 - 3, y + 32, 6, 6);

    // Desk surface.
    ctx.fillStyle = palette.surface_alt;
    ctx.strokeStyle = palette.border;
    rounded_rect(ctx, x + 6, y + 38, DESK_WIDTH - 12, 46, 6);
    ctx.fill();
    ctx.stroke();
    ctx.fillStyle = palette.surface;
    rounded_rect(ctx, x + DESK_WIDTH / 2 - 16, y + 62, 32, 8, 2);
    ctx.fill();
    ctx.fillStyle = palette.border;
    ctx.fillRect(x + 16, y + 84, 5, 10);
    ctx.fillRect(x + DESK_WIDTH - 21, y + 84, 5, 10);

    // Nameplate.
    ctx.fillStyle = palette.muted;
    ctx.font = '600 11px system-ui, sans-serif';
    ctx.textAlign = 'center';
    ctx.textBaseline = 'middle';
    const label = desk.label.length > 20 ? `${desk.label.slice(0, 19)}…` : desk.label;
    ctx.fillText(label, x + DESK_WIDTH / 2, y + 101);

    const seats: point[] = [
      { x: x + 16, y: y + 44 },
      { x: x + DESK_WIDTH - 34, y: y + 44 },
      { x: x + DESK_WIDTH / 2 - 9, y: y + 68 },
    ];
    const visible = Math.min(desk.agents.length, seats.length);
    for (let index = 0; index < visible; index += 1) {
      draw_character(ctx, seats[index].x, seats[index].y, desk, palette, selected_task_id, phase, still, index);
    }
    if (desk.agents.length > seats.length) {
      ctx.fillStyle = palette.muted;
      ctx.font = '600 10px system-ui, sans-serif';
      ctx.textAlign = 'right';
      ctx.fillText(`+${desk.agents.length - seats.length}`, x + DESK_WIDTH - 8, y + 76);
    }

    if (desk.agents[0]) {
      office_hits.push({ x: x, y: y, width: DESK_WIDTH, height: DESK_HEIGHT, task_id: desk.agents[0].task_id });
    }
  }

  function draw_office(
    canvas: HTMLCanvasElement,
    width: number,
    desk_plan: office_desk[],
    selected_task_id: string | null,
    phase: number,
    still: boolean,
  ): void {
    const palette = read_palette(canvas);
    const dpr = Math.min(window.devicePixelRatio || 1, 2);
    const logical_width = Math.max(320, Math.floor(width));
    const columns = Math.max(
      1,
      Math.min(4, Math.floor((logical_width - DESK_MARGIN * 2 + DESK_GAP_X) / (DESK_WIDTH + DESK_GAP_X))),
    );
    const rows = Math.max(1, Math.ceil(desk_plan.length / columns));
    const logical_height = 40 + rows * (DESK_HEIGHT + DESK_GAP_Y) + 16;

    canvas.width = Math.round(logical_width * dpr);
    canvas.height = Math.round(logical_height * dpr);
    canvas.style.width = '100%';
    canvas.style.height = `${logical_height}px`;
    office_logical_size = { width: logical_width, height: logical_height };

    const ctx = canvas.getContext('2d');
    if (!ctx) return;
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    office_hits = [];

    // Checkerboard floor.
    ctx.fillStyle = palette.bg;
    ctx.fillRect(0, 0, logical_width, logical_height);
    const tile = 26;
    ctx.fillStyle = palette.surface;
    for (let tile_y = 0; tile_y < logical_height; tile_y += tile) {
      for (let tile_x = 0; tile_x < logical_width; tile_x += tile) {
        if ((tile_x / tile + tile_y / tile) % 2 === 0) ctx.fillRect(tile_x, tile_y, tile, tile);
      }
    }

    if (desk_plan.length === 0) {
      ctx.fillStyle = palette.muted;
      ctx.font = '600 13px system-ui, sans-serif';
      ctx.textAlign = 'center';
      ctx.textBaseline = 'middle';
      ctx.fillText('No detected harness rows yet', logical_width / 2, logical_height / 2);
      return;
    }

    desk_plan.forEach((desk, desk_index) => {
      const column = desk_index % columns;
      const row = Math.floor(desk_index / columns);
      const x = DESK_MARGIN + column * (DESK_WIDTH + DESK_GAP_X);
      const y = 30 + row * (DESK_HEIGHT + DESK_GAP_Y);
      draw_desk(ctx, x, y, desk, palette, selected_task_id, phase, still);
    });
  }

  function on_office_click(event: MouseEvent): void {
    const canvas = event.currentTarget as HTMLCanvasElement;
    const rect = canvas.getBoundingClientRect();
    if (rect.width === 0 || rect.height === 0) return;
    const x = (event.clientX - rect.left) * (office_logical_size.width / rect.width);
    const y = (event.clientY - rect.top) * (office_logical_size.height / rect.height);
    const hit = [...office_hits].reverse().find((candidate) =>
      x >= candidate.x && x <= candidate.x + candidate.width && y >= candidate.y && y <= candidate.y + candidate.height,
    );
    if (hit) on_select(hit.task_id);
  }

  $effect(() => {
    if (reduced_motion) return;
    const timer = window.setInterval(() => {
      pulse += 1;
    }, 480);
    return () => window.clearInterval(timer);
  });

  $effect(() => {
    const canvas = canvas_element;
    const width = canvas_width;
    const desk_plan = desks;
    const selected_task_id = selected_id;
    const phase = pulse;
    const still = reduced_motion;
    // Read the theme revision so an OS or explicit theme change redraws.
    void theme_revision;
    if (!canvas) return;
    draw_office(canvas, width, desk_plan, selected_task_id, phase, still);
  });

  onMount(() => {
    const motion_query = window.matchMedia('(prefers-reduced-motion: reduce)');
    const on_motion = (event: MediaQueryListEvent) => {
      reduced_motion = event.matches;
    };
    motion_query.addEventListener('change', on_motion);

    const scheme_query = window.matchMedia('(prefers-color-scheme: dark)');
    const on_scheme = () => {
      theme_revision += 1;
    };
    scheme_query.addEventListener('change', on_scheme);

    return () => {
      motion_query.removeEventListener('change', on_motion);
      scheme_query.removeEventListener('change', on_scheme);
    };
  });

  const office_aria_label = $derived(
    `Office with ${desks.length} desk${desks.length === 1 ? '' : 's'} and ${projection.agents.length} character${projection.agents.length === 1 ? '' : 's'}` +
      (attention_count > 0 ? `, ${attention_count} needing attention` : ''),
  );
</script>

{#snippet flow_node_body(node: collaboration_node)}
  <title>{node_aria_label(node)}</title>
  <rect class="flow_node_box" width={NODE_WIDTH} height={NODE_HEIGHT} rx="13" />
  <text class="flow_node_kind" x="15" y="21">{node.kind}</text>
  <text class="flow_node_state" x={NODE_WIDTH - 15} y="21" text-anchor="end">{node.state}</text>
  <text class="flow_node_label" x="15" y="43">{node.label}</text>
{/snippet}

<div class="collaborate_tabs" role="tablist" aria-label="Collaboration views">
  <button
    type="button"
    role="tab"
    id="collaborate_flow_tab"
    aria-controls="collaborate_flow_panel"
    aria-selected={active_tab === 'flow'}
    class:active={active_tab === 'flow'}
    class="collaborate_tab"
    onclick={() => (active_tab = 'flow')}
  >
    Flow
  </button>
  <button
    type="button"
    role="tab"
    id="collaborate_office_tab"
    aria-controls="collaborate_office_panel"
    aria-selected={active_tab === 'office'}
    class:active={active_tab === 'office'}
    class="collaborate_tab"
    onclick={() => (active_tab = 'office')}
  >
    Office
    {#if attention_count > 0}<span class="attention_badge" aria-label={`${attention_count} needing attention`}>{attention_count}</span>{/if}
  </button>
</div>

{#if active_tab === 'flow'}
  <div id="collaborate_flow_panel" role="tabpanel" aria-labelledby="collaborate_flow_tab">
    <section class="panel collaborate_panel" aria-labelledby="collaborate_flow_heading">
      <div class="panel_heading collaborate_heading">
        <div>
          <p class="eyebrow">Task · run · harness</p>
          <h2 id="collaborate_flow_heading">Flow</h2>
        </div>
        <div class="flow_toolbar" role="group" aria-label="Graph controls">
          <button class="small_button" type="button" onclick={() => zoom_by(1.15)} aria-label="Zoom in">＋</button>
          <span class="flow_zoom" aria-live="polite">{Math.round(zoom * 100)}%</span>
          <button class="small_button" type="button" onclick={() => zoom_by(1 / 1.15)} aria-label="Zoom out">－</button>
          <button class="small_button" type="button" onclick={reset_view}>Reset</button>
        </div>
      </div>

      {#if projection.empty}
        <p class="empty_state">No tasks are available from the local daemon, so there is nothing to plot yet.</p>
      {:else}
        <div class="flow_stage">
          <svg
            class="flow_canvas"
            viewBox={`0 0 ${graph_size.width} ${graph_size.height}`}
            preserveAspectRatio="xMidYMid meet"
            role="img"
            aria-label={`Collaboration graph with ${projection.nodes.length} nodes and ${projection.edges.length} edges. Drag to pan; use the zoom buttons to zoom.`}
            onpointerdown={on_pointer_down}
            onpointermove={on_pointer_move}
            onpointerup={on_pointer_up}
            onpointercancel={on_pointer_up}
            onwheel={on_wheel}
          >
            <defs>
              <marker id="flow_arrow" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse">
                <path d="M 0 0 L 10 5 L 0 10 z" fill="var(--muted_text)" />
              </marker>
              <marker id="flow_arrow_danger" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse">
                <path d="M 0 0 L 10 5 L 0 10 z" fill="var(--danger)" />
              </marker>
            </defs>
            <g class="flow_viewport" transform={`translate(${pan.x} ${pan.y}) scale(${zoom})`}>
              <g class="flow_edges" aria-hidden="true">
                {#each edge_shapes as edge (edge.id)}
                  <path
                    class="flow_edge"
                    data-kind={edge.kind}
                    d={edge.path}
                    marker-end={`url(#flow_arrow${edge.kind === 'blocked_by' ? '_danger' : ''})`}
                  />
                {/each}
              </g>
              <g class="flow_nodes">
                {#each projection.nodes as node (node.id)}
                  {@const position = positions.get(node.id)}
                  {#if position}
                    {#if node.task_id}
                      <g
                        class="flow_node"
                        class:selected={node.task_id === selected_id}
                        data-kind={node.kind}
                        data-node={node.id}
                        transform={`translate(${position.x} ${position.y})`}
                        role="button"
                        tabindex="0"
                        aria-label={node_aria_label(node)}
                        onclick={() => choose_node(node)}
                        onkeydown={(event) => {
                          if (event.key === 'Enter' || event.key === ' ') {
                            event.preventDefault();
                            choose_node(node);
                          }
                        }}
                      >
                        {@render flow_node_body(node)}
                      </g>
                    {:else}
                      <g
                        class="flow_node"
                        data-kind={node.kind}
                        data-node={node.id}
                        transform={`translate(${position.x} ${position.y})`}
                        role="group"
                        aria-label={node_aria_label(node)}
                      >
                        {@render flow_node_body(node)}
                      </g>
                    {/if}
                  {/if}
                {/each}
              </g>
            </g>
          </svg>
        </div>

        <div class="flow_legend" aria-label="Edge legend">
          <span data-kind="spawned">spawned</span>
          <span data-kind="executed_by">executed by</span>
          <span data-kind="isolated_in">isolated in</span>
          <span data-kind="handed_off">handed off</span>
          <span data-kind="blocked_by">blocked by</span>
        </div>

        <details class="flow_list">
          <summary>Node and edge list</summary>
          <h3 class="subheading">Nodes</h3>
          <ul class="collaborate_list">
            {#each projection.nodes as node (node.id)}
              <li>
                <span class="mono">{node.label}</span>
                <span class="collaborate_meta">{node.kind} · {node.state}</span>
              </li>
            {/each}
          </ul>
          <h3 class="subheading">Edges</h3>
          <ul class="collaborate_list">
            {#each projection.edges as edge (edge.id)}
              <li>
                <span class="mono">{edge.kind}</span>
                <span class="collaborate_meta">
                  {node_by_id.get(edge.from)?.label ?? edge.from} → {node_by_id.get(edge.to)?.label ?? edge.to}
                </span>
              </li>
            {/each}
          </ul>
        </details>
      {/if}
    </section>
  </div>
{:else}
  <div id="collaborate_office_panel" role="tabpanel" aria-labelledby="collaborate_office_tab">
    <section class="panel collaborate_panel" aria-labelledby="collaborate_office_heading">
      <div class="panel_heading collaborate_heading">
        <div>
          <p class="eyebrow">Detected harnesses</p>
          <h2 id="collaborate_office_heading">Office</h2>
        </div>
        <div class="office_summary">
          <span class="metric_badge"><strong>{desks.length}</strong> desks</span>
          <span class="metric_badge"><strong>{projection.agents.length}</strong> agents</span>
          {#if attention_count > 0}<span class="metric_badge attention_metric"><strong>{attention_count}</strong> need you</span>{/if}
        </div>
      </div>

      {#if projection.empty}
        <p class="empty_state">No active tasks or runs, so every desk is empty.</p>
      {/if}
      {#if console_data && (console_data.stale || console_data.connection.state !== 'connected')}
        <p class="list_note">Harness desks may be stale: the last console poll did not reach the daemon.</p>
      {/if}

      <div class="office_canvas_wrap" bind:clientWidth={canvas_width}>
        <div class="office_canvas_frame" role="img" aria-label={office_aria_label}>
          <canvas bind:this={canvas_element} class="office_canvas" aria-hidden="true" onclick={on_office_click}></canvas>
        </div>
      </div>
      <p class="list_note">Drawn from detected harness rows and the latest task and run snapshot. Select a character or desk to open the existing task detail below.</p>

      {#if projection.agents.length > 0}
        <div class="table_scroll">
          <table class="console_table office_table">
            <caption class="office_table_caption">Agents in this office</caption>
            <thead>
              <tr>
                <th scope="col">Agent</th>
                <th scope="col">Harness</th>
                <th scope="col">Task</th>
                <th scope="col">Run</th>
                <th scope="col">State</th>
                <th scope="col">Needs you</th>
                <th scope="col"><span class="visually_hidden">Select</span></th>
              </tr>
            </thead>
            <tbody>
              {#each projection.agents as agent (agent.id)}
                <tr class:attention_row={agent.attention}>
                  <td class="mono">{agent.label}</td>
                  <td>{agent.harness ?? 'Unassigned'}</td>
                  <td class="mono">{short_id(agent.task_id)}</td>
                  <td class="mono">{agent.run_id ? short_id(agent.run_id) : '—'}</td>
                  <td><span class="agent_state_pill" data-agent-state={agent.state}>{agent.state}</span></td>
                  <td>{agent.attention ? 'Yes' : '—'}</td>
                  <td><button class="small_button" type="button" onclick={() => on_select(agent.task_id)}>Open task</button></td>
                </tr>
              {/each}
            </tbody>
          </table>
        </div>
      {/if}
    </section>
  </div>
{/if}

{#if selected_task}
  <div class="collaborate_detail">
    <Task_detail task={selected_task} activity={selected_activity} freshness={data_freshness} />
  </div>
{/if}
