<script>
  import { tick } from "svelte";
  import {
    Copy,
    Pencil,
    ThumbsUp,
    ThumbsDown,
    RotateCcw,
    ArrowDown,
    FileText,
    FileCode2,
  } from "@lucide/svelte";
  import BrandMark from "./brand_mark.svelte";
  import MarkdownView from "./markdown_view.svelte";
  import { app_config } from "../config.js";
  import { extract_artifact } from "../ui_policy.js";
  let {
    conversation,
    generating,
    on_regenerate,
    on_edit,
    on_feedback,
    on_artifact,
    on_configure,
    notify,
    auto_scroll,
  } = $props();
  let visible_count = $state(app_config.page_size),
    editing = $state(null),
    edit_text = $state(""),
    at_bottom = $state(true),
    scroll_node;
  let current_id = "";
  const start = $derived(
    Math.max(0, conversation.messages.length - visible_count),
  );
  $effect(() => {
    const id = conversation.id;
    if (current_id !== id) {
      current_id = id;
      visible_count = app_config.page_size;
      editing = null;
      at_bottom = true;
      tick().then(() => {
        if (scroll_node) scroll_node.scrollTop = scroll_node.scrollHeight;
      });
    }
  });
  $effect(() => {
    const text = conversation.messages.at(-1)?.content;
    const count = conversation.messages.length;
    if (auto_scroll && at_bottom) {
      const handle = requestAnimationFrame(() => {
        if (scroll_node) scroll_node.scrollTop = scroll_node.scrollHeight;
      });
      return () => cancelAnimationFrame(handle);
    }
  });
  async function copy(text) {
    try {
      await navigator.clipboard.writeText(text);
      notify("Message copied");
    } catch {
      notify("Clipboard unavailable. Select the message to copy it.");
    }
  }
</script>

<div class="stream-outer">
  <div
    class="message-scroll"
    bind:this={scroll_node}
    onscroll={() =>
      (at_bottom =
        scroll_node.scrollHeight -
          scroll_node.scrollTop -
          scroll_node.clientHeight <
        100)}
  >
    <div class="message-list">
      {#if start > 0}<button
          class="btn load-older"
          onclick={async () => {
            const height = scroll_node.scrollHeight;
            visible_count += app_config.page_size;
            await tick();
            scroll_node.scrollTop = scroll_node.scrollHeight - height;
          }}>Load earlier messages ({start} more)</button
        >{/if}
      <div class="conversation-start">
        <span
          >{new Date(conversation.created_at).toLocaleDateString(undefined, {
            month: "long",
            day: "numeric",
            year: "numeric",
          })}</span
        ><span>{conversation.messages.length} messages</span>
      </div>
      {#each conversation.messages.slice(start) as message, index (message.id)}{@const user =
          message.role === "user"}{@const artifact =
          !user && extract_artifact(message.content || "")}
        <article
          class="message"
          class:user-message={user}
          class:assistant-message={!user}
          aria-label={`${user ? "Your" : "Assistant"} message ${start + index + 1}`}
        >
          <div class="message-avatar">
            {#if user}<span class="avatar small">Y</span>{:else}<BrandMark
              />{/if}
          </div>
          <div class="message-main">
            <div class="message-heading">
              <strong>{user ? "You" : "Claude"}</strong>{#if !user}<span
                  >{(message.model || conversation.model)
                    .replace("claude-", "")
                    .replaceAll("-", " ")}</span
                >{/if}{#if message.status === "streaming"}<span
                  class="streaming-label"
                  >{conversation.thinking
                    ? "Thinking & writing"
                    : "Writing"}<span class="status-dot"></span></span
                >{/if}
            </div>
            {#if editing === message.id}<form
                class="message-edit"
                onsubmit={(e) => {
                  e.preventDefault();
                  if (edit_text.trim() && !generating)
                    on_edit(
                      start + index,
                      edit_text.trim(),
                      () => (editing = null),
                    );
                }}
              >
                <textarea
                  aria-label="Edit message"
                  disabled={generating}
                  bind:value={edit_text}
                  rows="4"
                ></textarea>
                <div>
                  <button
                    type="button"
                    class="btn"
                    onclick={() => (editing = null)}>Cancel</button
                  ><button
                    type="submit"
                    class="btn btn-primary"
                    disabled={!edit_text.trim() || generating}
                    >Save & resend</button
                  >
                </div>
              </form>{:else}{#if message.attachments?.length}<div
                  class="message-attachments"
                >
                  {#each message.attachments as file}<div
                      class="message-attachment"
                    >
                      {#if file.type?.startsWith("image/")}<img
                          src={`data:${file.type};base64,${file.data}`}
                          alt={file.name}
                        />{:else}<FileText />{/if}<span>{file.name}</span>
                    </div>{/each}
                </div>{/if}{#if message.content}<MarkdownView
                  content={message.content}
                  {notify}
                  {on_artifact}
                />{:else if message.status === "streaming"}<div
                  class="thinking-animation"
                  role="status"
                  aria-label="Claude is thinking"
                >
                  <span></span><span></span><span></span><span
                    >{conversation.thinking
                      ? "Taking a little more time to think…"
                      : "A thought is taking shape…"}</span
                  >
                </div>{/if}{#if message.status === "stopped"}<p
                  class="message-status"
                >
                  Response stopped. Regenerate whenever you’re ready.
                </p>{/if}{#if message.error}<div
                  class="inline-error"
                  role="alert"
                >
                  <strong>That didn’t go through.</strong>
                  <p>{message.error}</p>
                  <button
                    class="btn"
                    disabled={generating}
                    onclick={() => on_regenerate(start + index)}
                    ><RotateCcw />Retry response</button
                  ><button class="btn" onclick={on_configure}
                    >API settings</button
                  >
                </div>{/if}{#if artifact && message.status !== "streaming"}<button
                  class="artifact-card"
                  onclick={() => on_artifact(artifact)}
                  ><span><FileCode2 /></span>
                  <div>
                    <strong>{artifact.title}</strong><small
                      >{artifact.language.toUpperCase()} · Click to open</small
                    >
                  </div>
                  <span>↗</span></button
                >{/if}
              <div class="message-actions">
                <button
                  class="icon-btn"
                  aria-label={user ? "Copy your message" : "Copy response"}
                  onclick={() => copy(message.content || "")}><Copy /></button
                >{#if user}<button
                    class="icon-btn"
                    aria-label="Edit message"
                    disabled={generating}
                    onclick={() => {
                      editing = message.id;
                      edit_text = message.content;
                    }}><Pencil /></button
                  >{:else}<button
                    class="icon-btn"
                    aria-label="Good response"
                    aria-pressed={message.feedback === "up"}
                    onclick={() =>
                      on_feedback(
                        message.id,
                        message.feedback === "up" ? null : "up",
                      )}><ThumbsUp /></button
                  ><button
                    class="icon-btn"
                    aria-label="Poor response"
                    aria-pressed={message.feedback === "down"}
                    onclick={() =>
                      on_feedback(
                        message.id,
                        message.feedback === "down" ? null : "down",
                      )}><ThumbsDown /></button
                  ><button
                    class="icon-btn"
                    aria-label="Regenerate response"
                    disabled={generating}
                    onclick={() => on_regenerate(start + index)}
                    ><RotateCcw /></button
                  >{#if message.usage}<span class="usage-hint"
                      >{message.usage.output_tokens} tokens</span
                    >{/if}{/if}
              </div>{/if}
          </div>
        </article>{/each}
    </div>
  </div>
  {#if !at_bottom}<button
      class="jump-bottom btn"
      onclick={() => {
        scroll_node.scrollTo({
          top: scroll_node.scrollHeight,
          behavior: "smooth",
        });
        at_bottom = true;
      }}><ArrowDown />Jump to latest</button
    >{/if}
</div>
