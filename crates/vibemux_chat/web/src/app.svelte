<script>
  import { onMount, untrack } from "svelte";
  import {
    PanelLeft,
    PanelRight,
    SquareTerminal,
    FolderOpen,
    ChevronDown,
    Check,
    Sun,
    Moon,
    ArrowUpRight,
    Brain,
    Sparkles,
    PenLine,
    Code2,
    Compass,
    ListChecks,
    X,
    Search,
    MessageSquare,
    Download,
    Keyboard,
    WifiOff,
    FileCode2,
    Settings2,
    ShieldCheck,
    ArrowRight,
  } from "@lucide/svelte";
  import BrandMark from "./components/brand_mark.svelte";
  import Modal from "./components/modal.svelte";
  import Sidebar from "./components/sidebar.svelte";
  import InterfaceSwitcher from "./components/interface_switcher.svelte";
  import CodexContext from "./components/codex_context.svelte";
  import { resolve_interface, interface_url } from "./interface_policy.js";
  import Composer from "./components/composer.svelte";
  import MessageStream from "./components/message_stream.svelte";
  import ArtifactPanel from "./components/artifact_panel.svelte";
  import SettingsPanel from "./components/settings_panel.svelte";
  import { models, app_config, shortcuts } from "./config.js";
  import {
    create_conversation,
    uid,
    export_conversation,
    export_all,
  } from "./services/chat_store.js";
  import { create_workspace_autosave } from "./services/workspace_autosave.js";
  import { require_workspace_host, native_unavailable } from "./services/workspace_host.js";
  import {
    group_conversations,
    extract_artifact,
    stopped_snapshot,
  } from "./ui_policy.js";

  let { host } = $props();
  const workspace_host = require_workspace_host(untrack(() => host));
  const native = workspace_host.kind === "native";
  const capabilities = workspace_host.capabilities;
  const { stream_response, validate_api_key, download_file } = workspace_host;
  const storage_session = workspace_host.storage_session;
  const initial = storage_session.initial;
  let native_preferences = $state(workspace_host.preferences?.state || null);
  let interface_style = $state(
    resolve_interface(location.search, initial.settings.interface_style),
  );
  const is_codex = $derived(interface_style === "codex");
  let context_open = $state(true),
    context_drawer = $state(false);
  let draft_context = $state(uid());
  const claude_prompts = [
    {
      icon: PenLine,
      label: "Write something",
      description: "Find the right words, together.",
      prompt: "Help me write a thoughtful introduction for a creative project.",
    },
    {
      icon: Code2,
      label: "Build an idea",
      description: "Turn a little spark into something real.",
      prompt:
        "Build a beautiful HTML landing page for a creative studio. Return a complete standalone HTML document in a fenced html code block.",
    },
    {
      icon: Compass,
      label: "Explore a question",
      description: "Follow your curiosity a little further.",
      prompt: "Explain a useful framework for thinking about a complex idea.",
    },
    {
      icon: ListChecks,
      label: "Make a plan",
      description: "A clearer path from here to there.",
      prompt: "Help me make a focused and realistic plan for the week.",
    },
  ];
  const codex_prompts = [
    {
      icon: Code2,
      label: "Build an interface",
      description: "Turn an idea into working HTML.",
      prompt:
        "Build a complete self-contained HTML page for a thoughtful creative studio. Include responsive CSS and a working interactive button in one fenced html block.",
    },
    {
      icon: PenLine,
      label: "Review some code",
      description: "Find a clearer way forward.",
      prompt:
        "Review this JavaScript and explain one practical improvement: function total(items) { let result = 0; for (let i = 0; i < items.length; i++) result += items[i]; return result; }",
    },
    {
      icon: Compass,
      label: "Understand a concept",
      description: "Make the unfamiliar click.",
      prompt:
        "Explain the JavaScript event loop with one small example and a clear execution timeline.",
    },
    {
      icon: ListChecks,
      label: "Plan a feature",
      description: "Start small. Ship something useful.",
      prompt:
        "Help me plan a small web feature I can ship in one day. Break it into clear implementation and verification steps.",
    },
  ];
  const prompt_cards = $derived(is_codex ? codex_prompts : claude_prompts);
  let conversations = $state(initial.conversations),
    settings = $state(native ? { ...initial.settings, ...workspace_host.preferences.state.values } : initial.settings),
    active_id = $state(initial.active_id),
    draft = $state(""),
    api_key = $state(""),
    api_status = $state("Add your API key to start a live conversation."),
    generating = $state(false),
    replacement = $state(null),
    replacement_notice = $state(null),
    collapsed = $state(matchMedia("(max-width: 760px)").matches),
    mobile = $state(matchMedia("(max-width: 760px)").matches),
    modal = $state(null),
    toast = $state(null),
    search = $state(""),
    search_query = $state(""),
    model_menu = $state(false),
    draft_model = $state(initial.settings.default_model),
    draft_thinking = $state(initial.settings.default_thinking),
    rename_value = $state(""),
    online = $state(navigator.onLine),
    storage_error = $state(initial.storage_warning
      ? workspace_host.storage_message(new Error(initial.storage_warning)) : ""),
    storage_ready = $state(!initial.storage_warning),
    resolved_theme = $state("light");
  let controller = null,
    toast_timer = null,
    api_validation = 0;
  function workspace_snapshot() {
    return {
      conversations: $state.snapshot(conversations),
      settings: workspace_host.workspace_settings($state.snapshot(settings)),
      active_id,
    };
  }
  const autosave = create_workspace_autosave({
    session: storage_session,
    snapshot: workspace_snapshot,
    on_error: (error) => (storage_error = workspace_host.storage_message(error)),
    on_saved: () => (storage_error = ""),
  });
  let tablet = $state(innerWidth <= 1180);
  const drawer_mode = $derived(mobile || (Boolean(artifact) && tablet));
  $effect(() => {
    if (artifact && tablet) untrack(() => (collapsed = true));
  });
  const active = $derived(conversations.find((chat) => chat.id === active_id));
  const displayed_chat = $derived(
    replacement && replacement.chat_id === active?.id
      ? { ...active, messages: replacement.messages }
      : active,
  );
  const model = $derived(active?.model || draft_model),
    thinking = $derived(active?.thinking ?? draft_thinking),
    has_messages = $derived(Boolean(active?.messages.length));
  const selected_model = $derived(
    models.find((item) => item.id === model) || models[0],
  );
  const artifact = $derived(active?.artifact_open && active.artifact);
  const usage = $derived(
    active?.messages.reduce(
      (sum, message) =>
        sum +
        (message.usage?.input_tokens || 0) +
        (message.usage?.output_tokens || 0),
      0,
    ) || 0,
  );
  const search_results = $derived(
    group_conversations(
      conversations.filter(
        (chat) =>
          !search_query ||
          chat.title.toLowerCase().includes(search_query) ||
          chat.messages.some((message) =>
            message.content?.toLowerCase().includes(search_query),
          ),
      ),
    ),
  );
  function set_interface(value) {
    interface_style = value;
    settings = { ...settings, interface_style: value };
    history.replaceState(
      history.state,
      "",
      interface_url(location.href, value),
    );
  }
  function change_settings(patch) {
    if (native) {
      if (patch.interface_style) set_interface(patch.interface_style);
      workspace_host.preferences.update(patch);
      return;
    }
    settings = { ...settings, ...patch };
    if (patch.interface_style) set_interface(patch.interface_style);
  }
  function set_collapsed(value) {
    collapsed = value;
    if (native) workspace_host.preferences.update({ sidebar_collapsed: value });
  }
  function toggle_context() {
    if (tablet) context_drawer = true;
    else if (artifact) {
      update_chat(active.id, { artifact_open: false });
      context_open = true;
    } else context_open = !context_open;
  }
  $effect(() => {
    document.documentElement.dataset.interface = interface_style;
    document.title = is_codex
      ? "VibeMux Code — Your coding workspace"
      : "VibeMux Chat — A little more possible";
  });
  $effect(() => {
    if (!tablet || !is_codex) context_drawer = false;
  });
  function notify(message, undo = null) {
    clearTimeout(toast_timer);
    toast = { message, undo };
    toast_timer = setTimeout(() => (toast = null), app_config.toast_duration);
  }
  function update_chat(id, patch) {
    conversations = conversations.map((chat) =>
      chat.id === id
        ? { ...chat, ...(typeof patch === "function" ? patch(chat) : patch) }
        : chat,
    );
  }
  function stop() {
    controller?.abort();
  }
  function focus_composer() {
    document.getElementById("message_composer")?.focus();
  }
  function new_chat() {
    draft_context = uid();
    stop();
    active_id = null;
    draft = "";
    draft_model = settings.default_model;
    draft_thinking = settings.default_thinking;
    if (mobile) collapsed = true;
    setTimeout(focus_composer, 0);
  }
  function select_chat(id) {
    draft_context = uid();
    stop();
    active_id = id;
    draft = "";
    model_menu = false;
    if (mobile) collapsed = true;
  }
  function choose_model(value) {
    if (generating || !capabilities.send) return;
    if (value === "claude-haiku-4-5") {
      if (active) update_chat(active.id, { thinking: false });
      else draft_thinking = false;
    }
    if (active) update_chat(active.id, { model: value });
    else draft_model = value;
    model_menu = false;
  }
  function choose_thinking(value) {
    if (generating || !capabilities.send) return;
    if (value && model === "claude-haiku-4-5") {
      notify("Choose Sonnet or Opus for extended thinking.");
      return;
    }
    if (active) update_chat(active.id, { thinking: value });
    else draft_thinking = value;
  }
  function toggle_theme() {
    change_settings({ theme: resolved_theme === "light" ? "dark" : "light" });
  }
  function open_settings(tab = "appearance") {
    modal = { type: "settings", tab };
  }
  function open_artifact(value) {
    if (
      replacement && replacement.chat_id === active?.id &&
      value !== active.artifact
    ) {
      notify("Wait for the replacement to finish before opening its artifact.");
      return;
    }
    if (active)
      update_chat(active.id, { artifact: value, artifact_open: true });
  }
  async function generate(chat, messages, { replace = false } = {}) {
    if (!capabilities.send) {
      notify(native_unavailable.send);
      return;
    }
    if (controller) return;
    if (replace && !online) {
      notify("You’re offline. Your original conversation is unchanged.");
      return;
    }
    const request = new AbortController();
    controller = request;
    generating = true;
    replacement_notice = null;
    let reply = {
      id: uid(),
      role: "assistant",
      model: chat.model,
      thinking: chat.thinking,
      content: "",
      status: "streaming",
      created_at: new Date().toISOString(),
    };
    const chat_id = chat.id;
    const retained_artifact =
      chat.artifact &&
      messages.some(
        (message) =>
          message.role === "assistant" &&
          message.content?.includes(chat.artifact.content),
      );
    const artifact_patch = retained_artifact
      ? {}
      : { artifact: null, artifact_open: false };
    // Replacement previews never enter saved/exported history. A reload or
    // failed request must still have the complete original conversation.
    if (replace) replacement = { chat_id, messages: [...messages, reply] };
    else {
      update_chat(chat_id, {
        messages: [...messages, reply],
        updated_at: new Date().toISOString(),
        ...artifact_patch,
      });
    }
    const update_reply = (patch) => {
      reply = { ...reply, ...patch };
      if (replace) replacement = { chat_id, messages: [...messages, reply] };
      else {
        update_chat(chat_id, (current) => ({
          messages: current.messages.map((message) =>
            message.id === reply.id ? { ...message, ...reply } : message,
          ),
        }));
      }
    };
    let content = "";
    try {
      // New submissions checkpoint before inference; replacements checkpoint
      // the original history, including page lifecycle flushes while streaming.
      if (storage_ready) await autosave.persist(workspace_snapshot());
      const full = await stream_response({
        messages: messages.filter(
          (message) => message.role === "user" || message.content.trim(),
        ),
        model: chat.model,
        thinking: chat.thinking,
        api_key,
        signal: request.signal,
        on_delta: (chunk) => {
          content += chunk;
          update_reply({ content });
        },
        on_usage: (usage) => update_reply({ usage }),
      });
      request.signal.throwIfAborted();
      if (replace && !full.trim()) {
        throw new Error("The provider returned an empty replacement. Please retry.");
      }
      const new_artifact = extract_artifact(full);
      reply = { ...reply, content: full, status: "complete" };
      update_chat(chat_id, (current) => ({
        messages: replace
          ? [
              ...messages.map((message) => {
                const saved = current.messages.find((item) => item.id === message.id);
                return saved && Object.hasOwn(saved, "feedback")
                  ? { ...message, feedback: saved.feedback }
                  : message;
              }),
              reply,
            ]
          : current.messages.map((message) =>
              message.id === reply.id ? { ...message, ...reply } : message,
            ),
        ...(replace
          ? { updated_at: new Date().toISOString(), ...artifact_patch }
          : {}),
        ...(new_artifact
          ? { artifact: new_artifact, artifact_open: true }
          : {}),
      }));
    } catch (error) {
      const stopped = error.name === "AbortError";
      if (replace) {
        replacement_notice = {
          chat_id,
          error: !stopped,
          message: `${stopped ? "Response stopped." : error.message || "The request failed. Please retry."} Your original conversation was kept.`,
        };
      } else {
        update_reply({
          status: stopped ? "stopped" : "error",
          ...(!stopped
            ? { error: error.message || "The request failed. Please retry." }
            : {}),
        });
      }
    } finally {
      if (controller === request) {
        replacement = null;
        controller = null;
        generating = false;
      }
      if (storage_ready) autosave.flush_now();
    }
  }
  function send_message(content, attachments = [], force_new = false) {
    if (!capabilities.send) {
      notify(native_unavailable.send);
      return false;
    }
    if (controller || (!content.trim() && !attachments.length)) return false;
    if (!online) {
      notify("You’re offline. Your draft is preserved until you reconnect.");
      return false;
    }
    const chat = (!force_new && active) || {
      ...create_conversation(settings),
      model: draft_model,
      thinking: draft_thinking,
    };
    const message = {
      id: uid(),
      role: "user",
      content,
      attachments,
      created_at: new Date().toISOString(),
    };
    if (!active || force_new) {
      chat.title = (content || attachments[0]?.name || "New conversation")
        .replace(/\s+/g, " ")
        .slice(0, 58);
      conversations = [chat, ...conversations];
      active_id = chat.id;
    }
    generate(chat, [...chat.messages, message]);
    return true;
  }
  function regenerate(index) {
    if (!active || controller || !capabilities.send) return;
    const chat = active;
    const context = chat.messages.slice(0, index);
    if (context.at(-1)?.role !== "user") return;
    const action = () => generate(chat, context, { replace: true });
    if (index < chat.messages.length - 1) {
      modal = {
        type: "confirm",
        title: "Regenerate this earlier response?",
        description:
          "A successful replacement will remove this response and all later messages. If it fails or you stop it, your original conversation will be kept.",
        confirm: "Regenerate & replace later messages",
        action,
      };
    } else action();
  }
  function edit_message(index, content, on_saved) {
    if (!active || controller || !capabilities.send) return false;
    const chat = active;
    modal = {
      type: "confirm",
      title: "Resend this message?",
      description:
        "A successful response will replace this message and all later replies. If it fails or you stop it, your original conversation will be kept.",
      confirm: "Save & resend",
      action: () => {
        if (controller) {
          notify("Wait for the current response or stop it first.");
          return;
        }
        generate(
          chat,
          chat.messages
            .slice(0, index + 1)
            .map((m, i) => (i === index ? { ...m, content } : m)),
          { replace: true },
        );
        on_saved?.();
      },
    };
  }
  function chat_action(action, chat) {
    if (action === "export") {
      if (!capabilities.exports) {
        notify(native_unavailable.exports);
        return;
      }
      try {
        download_file(
          (chat.title
            .toLowerCase()
            .replace(/[^a-z0-9]+/g, "_")
            .slice(0, 60) || "conversation") + ".md",
          export_conversation(chat, "md"),
          "text/markdown;charset=utf-8",
        );
        notify("Conversation exported as Markdown");
      } catch (error) {
        notify(error.message);
      }
      return;
    }
    if (action === "rename") {
      rename_value = chat.title;
      modal = { type: "rename", chat };
      return;
    }
    if (action === "delete") {
      modal = {
        type: "confirm",
        title: "Delete this conversation?",
        description: `“${chat.title}” will be removed from this workspace. You can undo this from the notification.`,
        confirm: "Delete conversation",
        danger: true,
        action: () => {
          if (active_id === chat.id) {
            stop();
            active_id = null;
          }
          conversations = conversations.filter((c) => c.id !== chat.id);
          notify(
            "Conversation deleted",
            () =>
              (conversations = [
                stopped_snapshot(chat),
                ...conversations.filter((c) => c.id !== chat.id),
              ]),
          );
        },
      };
      return;
    }
    if (action === "pin") {
      update_chat(chat.id, { pinned: !chat.pinned });
      notify(
        chat.pinned ? "Conversation unpinned" : "Conversation pinned",
        () => update_chat(chat.id, { pinned: chat.pinned }),
      );
      return;
    }
    if (action === "duplicate") {
      const copy = stopped_snapshot(chat);
      copy.id = uid();
      copy.title = `${chat.title} (copy)`;
      copy.created_at = new Date().toISOString();
      copy.updated_at = copy.created_at;
      copy.messages = copy.messages.map((m) => ({
        ...m,
        id: uid(),
      }));
      conversations = [copy, ...conversations];
      select_chat(copy.id);
      notify("Conversation duplicated", () => {
        conversations = conversations.filter((c) => c.id !== copy.id);
        active_id = chat.id;
      });
    }
  }
  function clear_chats() {
    modal = {
      type: "confirm",
      title: "Clear all conversations?",
      description: native
        ? `All ${conversations.length} conversations and attachments will be removed from this WebView. Native export is unavailable. You can undo from the notification.`
        : `All ${conversations.length} conversations and attachments will be removed from this browser. You can export first and undo from the notification.`,
      confirm: "Clear conversations",
      danger: true,
      action: () => {
        const saved = conversations.map(stopped_snapshot);
        stop();
        conversations = [];
        active_id = null;
        notify("All conversations cleared", () => (conversations = saved));
      },
    };
  }
  function export_chats(format) {
    if (!capabilities.exports) {
      notify(native_unavailable.exports);
      return;
    }
    try {
      download_file(
        `vibemux_conversations.${format}`,
        export_all(conversations, format),
        format === "json" ? "application/json" : "text/markdown;charset=utf-8",
      );
      notify(`All chats exported as ${format.toUpperCase()}`);
    } catch (error) {
      notify(error.message);
    }
  }
  async function save_api(value) {
    if (native) throw new Error("Use native provider profiles; raw API keys are not accepted here.");
    const validation = ++api_validation;
    if (!value) {
      api_key = "";
      api_status = "Key removed. Add a key when you’re ready to chat.";
      notify("API key removed from memory");
      return;
    }
    api_status = "Validating your connection…";
    try {
      const result = await validate_api_key(value);
      if (validation !== api_validation) return;
      api_key = value;
      api_status = result.message || "Connected to Anthropic";
      notify("Connected to Anthropic");
    } catch (error) {
      if (validation !== api_validation) return;
      api_status = error.message;
      throw error;
    }
  }
  function recover_storage() {
    modal = {
      type: "confirm",
      title: "Reset unreadable local history?",
      description: native
        ? "Native export is unavailable. Resetting permanently removes the unreadable local history in this WebView. Cancel to keep the original record; native settings and provider profiles are not affected."
        : "Download the original browser record first if you need to recover it. Resetting clears that record so new conversations can be saved.",
      confirm: "Reset local history",
      danger: true,
      action: async () => {
        try {
          await storage_session.acquire();
          storage_session.reset();
          autosave.discard();
          conversations = [];
          active_id = null;
          storage_ready = true;
          storage_error = "";
          notify("Local history reset");
        } catch (error) {
          notify(error.message);
        }
      },
    };
  }
  $effect(() => {
    const snapshot = workspace_snapshot();
    if (storage_ready) autosave.schedule(snapshot);
  });
  $effect(() => {
    const value = search;
    const timer = setTimeout(
      () => (search_query = value.trim().toLowerCase()),
      app_config.search_delay,
    );
    return () => clearTimeout(timer);
  });
  $effect(() => {
    const choice = settings.theme;
    const query = matchMedia("(prefers-color-scheme: dark)");
    const apply = () => {
      resolved_theme =
        choice === "system" ? (query.matches ? "dark" : "light") : choice;
      document.documentElement.dataset.theme = resolved_theme;
    };
    apply();
    query.addEventListener("change", apply);
    return () => query.removeEventListener("change", apply);
  });
  $effect(() => {
    document.documentElement.dataset.font = settings.font_size;
    document.documentElement.dataset.density = settings.density;
    document.documentElement.dataset.reduceMotion = String(native && Boolean(settings.reduce_motion));
  });
  function hotkey(event) {
    if (event.isComposing || event.defaultPrevented) return;
    if (event.key === "Escape") {
      if (generating) {
        event.preventDefault();
        stop();
      } else if (artifact) update_chat(active.id, { artifact_open: false });
      model_menu = false;
      return;
    }
    if (modal || context_drawer) return;
    if ((event.ctrlKey || event.metaKey) && event.key === "Enter") {
      event.preventDefault();
      document.getElementById("message_composer")?.form?.requestSubmit();
      return;
    }
    if (event.altKey && !event.ctrlKey && !event.metaKey) {
      const key = event.key.toLowerCase();
      if (["n", "b", "t", "k", "/"].includes(key)) {
        event.preventDefault();
        if (key === "n") new_chat();
        if (key === "b") set_collapsed(!collapsed);
        if (key === "t") toggle_theme();
        if (key === "k") {
          search = "";
          modal = { type: "search" };
        }
        if (key === "/") focus_composer();
      }
    }
  }
  onMount(() => {
    let sidebar_preference;
    const unsubscribe_preferences = workspace_host.preferences?.subscribe((state) => {
      native_preferences = state;
      settings = { ...settings, ...state.values };
      if (state.loaded && sidebar_preference !== state.values.sidebar_collapsed) {
        sidebar_preference = state.values.sidebar_collapsed;
        if (!mobile) collapsed = sidebar_preference;
      }
    });
    void workspace_host.preferences?.load();
    const flush_preferences = () => { void workspace_host.preferences?.flush(); };
    window.addEventListener("pagehide", flush_preferences);
    autosave.attach(window, document);
    const query = matchMedia("(max-width: 760px)");
    const resize = () => {
      mobile = query.matches;
      if (mobile) collapsed = true;
    };
    const tablet_query = matchMedia("(max-width: 1180px)");
    const tablet_change = () => (tablet = tablet_query.matches);
    tablet_query.addEventListener("change", tablet_change);
    const up = () => (online = true),
      down = () => (online = false);
    query.addEventListener("change", resize);
    window.addEventListener("online", up);
    window.addEventListener("offline", down);
    window.addEventListener("keydown", hotkey);
    return () => {
      clearTimeout(toast_timer);
      controller?.abort();
      autosave.dispose();
      unsubscribe_preferences?.();
      workspace_host.preferences?.dispose();
      window.removeEventListener("pagehide", flush_preferences);
      query.removeEventListener("change", resize);
      tablet_query.removeEventListener("change", tablet_change);
      window.removeEventListener("online", up);
      window.removeEventListener("offline", down);
      window.removeEventListener("keydown", hotkey);
    };
  });
</script>

<div
  class="app"
  class:has-artifact={Boolean(artifact)}
  class:interface-codex={is_codex}
  class:has-context={is_codex && context_open && !tablet && !artifact}
>
  <a class="skip-link" href="#message_composer">Skip to composer</a><Sidebar
    host={workspace_host}
    {interface_style}
    {conversations}
    {active_id}
    {collapsed}
    mobile={drawer_mode}
    on_toggle={() => set_collapsed(!collapsed)}
    on_new={new_chat}
    on_select={select_chat}
    on_search={() => {
      search = "";
      modal = { type: "search" };
    }}
    on_action={chat_action}
    on_settings={open_settings}
    on_account={() => (modal = { type: "account" })}
    on_library={() => (modal = { type: "library" })}
    connected={Boolean(api_key)}
  />
  <main class="workspace" inert={drawer_mode && !collapsed}>
    <header class="topbar">
      <div class="topbar-left">
        {#if collapsed}<button
            class="icon-btn"
            aria-label="Open sidebar"
            onclick={() => set_collapsed(false)}><PanelLeft /></button
          >{/if}
        {#if is_codex}<div class="codex-breadcrumb">
            <FolderOpen /><span>Personal workspace</span><span>/</span><strong
              >{active?.title || "New chat"}</strong
            >
          </div>{/if}
        <div class="model-menu-anchor">
          <button
            class="model-trigger"
            aria-expanded={model_menu}
            disabled={generating || !capabilities.send}
            title={!capabilities.send ? native_unavailable.send : undefined}
            onclick={() => (model_menu = !model_menu)}
            ><span class="model-family">{native ? "Desktop" : "Claude"}</span>
            <strong>{native ? "Chat unavailable" : selected_model.name}</strong><ChevronDown /></button
          >{#if model_menu}<button
              class="menu-dismiss"
              aria-label="Dismiss model menu"
              tabindex="-1"
              onclick={() => (model_menu = false)}
            ></button>
            <div class="popover model-menu">
              <div class="popover-label">CHOOSE YOUR MODEL</div>
              {#each models as item}<button
                  aria-pressed={model === item.id}
                  onclick={() => choose_model(item.id)}
                  ><div>
                    <strong>{item.name}</strong><small>{item.description}</small
                    >
                  </div>
                  {#if model === item.id}<Check />{/if}</button
                >{/each}
              <p>Model access and limits depend on your API account.</p>
            </div>{/if}
        </div>
        <button
          class="thinking-chip topbar-mode"
          class:active={thinking}
          aria-label="Toggle conversation thinking mode"
          aria-pressed={thinking}
          disabled={!capabilities.send || generating || model === "claude-haiku-4-5"}
          title={!capabilities.send ? native_unavailable.send : model === "claude-haiku-4-5"
            ? "Haiku uses normal answers; choose Sonnet or Opus for extended thinking"
            : "Toggle extended thinking"}
          onclick={() => choose_thinking(!thinking)}
          ><Brain /><span>{thinking ? "Extended" : "Normal"}</span></button
        >{#if has_messages}<span class="topbar-chat-title"
            >/ {active.title}</span
          >{/if}
      </div>
      <div class="topbar-right">
        <InterfaceSwitcher value={interface_style} on_select={set_interface} />
        {#if is_codex}<button
            class="icon-btn context-toggle"
            aria-label="Toggle context panel"
            aria-pressed={tablet ? context_drawer : context_open && !artifact}
            title="Conversation context"
            onclick={toggle_context}><PanelRight /></button
          >{/if}
        <span class="workspace-badge"
          ><span class="status-dot"></span>{native
            ? workspace_host.fixture ? "Desktop fixture · Configuration only" : "Desktop · Configuration only"
            : import.meta.env.MODE === "qa" ? "Controlled QA fixture" : "Personal workspace"}</span
        ><button
          class="icon-btn shortcuts-button"
          aria-label="Keyboard shortcuts"
          onclick={() => (modal = { type: "shortcuts" })}><Keyboard /></button
        ><button
          class="icon-btn"
          aria-label={`Switch to ${resolved_theme === "light" ? "dark" : "light"} theme`}
          disabled={native && !native_preferences?.loaded}
          onclick={toggle_theme}
          >{#if resolved_theme === "light"}<Moon />{:else}<Sun />{/if}</button
        >{#if has_messages}<button
            class="icon-btn"
            aria-label="Export conversation"
            disabled={!capabilities.exports}
            title={!capabilities.exports ? native_unavailable.exports : undefined}
            onclick={() => chat_action("export", active)}><Download /></button
          >{/if}
      </div>
    </header>
    {#if native}<div class="native-banner" role="status">
        <span><strong>Local UI and configuration only.</strong> Chat, projects and Ask are unavailable. Attachments, downloads and external links have no native adapter. Drafts stay in this window.</span>
        <button class="text-btn" onclick={() => open_settings("api")}>Provider profiles</button>
      </div>
      {#if native_preferences?.status === "error"}<div class="storage-banner" role="alert">
          <span>{native_preferences.message}</span>
          <button class="text-btn" onclick={() => workspace_host.preferences.retry()}>Retry desktop settings</button>
        </div>{/if}
    {:else if !online}<div class="network-banner" role="status">
        <WifiOff />You’re offline. Reconnect to continue. Your draft is kept
        here.
      </div>{/if}{#if storage_error}<div class="storage-banner" role="alert">
        <span>{storage_error}</span>{#if capabilities.exports}<button
          class="text-btn"
          onclick={() => {
            try {
              download_file(
                "vibemux_history_recovery.json",
                workspace_host.read_original_record(),
                "application/json",
              );
            } catch (error) {
              notify(error.message);
            }
          }}>Download original record</button
        >{/if}{#if storage_ready}{#if capabilities.exports}<button
            class="text-btn"
            onclick={() => export_chats("json")}>Export this tab's chats</button
          >{/if}<button
            class="text-btn"
            onclick={() => autosave.persist(workspace_snapshot())}>Retry saving</button
          >{:else}<button class="text-btn" onclick={recover_storage}
            >Reset local history</button
          >{/if}
      </div>{/if}
    <div class="workspace-content">
      {#if has_messages}<div class="conversation-info">
          <span
            >{#if thinking}<Brain />Extended thinking{:else}<Sparkles />Normal
              answer{/if}</span
          ><span
            >{native ? "Local transcript · Desktop chat unavailable" : usage
              ? `${usage.toLocaleString()} tokens used · Provider limits apply`
              : api_key
                ? "API limits are managed by your Anthropic account"
                : "Connect your API in Settings to receive a response"}</span
          >{#if active.artifact && !active.artifact_open}<button
              class="text-btn"
              onclick={() => update_chat(active.id, { artifact_open: true })}
              ><FileCode2 />Open artifact</button
            >{/if}
        </div>
        {#if replacement?.chat_id === active.id}<div
            class="conversation-info"
            role="status"
          >Generating a replacement. Your original replies remain saved until it finishes.</div
          >{:else if replacement_notice?.chat_id === active.id}<div
            class={replacement_notice.error ? "storage-banner" : "conversation-info"}
            role={replacement_notice.error ? "alert" : "status"}
          >
            <span>{replacement_notice.message}</span>
            {#if replacement_notice.error}<button
                class="text-btn"
                onclick={() => open_settings("api")}>API settings</button
              >{/if}<button
              class="text-btn"
              onclick={() => (replacement_notice = null)}>Dismiss</button
            >
          </div>{/if}
        <MessageStream
          host={workspace_host}
          conversation={displayed_chat}
          replacing={Boolean(replacement)}
          {generating}
          on_regenerate={regenerate}
          on_edit={edit_message}
          on_feedback={(id, feedback) => {
            update_chat(active.id, (c) => ({
              messages: c.messages.map((m) =>
                m.id === id ? { ...m, feedback } : m,
              ),
            }));
            notify(feedback ? "Feedback saved locally" : "Feedback removed");
          }}
          on_artifact={open_artifact}
          on_configure={() => open_settings("api")}
          {notify}
          auto_scroll={settings.auto_scroll}
        />
        <div class="bottom-composer">
          <Composer
            host={workspace_host}
            context_id={(active_id || "new") + ":" + draft_context}
            bind:draft
            {generating}
            on_send={send_message}
            on_stop={stop}
            on_model={choose_model}
            on_thinking={choose_thinking}
            {model}
            {thinking}
            {settings}
            {notify}
          />
        </div>{:else}<div class="welcome-scroll">
          <div class="welcome">
            <div class="welcome-brand">
              {#if is_codex}<span class="codex-welcome-icon"
                  ><SquareTerminal /></span
                >{:else}<BrandMark />{/if}<span
                >{is_codex
                  ? "YOUR CODING WORKSPACE"
                  : "A SPACE FOR YOUR NEXT IDEA"}</span
              >
            </div>
            <h1>
              {is_codex ? "What should we build?" : "A little more possible."}
            </h1>
            <p class="welcome-description">
              {#if is_codex}Start with a question, a bug, or an idea. Let’s work
                through it.{:else}Big questions, small curiosities, and
                everything in between.<br />Let’s see where a conversation can
                take you.{/if}
            </p>
            <Composer
              host={workspace_host}
              context_id={(active_id || "new") + ":" + draft_context}
              bind:draft
              {generating}
              on_send={send_message}
              on_stop={stop}
              on_model={choose_model}
              on_thinking={choose_thinking}
              {model}
              {thinking}
              {settings}
              {notify}
            />
            <div class="prompt-section">
              <div class="prompt-label">
                <span
                  >{is_codex
                    ? "SUGGESTED TASKS"
                    : "A FEW PLACES TO START"}</span
                ><span
                  >{is_codex
                    ? "A useful place to begin"
                    : "Made for your everyday"}</span
                >
              </div>
              <div class="prompt-grid">
                {#each prompt_cards as { icon: Icon, label, description, prompt }}<button
                    class="prompt-card"
                    disabled={generating || !capabilities.send}
                    title={!capabilities.send ? native_unavailable.send : undefined}
                    onclick={() => send_message(prompt)}
                    ><div><Icon /><ArrowUpRight /></div>
                    <strong>{label}</strong><span>{description}</span></button
                  >{/each}
              </div>
            </div>
            <div class="welcome-note">
              <span class="note-line"></span><span
                >{is_codex
                  ? "One conversation. A clearer next step."
                  : "Room to think. Space to create."}</span
              ><span class="note-line"></span>
            </div>
          </div>
          <footer class="welcome-footer">
            <ShieldCheck /><span>Your conversations stay on this device.</span
            ><button class="text-btn" onclick={() => open_settings("data")}
              >You’re in control <ArrowUpRight /></button
            >
          </footer>
        </div>{/if}
    </div>
  </main>
  {#if is_codex && context_open && !tablet && !artifact}<CodexContext
      host={workspace_host}
      conversation={active || null}
      connected={Boolean(api_key)}
      on_close={() => (context_open = false)}
      on_open_artifact={() =>
        active?.artifact && open_artifact(active.artifact)}
      on_export={() => active && chat_action("export", active)}
      on_settings={() => open_settings("api")}
      {notify}
    />{/if}
  {#if artifact}<ArtifactPanel
      host={workspace_host}
      {artifact}
      on_close={() => update_chat(active.id, { artifact_open: false })}
      {notify}
    />{/if}
  <div class="toast-region" aria-live="polite">
    {#if toast}<div class="toast">
        <Check /><span>{toast.message}</span>{#if toast.undo}<button
            onclick={() => {
              const undo = toast.undo;
              toast = null;
              undo();
            }}>Undo</button
          >{/if}<button
          class="icon-btn"
          aria-label="Dismiss notification"
          onclick={() => (toast = null)}><X /></button
        >
      </div>{/if}
  </div>
  {#if modal}<Modal
      title={modal.type === "settings"
        ? "Settings"
        : modal.type === "search"
          ? "Search conversations"
          : modal.title || modal.type}
      on_close={() => (modal = null)}
      class_name={modal.type === "settings"
        ? "settings-dialog"
        : modal.type === "search"
          ? "search-dialog"
          : ""}
    >
      {#if modal.type === "settings"}<SettingsPanel
          host={workspace_host}
          {native_preferences}
          settings={{ ...settings, interface_style }}
          on_change={change_settings}
          on_close={() => (modal = null)}
          {api_key}
          on_api_save={save_api}
          {api_status}
          on_export={export_chats}
          on_clear={clear_chats}
          conversation_count={conversations.length}
          initial_tab={modal.tab}
        />
      {:else if modal.type === "search"}<div class="search-input">
          <Search /><input
            aria-label="Search all conversations"
            placeholder="Search titles and messages…"
            bind:value={search}
          /><button
            class="icon-btn"
            aria-label="Close search"
            onclick={() => (modal = null)}><X /></button
          >
        </div>
        <div class="search-results">
          {#each search_results as [name, items]}<section>
              <h3>{name}</h3>
              {#each items as chat}<button
                  onclick={() => {
                    select_chat(chat.id);
                    modal = null;
                  }}
                  ><MessageSquare />
                  <div>
                    <strong>{chat.title}</strong><span
                      >{(
                        chat.messages.find((m) =>
                          m.content?.toLowerCase().includes(search_query),
                        )?.content || `${chat.messages.length} messages`
                      )
                        .slice(0, 130)
                        .replace(/[#*`\n]/g, " ")}</span
                    >
                  </div>
                  <ArrowUpRight /></button
                >{/each}
            </section>{/each}{#if !search_results.length}<div
              class="modal-empty"
            >
              <Search />
              <h3>
                {search_query
                  ? "No conversations found"
                  : "Your ideas will be easy to find."}
              </h3>
              <p>
                {search_query
                  ? "Try another word from a title or message."
                  : "Start a conversation, then search its title or any message here."}
              </p>
            </div>{/if}
        </div>
        <div class="search-footer">
          Searches every title and message <kbd>Esc to close</kbd>
        </div>
      {:else if modal.type === "rename"}<form
          class="simple-dialog"
          onsubmit={(e) => {
            e.preventDefault();
            if (rename_value.trim()) {
              const chat = modal.chat;
              update_chat(chat.id, { title: rename_value.trim() });
              modal = null;
              notify("Conversation renamed", () =>
                update_chat(chat.id, { title: chat.title }),
              );
            }
          }}
        >
          <div class="dialog-title">
            <h2>Give this chat a name</h2>
            <button
              class="icon-btn"
              type="button"
              aria-label="Close rename dialog"
              onclick={() => (modal = null)}><X /></button
            >
          </div>
          <p>A good name makes your ideas easier to find.</p>
          <label class="field"
            >Conversation name<input
              bind:value={rename_value}
              maxlength="120"
            /></label
          >
          <div class="dialog-actions">
            <button class="btn" type="button" onclick={() => (modal = null)}
              >Cancel</button
            ><button
              class="btn btn-primary"
              type="submit"
              disabled={!rename_value.trim()}>Save name</button
            >
          </div>
        </form>
      {:else if modal.type === "confirm"}<div class="simple-dialog">
          <div class="dialog-title">
            <h2>{modal.title}</h2>
            <button
              class="icon-btn"
              aria-label="Close confirmation"
              onclick={() => (modal = null)}><X /></button
            >
          </div>
          <p>{modal.description}</p>
          <div class="dialog-actions">
            <button class="btn" onclick={() => (modal = null)}>Cancel</button
            ><button
              class="btn"
              class:btn-danger={modal.danger}
              class:btn-primary={!modal.danger}
              onclick={() => {
                const action = modal.action;
                modal = null;
                action();
              }}>{modal.confirm}</button
            >
          </div>
        </div>
      {:else if modal.type === "library"}<div
          class="simple-dialog artifact-library"
        >
          <div class="dialog-title">
            <h2>Your artifacts</h2>
            <button
              class="icon-btn"
              aria-label="Close artifacts library"
              onclick={() => (modal = null)}><X /></button
            >
          </div>
          <p>The things you’ve made, all in one place.</p>
          {#each conversations.filter((c) => c.artifact) as chat}<button
              class="artifact-card"
              onclick={() => {
                select_chat(chat.id);
                update_chat(chat.id, { artifact_open: true });
                modal = null;
              }}
              ><FileCode2 />
              <div>
                <strong>{chat.artifact.title}</strong><small>{chat.title}</small
                >
              </div>
              <ArrowUpRight /></button
            >{/each}{#if !conversations.some((c) => c.artifact)}<div
              class="modal-empty"
            >
              <FileCode2 />
              <h3>{native ? "No local artifacts yet." : "Your next creation starts with a chat."}</h3>
              <p>
                {native ? "Artifact generation is unavailable. Existing local artifacts can be inspected in code view; executable previews and downloads are disabled."
                  : "Ask Claude to build a web page, write code, or draft a document."}
              </p>
              <button
                class="btn btn-primary"
                disabled={!capabilities.send}
                title={!capabilities.send ? native_unavailable.send : undefined}
                onclick={() => {
                  modal = null;
                  send_message(
                    is_codex
                      ? codex_prompts[0].prompt
                      : claude_prompts[1].prompt,
                    [],
                    true,
                  );
                }}>Build a first artifact <ArrowRight /></button
              >
            </div>{/if}
        </div>
      {:else if modal.type === "account"}<div class="simple-dialog">
          <div class="dialog-title">
            <h2>Your personal workspace</h2>
            <button
              class="icon-btn"
              aria-label="Close account details"
              onclick={() => (modal = null)}><X /></button
            >
          </div>
          <div class="account-detail">
            <span class="avatar">Y</span>
            <div>
              <strong>A space that’s yours.</strong>
              <p>
                {conversations.length} conversations · {conversations.filter(
                  (c) => c.artifact,
                ).length} artifacts
              </p>
            </div>
          </div>
          <p>
            {#if native}This is a local UI and configuration workspace. History in this WebView requires a writer lock; unsent drafts are in memory only. Desktop settings and provider profiles use separate native storage. None of these records are canonical Tasks, Runs or Events.
            {:else}This is a local, single-user workspace. Chats and preferences are
            saved in this browser, separate from the VibeMux daemon.{/if}
          </p>
          <button
            class="btn btn-primary"
            onclick={() => open_settings("appearance")}
            ><Settings2 />Manage workspace</button
          >
        </div>
      {:else}<div class="simple-dialog">
          <div class="dialog-title">
            <h2>Keep your flow</h2>
            <button
              class="icon-btn"
              aria-label="Close keyboard shortcuts"
              onclick={() => (modal = null)}><X /></button
            >
          </div>
          <p>A few shortcuts to make this space feel like yours.</p>
          <div class="shortcut-list">
            {#each shortcuts as [label, key]}<div>
                <span>{label}</span><kbd>{key}</kbd>
              </div>{/each}
          </div>
        </div>{/if}
    </Modal>{/if}
  {#if is_codex && context_drawer}<Modal
      title="Conversation context"
      class_name="context-dialog"
      on_close={() => (context_drawer = false)}
      ><CodexContext
        host={workspace_host}
        conversation={active || null}
        connected={Boolean(api_key)}
        on_close={() => (context_drawer = false)}
        on_open_artifact={() => {
          context_drawer = false;
          if (active?.artifact) open_artifact(active.artifact);
        }}
        on_export={() => active && chat_action("export", active)}
        on_settings={() => {
          context_drawer = false;
          open_settings("api");
        }}
        {notify}
      /></Modal
    >{/if}
</div>
