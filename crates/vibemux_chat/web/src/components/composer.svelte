<script>
  import {
    Plus,
    ArrowUp,
    Square,
    Brain,
    X,
    FileText,
    ChevronDown,
  } from "@lucide/svelte";
  import { prepare_attachment, uid } from "../services/chat_store.js";
  import { app_config, models } from "../config.js";
  let {
    draft = $bindable(""),
    generating = false,
    on_send,
    on_stop,
    on_model,
    on_thinking,
    model,
    thinking,
    settings,
    notify,
    context_id = "new",
  } = $props();
  let attachments = $state([]),
    file_input,
    textarea,
    dragging = $state(false);
  const reading = $derived(attachments.some((a) => a.progress < 100));
  const can_send = $derived(
    Boolean(draft.trim() || attachments.length) && !generating && !reading,
  );
  $effect(() => {
    draft;
    if (textarea) {
      textarea.style.height = "auto";
      textarea.style.height = `${textarea.scrollHeight}px`;
    }
  });
  $effect(() => {
    context_id;
    attachments = [];
  });
  async function add_files(files) {
    const available = Math.max(
      0,
      app_config.max_attachments - attachments.length,
    );
    const reserved = [...files]
      .slice(0, available)
      .map((file) => ({ id: uid(), file }));
    attachments = [
      ...attachments,
      ...reserved.map(({ id, file }) => ({
        id,
        name: file.name,
        type: file.type,
        progress: 0,
      })),
    ];
    if (files.length > available)
      notify("Attach up to five files per message.");
    for (const { id, file } of reserved) {
      try {
        const ready = await prepare_attachment(file, (progress) => {
          attachments = attachments.map((a) =>
            a.id === id ? { ...a, progress } : a,
          );
        });
        attachments = attachments.map((a) =>
          a.id === id ? { ...ready, id, progress: 100 } : a,
        );
      } catch (error) {
        attachments = attachments.filter((a) => a.id !== id);
        notify(error.message);
      }
    }
  }
  function send() {
    if (!can_send) return;
    const accepted = on_send(draft.trim(), attachments);
    if (accepted === false) return;
    draft = "";
    attachments = [];
  }
  function on_key(event) {
    if (event.isComposing) return;
    if (
      event.key === "Enter" &&
      (event.ctrlKey ||
        event.metaKey ||
        (settings.enter_to_send && !event.shiftKey && !event.altKey))
    ) {
      event.preventDefault();
      send();
    }
  }
</script>

<div
  class:dragging
  class="composer-wrap"
  role="presentation"
  ondragover={(e) => {
    e.preventDefault();
    dragging = true;
  }}
  ondragleave={() => (dragging = false)}
  ondrop={(e) => {
    e.preventDefault();
    dragging = false;
    if (!generating) add_files(e.dataTransfer.files);
  }}
>
  <form
    class="composer"
    onsubmit={(e) => {
      e.preventDefault();
      send();
    }}
  >
    {#if attachments.length}<div class="attachment-list">
        {#each attachments as file (file.id)}<div class="attachment-chip">
            {#if file.type.startsWith("image/") && file.data}<img
                src={`data:${file.type};base64,${file.data}`}
                alt={file.name}
              />{:else}<FileText />{/if}
            <div>
              <strong>{file.name}</strong><span
                >{file.progress < 100
                  ? `Reading ${file.progress}%`
                  : `${Math.max(1, Math.round((file.size || 0) / 1024))} KB · Ready`}</span
              >{#if file.progress < 100}<progress
                  value={file.progress}
                  max="100"
                  aria-label={`Reading ${file.name}`}
                ></progress>{/if}
            </div>
            <button
              type="button"
              class="icon-btn"
              aria-label={`Remove ${file.name}`}
              onclick={() =>
                (attachments = attachments.filter((a) => a.id !== file.id))}
              ><X /></button
            >
          </div>{/each}
      </div>{/if}
    <textarea
      bind:this={textarea}
      bind:value={draft}
      id="message_composer"
      aria-label="Message VibeMux"
      placeholder="How can I help you today?"
      rows="2"
      maxlength={app_config.max_characters}
      disabled={generating}
      onkeydown={on_key}
    ></textarea>
    <div class="composer-toolbar">
      <div class="composer-tools">
        <button
          type="button"
          class="icon-btn"
          aria-label="Attach images or files"
          title="Attach images or files"
          disabled={generating ||
            attachments.length >= app_config.max_attachments}
          onclick={() => file_input.click()}><Plus /></button
        ><input
          bind:this={file_input}
          hidden
          type="file"
          multiple
          aria-label="Upload attachments"
          accept="image/png,image/jpeg,image/gif,image/webp,application/pdf,.txt,.md,.js,.jsx,.ts,.tsx,.html,.css,.json,.csv,.py,.sql,.yaml,.yml"
          onchange={(e) => {
            add_files(e.currentTarget.files);
            e.currentTarget.value = "";
          }}
        /><button
          type="button"
          class="thinking-chip"
          aria-label="Toggle extended thinking"
          class:active={thinking}
          aria-pressed={thinking}
          disabled={generating || model === "claude-haiku-4-5"}
          onclick={() => on_thinking(!thinking)}
          title={model === "claude-haiku-4-5"
            ? "Choose Sonnet or Opus for extended thinking"
            : "Toggle extended thinking"}
          ><Brain /><span
            >{thinking ? "Extended thinking" : "Think it through"}</span
          ></button
        >
      </div>
      <div class="composer-right">
        <label class="composer-model"
          ><span class="sr-only">Composer model</span><select
            value={model}
            disabled={generating}
            onchange={(e) => on_model(e.currentTarget.value)}
            >{#each models as item}<option value={item.id}>{item.name}</option
              >{/each}</select
          ><ChevronDown /></label
        >{#if generating}<button
            class="send-btn stop-btn"
            type="button"
            aria-label="Stop generating"
            title="Stop generating · Escape"
            onclick={on_stop}><Square /></button
          >{:else}<button
            class="send-btn"
            type="submit"
            disabled={!can_send}
            aria-label="Send message"
            title="Send message"><ArrowUp /></button
          >{/if}
      </div>
    </div>
  </form>
  <div class="composer-caption">
    <span
      >{draft.length
        ? `${draft.length.toLocaleString()} / ${app_config.max_characters.toLocaleString()} characters`
        : "VibeMux can make mistakes. Check important details."}</span
    ><span
      >{reading
        ? "Preparing attachments…"
        : settings.enter_to_send
          ? "↵ to send · Shift ↵ for a new line"
          : "Ctrl / ⌘ ↵ to send"}</span
    >
  </div>
</div>
