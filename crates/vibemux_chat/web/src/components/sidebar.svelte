<script>
  import {
    PanelLeftClose,
    Plus,
    Search,
    Pin,
    MoreHorizontal,
    Pencil,
    Copy,
    Trash2,
    Download,
    Settings2,
    ArrowUpRight,
    FileCode2,
    X,
    ChevronDown,
    MessageSquare,
    SquareTerminal,
    FolderOpen,
  } from "@lucide/svelte";
  import BrandMark from "./brand_mark.svelte";
  import { group_conversations } from "../ui_policy.js";
  let {
    conversations,
    interface_style = "claude",
    active_id,
    collapsed,
    mobile,
    on_toggle,
    on_new,
    on_select,
    on_search,
    on_action,
    on_settings,
    on_account,
    on_library,
    connected,
  } = $props();
  let workspace_expanded = $state(true);
  let menu_id = $state(null),
    node;
  function drawer_key(event) {
    if (!mobile || collapsed) return;
    if (event.key === "Escape") {
      on_toggle();
      event.stopPropagation();
    }
    if (event.key === "Tab") {
      const buttons = [...node.querySelectorAll("button,a,summary")].filter(
        (el) => el.offsetParent !== null,
      );
      const first = buttons[0],
        last = buttons.at(-1);
      if (event.shiftKey && document.activeElement === first) {
        event.preventDefault();
        last?.focus();
      } else if (!event.shiftKey && document.activeElement === last) {
        event.preventDefault();
        first?.focus();
      }
    }
  }
  $effect(() => {
    if (mobile && !collapsed && node) {
      const timer = setTimeout(() => node.querySelector("button")?.focus(), 0);
      return () => clearTimeout(timer);
    }
  });
</script>

<button
  class="drawer-backdrop"
  class:visible={!collapsed}
  aria-label="Close sidebar backdrop"
  onclick={on_toggle}
  tabindex="-1"
></button>
<aside
  bind:this={node}
  class="sidebar"
  class:collapsed
  aria-label="Conversation sidebar"
  aria-modal={mobile && !collapsed ? true : undefined}
  role={mobile && !collapsed ? "dialog" : undefined}
  inert={collapsed}
  onkeydown={drawer_key}
>
  <div class="sidebar-brand">
    <button class="wordmark" aria-label="VibeMux home" onclick={on_new}
      >{#if interface_style === "codex"}<SquareTerminal />{:else}<BrandMark
        />{/if}<span>VibeMux</span>{#if interface_style === "codex"}<small
          class="codex-wordmark-label">Code</small
        >{/if}</button
    ><button
      class="icon-btn"
      aria-label={mobile ? "Close sidebar" : "Collapse sidebar"}
      onclick={on_toggle}
      >{#if mobile}<X />{:else}<PanelLeftClose />{/if}</button
    >
  </div>
  <div class="sidebar-nav">
    <button class="new-chat" onclick={on_new}
      ><span class="new-plus"><Plus /></span>New chat<kbd>Alt N</kbd></button
    ><button class="nav-btn" onclick={on_search}
      ><Search />Search conversations<kbd>Alt K</kbd></button
    ><button class="nav-btn" onclick={on_library}
      ><FileCode2 />Your artifacts<span class="subtle-count"
        >{conversations.filter((c) => c.artifact).length}</span
      ></button
    >
  </div>
  <div class="history">
    {#if interface_style === "codex"}<div class="history-label">WORKSPACE</div>
      <button
        class="codex-project"
        aria-label="Toggle workspace conversations"
        aria-expanded={workspace_expanded}
        onclick={() => (workspace_expanded = !workspace_expanded)}
        ><FolderOpen /><span
          ><strong>Personal workspace</strong><small
            >Local conversation history</small
          ></span
        ><ChevronDown
          class={workspace_expanded ? "" : "project-closed"}
        /></button
      >{:else}<div class="history-label">YOUR CONVERSATIONS</div>{/if}
    {#if interface_style !== "codex" || workspace_expanded}
      {#each group_conversations(conversations) as [name, items]}<section
          class="history-group"
        >
          <h2>
            {#if name === "Pinned"}<Pin />{/if}{name}
          </h2>
          {#each items as chat (chat.id)}<div
              class="history-row"
              class:selected={active_id === chat.id}
            >
              <button
                class="conversation-link"
                aria-current={active_id === chat.id ? "page" : undefined}
                title={chat.title}
                onclick={() => {
                  menu_id = null;
                  on_select(chat.id);
                }}><span>{chat.title}</span></button
              >
              <div class="conversation-menu-anchor">
                <button
                  class="icon-btn history-more"
                  aria-label={`Actions for ${chat.title}`}
                  aria-expanded={menu_id === chat.id}
                  onclick={() =>
                    (menu_id = menu_id === chat.id ? null : chat.id)}
                  ><MoreHorizontal /></button
                >{#if menu_id === chat.id}<button
                    class="menu-dismiss"
                    aria-label="Dismiss conversation menu"
                    tabindex="-1"
                    onclick={() => (menu_id = null)}
                  ></button>
                  <div class="popover history-menu">
                    {#each [[chat.pinned ? "Unpin" : "Pin", Pin, "pin"], ["Rename", Pencil, "rename"], ["Duplicate", Copy, "duplicate"], ["Export Markdown", Download, "export"], ["Delete", Trash2, "delete"]] as [label, Icon, action]}<button
                        class:danger-text={action === "delete"}
                        onclick={() => {
                          menu_id = null;
                          on_action(action, chat);
                        }}><Icon />{label}</button
                      >{/each}
                  </div>{/if}
              </div>
            </div>{/each}
        </section>{/each}{#if !conversations.length}<div class="history-empty">
          <MessageSquare />
          <p>A little space for your next idea.</p>
          <span>Your conversations will appear here.</span>
        </div>{/if}
    {/if}
  </div>
  <div class="sidebar-bottom">
    <button class="connection-card" onclick={() => on_settings("api")}
      ><div>
        <span class="status-dot" class:connected></span><strong
          >{connected ? "API connected" : "Make it your own"}</strong
        ><ArrowUpRight />
      </div>
      <span
        >{connected
          ? "Your models, your workspace"
          : "Connect Claude with your API key"}</span
      ></button
    >
    <div class="account-row">
      <button class="account-button" onclick={on_account}
        ><span class="avatar">Y</span><span
          ><strong>Your workspace</strong><small
            >Personal · {connected ? "Connected" : "Local history"}</small
          ></span
        ><ChevronDown /></button
      ><button
        class="icon-btn"
        aria-label="Open settings"
        onclick={() => on_settings("appearance")}><Settings2 /></button
      >
    </div>
  </div>
</aside>
