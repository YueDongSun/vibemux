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
    load_workspace,
    save_workspace,
    create_conversation,
    uid,
    export_conversation,
    export_all,
    download_file,
  } from "./services/chat_store.js";
  import { stream_response, validate_api_key } from "./services/provider.js";
  import {
    group_conversations,
    extract_artifact,
    stopped_snapshot,
  } from "./ui_policy.js";

  const initial = load_workspace();
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
    settings = $state(initial.settings),
    active_id = $state(initial.active_id),
    draft = $state(""),
    api_key = $state(""),
    api_status = $state("Add your API key to start a live conversation."),
    generating = $state(false),
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
    storage_error = $state(initial.storage_warning || ""),
    storage_ready = $state(!initial.storage_warning),
    resolved_theme = $state("light");
  let controller = null,
    toast_timer = null,
    api_validation = 0;
  let tablet = $state(innerWidth <= 1180);
  const drawer_mode = $derived(mobile || (Boolean(artifact) && tablet));
  $effect(() => {
    if (artifact && tablet) untrack(() => (collapsed = true));
  });
  const active = $derived(conversations.find((chat) => chat.id === active_id));
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
    settings = { ...settings, ...patch };
    if (patch.interface_style) set_interface(patch.interface_style);
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
    if (generating) return;
    if (value === "claude-haiku-4-5") {
      if (active) update_chat(active.id, { thinking: false });
      else draft_thinking = false;
    }
    if (active) update_chat(active.id, { model: value });
    else draft_model = value;
    model_menu = false;
  }
  function choose_thinking(value) {
    if (generating) return;
    if (value && model === "claude-haiku-4-5") {
      notify("Choose Sonnet or Opus for extended thinking.");
      return;
    }
    if (active) update_chat(active.id, { thinking: value });
    else draft_thinking = value;
  }
  function toggle_theme() {
    settings = {
      ...settings,
      theme: resolved_theme === "light" ? "dark" : "light",
    };
  }
  function open_settings(tab = "appearance") {
    modal = { type: "settings", tab };
  }
  function open_artifact(value) {
    if (active)
      update_chat(active.id, { artifact: value, artifact_open: true });
  }
  async function generate(chat, messages) {
    if (controller) return;
    const request = new AbortController();
    controller = request;
    generating = true;
    const reply = {
      id: uid(),
      role: "assistant",
      model: chat.model,
      thinking: chat.thinking,
      content: "",
      status: "streaming",
      created_at: new Date().toISOString(),
    };
    const chat_id = chat.id;
    update_chat(chat_id, {
      messages: [...messages, reply],
      updated_at: new Date().toISOString(),
      ...(chat.artifact &&
      messages.some(
        (m) =>
          m.role === "assistant" && m.content?.includes(chat.artifact.content),
      )
        ? {}
        : { artifact: null, artifact_open: false }),
    });
    let content = "";
    try {
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
          update_chat(chat_id, (c) => ({
            messages: c.messages.map((m) =>
              m.id === reply.id ? { ...m, content } : m,
            ),
          }));
        },
        on_usage: (usage) =>
          update_chat(chat_id, (c) => ({
            messages: c.messages.map((m) =>
              m.id === reply.id ? { ...m, usage } : m,
            ),
          })),
      });
      const new_artifact = extract_artifact(full);
      update_chat(chat_id, (c) => ({
        messages: c.messages.map((m) =>
          m.id === reply.id ? { ...m, content: full, status: "complete" } : m,
        ),
        ...(new_artifact
          ? { artifact: new_artifact, artifact_open: true }
          : {}),
      }));
    } catch (error) {
      update_chat(chat_id, (c) => ({
        messages: c.messages.map((m) =>
          m.id === reply.id
            ? {
                ...m,
                status: error.name === "AbortError" ? "stopped" : "error",
                ...(error.name === "AbortError"
                  ? {}
                  : {
                      error:
                        error.message || "The request failed. Please retry.",
                    }),
              }
            : m,
        ),
      }));
    } finally {
      if (controller === request) {
        controller = null;
        generating = false;
      }
    }
  }
  function send_message(content, attachments = [], force_new = false) {
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
    if (!active || controller) return;
    const context = active.messages.slice(0, index);
    if (context.at(-1)?.role === "user") generate(active, context);
  }
  function edit_message(index, content, on_saved) {
    if (!active || controller) return false;
    const chat = active;
    modal = {
      type: "confirm",
      title: "Resend this message?",
      description:
        "Claude will respond to the edited message. Replies after it will be replaced.",
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
        );
        on_saved?.();
      },
    };
  }
  function chat_action(action, chat) {
    if (action === "export") {
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
      description: `All ${conversations.length} conversations and attachments will be removed from this browser. You can export first and undo from the notification.`,
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
      description:
        "Download the original browser record first if you need to recover it. Resetting clears that record so new conversations can be saved.",
      confirm: "Reset local history",
      danger: true,
      action: () => {
        try {
          localStorage.removeItem(app_config.storage_key);
          load_workspace();
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
    const snapshot = {
      conversations: $state.snapshot(conversations),
      settings: $state.snapshot(settings),
      active_id,
    };
    if (!storage_ready) return;
    const timer = setTimeout(() => {
      try {
        save_workspace(snapshot);
        storage_error = "";
      } catch (error) {
        storage_error = error.message;
      }
    }, app_config.save_delay);
    return () => clearTimeout(timer);
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
        if (key === "b") collapsed = !collapsed;
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
    {interface_style}
    {conversations}
    {active_id}
    {collapsed}
    mobile={drawer_mode}
    on_toggle={() => (collapsed = !collapsed)}
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
            onclick={() => (collapsed = false)}><PanelLeft /></button
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
            disabled={generating}
            onclick={() => (model_menu = !model_menu)}
            ><span class="model-family">Claude</span>
            <strong>{selected_model.name}</strong><ChevronDown /></button
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
          disabled={generating || model === "claude-haiku-4-5"}
          title={model === "claude-haiku-4-5"
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
          ><span class="status-dot"></span>{import.meta.env.MODE === "qa"
            ? "Controlled QA fixture"
            : "Personal workspace"}</span
        ><button
          class="icon-btn shortcuts-button"
          aria-label="Keyboard shortcuts"
          onclick={() => (modal = { type: "shortcuts" })}><Keyboard /></button
        ><button
          class="icon-btn"
          aria-label={`Switch to ${resolved_theme === "light" ? "dark" : "light"} theme`}
          onclick={toggle_theme}
          >{#if resolved_theme === "light"}<Moon />{:else}<Sun />{/if}</button
        >{#if has_messages}<button
            class="icon-btn"
            aria-label="Export conversation"
            onclick={() => chat_action("export", active)}><Download /></button
          >{/if}
      </div>
    </header>
    {#if !online}<div class="network-banner" role="status">
        <WifiOff />You’re offline. Reconnect to continue. Your draft is kept
        here.
      </div>{/if}{#if storage_error}<div class="storage-banner" role="alert">
        <span>{storage_error}</span><button
          class="text-btn"
          onclick={() => {
            try {
              download_file(
                "vibemux_history_recovery.json",
                localStorage.getItem(app_config.storage_key) || "",
                "application/json",
              );
            } catch (error) {
              notify(error.message);
            }
          }}>Download original record</button
        >{#if !storage_ready}<button class="text-btn" onclick={recover_storage}
            >Reset local history</button
          >{/if}
      </div>{/if}
    <div class="workspace-content">
      {#if has_messages}<div class="conversation-info">
          <span
            >{#if thinking}<Brain />Extended thinking{:else}<Sparkles />Normal
              answer{/if}</span
          ><span
            >{usage
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
        <MessageStream
          conversation={active}
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
                    disabled={generating}
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
              <h3>Your next creation starts with a chat.</h3>
              <p>
                Ask Claude to build a web page, write code, or draft a document.
              </p>
              <button
                class="btn btn-primary"
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
            This is a local, single-user workspace. Chats and preferences are
            saved in this browser, separate from the VibeMux daemon.
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
