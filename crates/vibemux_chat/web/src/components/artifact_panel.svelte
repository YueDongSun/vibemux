<script>
  import {
    Code2,
    Eye,
    Download,
    Maximize2,
    Minimize2,
    X,
    Copy,
    FileCode2,
  } from "@lucide/svelte";
  import { download_file } from "../services/chat_store.js";
  import MarkdownView from "./markdown_view.svelte";
  let { artifact, on_close, notify } = $props();
  let view = $state("code"),
    fullscreen = $state(false),
    panel;
  $effect(() => {
    view = ["html", "svg", "md", "markdown"].includes(artifact.language)
      ? "preview"
      : "code";
  });
  const previewable = $derived(
    ["html", "svg", "md", "markdown"].includes(artifact.language),
  );
  const document = $derived(["md", "markdown"].includes(artifact.language));
  const preview = $derived(
    `<meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src 'unsafe-inline'; script-src 'unsafe-inline'; img-src data: blob:; font-src data:; connect-src 'none'; form-action 'none'; base-uri 'none'">${artifact.content}`,
  );
  function resize_start(event) {
    event.preventDefault();
    const start_x = event.clientX,
      start_width = panel.getBoundingClientRect().width;
    function move(e) {
      panel.style.setProperty(
        "--artifact-width",
        `${start_width + start_x - e.clientX}px`,
      );
    }
    function end() {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", end);
    }
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", end, { once: true });
  }
  async function copy() {
    try {
      await navigator.clipboard.writeText(artifact.content);
      notify("Artifact copied");
    } catch {
      notify("Clipboard unavailable. Open code view to select the content.");
    }
  }
  function download() {
    download_file(
      `artifact.${{ html: "html", svg: "svg", markdown: "md", md: "md", javascript: "js", typescript: "ts", python: "py", css: "css", json: "json" }[artifact.language] || "txt"}`,
      artifact.content,
      "text/plain",
    );
    notify("Artifact downloaded");
  }
</script>

<aside
  bind:this={panel}
  class="artifact-panel"
  class:fullscreen
  aria-label="Artifact panel"
>
  <button
    type="button"
    class="resize-handle"
    aria-label="Resize artifact panel with arrow keys or drag"
    onpointerdown={resize_start}
    onkeydown={(e) => {
      if (e.key === "ArrowLeft" || e.key === "ArrowRight") {
        e.preventDefault();
        panel.style.setProperty(
          "--artifact-width",
          `${panel.getBoundingClientRect().width + (e.key === "ArrowLeft" ? 1 : -1) * parseFloat(getComputedStyle(panel).getPropertyValue("--space-8"))}px`,
        );
      }
    }}
  ></button>
  <header class="artifact-header">
    <FileCode2 />
    <div>
      <strong>{artifact.title}</strong><span
        >{artifact.language.toUpperCase()} · Saved with this chat</span
      >
    </div>
    <button
      class="icon-btn"
      aria-label={fullscreen ? "Exit fullscreen" : "Fullscreen artifact"}
      onclick={() => (fullscreen = !fullscreen)}
      >{#if fullscreen}<Minimize2 />{:else}<Maximize2 />{/if}</button
    ><button class="icon-btn" aria-label="Close artifact" onclick={on_close}
      ><X /></button
    >
  </header>
  <div class="artifact-tools">
    <div class="segmented">
      <button class:active={view === "code"} onclick={() => (view = "code")}
        ><Code2 />Code</button
      ><button
        class:active={view === "preview"}
        disabled={!previewable}
        title={previewable
          ? "Preview artifact"
          : "HTML, SVG and Markdown support preview"}
        onclick={() => (view = "preview")}><Eye />Preview</button
      >
    </div>
    <button class="icon-btn" aria-label="Copy artifact" onclick={copy}
      ><Copy /></button
    ><button class="icon-btn" aria-label="Download artifact" onclick={download}
      ><Download /></button
    >
  </div>
  <div class="artifact-body">
    {#if view === "preview"}{#if document}<div class="document-preview">
          <MarkdownView content={artifact.content} {notify} />
        </div>{:else}<iframe
          title="Sandboxed artifact preview"
          sandbox="allow-scripts"
          srcdoc={preview}
        ></iframe>{/if}{:else}<div class="artifact-code">
        <pre class="raw-artifact-code"><code>{artifact.content}</code></pre>
      </div>{/if}
  </div>
  <footer class="artifact-footer">
    <span class="status-dot"></span>{view === "preview"
      ? "Preview runs in an isolated sandbox"
      : `${artifact.content.split("\n").length} lines · ${artifact.content.length.toLocaleString()} characters`}
  </footer>
</aside>
