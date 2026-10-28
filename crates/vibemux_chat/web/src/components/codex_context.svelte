<script>
  import {
    Bot,
    Brain,
    Clock3,
    Download,
    FileText,
    FolderOpen,
    KeyRound,
    LayoutDashboard,
    MessageSquare,
    Paperclip,
    Settings2,
    X,
  } from "@lucide/svelte";
  import { models } from "../config.js";
  import { summarize_context } from "../codex_context_policy.js";

  let {
    conversation = null,
    connected = false,
    on_close,
    on_open_artifact,
    on_export,
    on_settings,
    notify,
  } = $props();

  const number_formatter = new Intl.NumberFormat();
  const tab_ids = ["overview", "files"];
  let active_tab = $state("overview");
  let summary = $derived(summarize_context(conversation));
  let model_config = $derived(
    models.find((model) => model.id === summary.model),
  );
  let model_name = $derived(
    model_config?.name ?? summary.model ?? "Not selected",
  );
  let model_id = $derived(model_config ? summary.model : null);
  let has_context = $derived(summary.message_count > 0);

  function format_count(value) {
    return number_formatter.format(value);
  }

  function format_created_at(value) {
    if (!value) return "Not recorded";
    const date = new Date(value);
    return Number.isNaN(date.getTime()) ? String(value) : date.toLocaleString();
  }

  function format_file_size(attachment) {
    const size = attachment?.size ?? attachment?.size_bytes;
    if (!Number.isFinite(size) || size < 0) return null;
    if (size < 1024) return `${size} B`;
    if (size < 1024 * 1024) return `${(size / 1024).toFixed(1)} KB`;
    return `${(size / (1024 * 1024)).toFixed(1)} MB`;
  }

  function attachment_name(attachment) {
    return attachment?.name ?? attachment?.path ?? "Attached file";
  }

  function open_artifact() {
    if (!summary.artifact) return;
    if (on_open_artifact) {
      on_open_artifact(summary.artifact);
    } else {
      notify?.("Artifact preview is unavailable.", "info");
    }
  }

  function open_settings() {
    if (on_settings) {
      on_settings();
    } else {
      notify?.("API settings are unavailable.", "info");
    }
  }

  function export_conversation() {
    if (!has_context) return;
    if (on_export) {
      on_export(conversation);
    } else {
      notify?.("Conversation export is unavailable.", "info");
    }
  }

  function handle_tab_keydown(event) {
    let next_index = tab_ids.indexOf(active_tab);
    if (event.key === "ArrowLeft")
      next_index = (next_index + tab_ids.length - 1) % tab_ids.length;
    else if (event.key === "ArrowRight")
      next_index = (next_index + 1) % tab_ids.length;
    else if (event.key === "Home") next_index = 0;
    else if (event.key === "End") next_index = tab_ids.length - 1;
    else return;

    event.preventDefault();
    active_tab = tab_ids[next_index];
    document.getElementById(`context_${active_tab}_tab`)?.focus();
  }
</script>

<aside class="codex-context" aria-labelledby="codex_context_heading">
  <header class="context-header">
    <div>
      <h2 id="codex_context_heading">Context</h2>
      <span>{connected ? "Connected" : "Not connected"}</span>
    </div>
    <button
      type="button"
      class="icon-btn"
      aria-label="Close context inspector"
      onclick={on_close}
    >
      <X size={17} aria-hidden="true" />
    </button>
  </header>

  <div class="context-tabs" role="tablist" aria-label="Context views">
    <button
      id="context_overview_tab"
      type="button"
      role="tab"
      aria-selected={active_tab === "overview"}
      aria-controls="context_overview_panel"
      tabindex={active_tab === "overview" ? 0 : -1}
      onclick={() => (active_tab = "overview")}
      onkeydown={handle_tab_keydown}
    >
      <LayoutDashboard size={15} aria-hidden="true" />
      Overview
    </button>
    <button
      id="context_files_tab"
      type="button"
      role="tab"
      aria-selected={active_tab === "files"}
      aria-controls="context_files_panel"
      tabindex={active_tab === "files" ? 0 : -1}
      onclick={() => (active_tab = "files")}
      onkeydown={handle_tab_keydown}
    >
      <Paperclip size={15} aria-hidden="true" />
      Files ({summary.attachments.length + (summary.artifact ? 1 : 0)})
    </button>
  </div>

  <div class="context-body">
    {#if active_tab === "overview"}
      <div
        id="context_overview_panel"
        role="tabpanel"
        aria-labelledby="context_overview_tab"
      >
        <section
          class="context-section"
          aria-labelledby="context_activity_heading"
        >
          <h3 id="context_activity_heading">Conversation</h3>
          <div class="context-metric-grid">
            <div class="context-metric">
              <MessageSquare size={15} aria-hidden="true" />
              <strong>{format_count(summary.message_count)}</strong>
              <span>Messages</span>
            </div>
            <div class="context-metric">
              <Download size={15} aria-hidden="true" />
              <strong>{format_count(summary.input_tokens)}</strong>
              <span>Input tokens</span>
            </div>
            <div class="context-metric">
              <Bot size={15} aria-hidden="true" />
              <strong>{format_count(summary.output_tokens)}</strong>
              <span>Output tokens</span>
            </div>
            <div class="context-metric">
              <Paperclip size={15} aria-hidden="true" />
              <strong>{format_count(summary.attachments.length)}</strong>
              <span>Files</span>
            </div>
          </div>
          {#if !has_context}
            <p class="context-empty">
              Start a conversation to see its context.
            </p>
          {/if}
        </section>

        <section
          class="context-section"
          aria-labelledby="context_model_heading"
        >
          <h3 id="context_model_heading">Model &amp; mode</h3>
          <div class="context-row">
            <Bot size={16} aria-hidden="true" />
            <div>
              <strong>{model_name}</strong>
              {#if model_id}<div>{model_id}</div>{/if}
              {#if model_config}<p>{model_config.description}</p>{/if}
            </div>
          </div>
          <div class="context-row">
            <Brain size={16} aria-hidden="true" />
            <span>Thinking</span>
            <strong>{summary.thinking ? "On" : "Off"}</strong>
          </div>
          <div class="context-row">
            <Clock3 size={16} aria-hidden="true" />
            <span>Created</span>
            <time datetime={summary.created_at ?? undefined}
              >{format_created_at(summary.created_at)}</time
            >
          </div>
        </section>
      </div>
    {:else}
      <div
        id="context_files_panel"
        role="tabpanel"
        aria-labelledby="context_files_tab"
      >
        <section
          class="context-section"
          aria-labelledby="context_files_heading"
        >
          <h3 id="context_files_heading">Attachments</h3>
          {#if summary.attachments.length === 0}
            <p class="context-empty">
              No files are attached to this conversation.
            </p>
          {:else}
            {#each summary.attachments as attachment, index (attachment.id ?? attachment.path ?? attachment.name ?? index)}
              <div class="context-file">
                <FileText size={17} aria-hidden="true" />
                <div>
                  <strong>{attachment_name(attachment)}</strong>
                  <span>
                    {format_file_size(attachment) ?? attachment?.type ?? "File"}
                  </span>
                </div>
              </div>
            {/each}
          {/if}
        </section>

        <section
          class="context-section"
          aria-labelledby="context_artifact_heading"
        >
          <h3 id="context_artifact_heading">Artifact</h3>
          {#if summary.artifact}
            <div class="context-file">
              <FileText size={17} aria-hidden="true" />
              <div>
                <strong
                  >{summary.artifact.name ??
                    summary.artifact.title ??
                    "Conversation artifact"}</strong
                >
                <span
                  >{summary.artifact.language?.toUpperCase() ??
                    summary.artifact.type ??
                    summary.artifact.media_type ??
                    "Generated output"}</span
                >
              </div>
            </div>
            <button type="button" class="btn" onclick={open_artifact}>
              <FolderOpen size={15} aria-hidden="true" />
              Open artifact
            </button>
          {:else}
            <p class="context-empty">No artifact has been generated.</p>
          {/if}
        </section>
      </div>
    {/if}

    <section class="context-section" aria-labelledby="context_api_heading">
      <h3 id="context_api_heading">API setup</h3>
      <div class="context-row">
        <KeyRound size={16} aria-hidden="true" />
        <span
          >{connected
            ? "API key configured"
            : "Connect an API key to send messages."}</span
        >
      </div>
      <button type="button" class="btn" onclick={open_settings}>
        <Settings2 size={15} aria-hidden="true" />
        {connected ? "Manage API settings" : "Set up API key"}
      </button>
    </section>
  </div>

  <footer class="context-footer">
    <button
      type="button"
      class="btn"
      disabled={!has_context}
      onclick={export_conversation}
    >
      <Download size={15} aria-hidden="true" />
      Export conversation
    </button>
  </footer>
</aside>
