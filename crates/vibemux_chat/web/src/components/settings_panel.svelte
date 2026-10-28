<script>
  import {
    X,
    Palette,
    SlidersHorizontal,
    KeyRound,
    Database,
    Keyboard,
    Eye,
    EyeOff,
    ExternalLink,
  } from "@lucide/svelte";
  import { models, shortcuts } from "../config.js";
  import { interface_options } from "../interface_policy.js";
  let {
    settings,
    on_change,
    on_close,
    api_key,
    on_api_save,
    api_status,
    on_export,
    on_clear,
    conversation_count,
    initial_tab = "appearance",
  } = $props();
  const tabs = [
    ["appearance", "Appearance", Palette],
    ["behavior", "Behavior", SlidersHorizontal],
    ["api", "API connection", KeyRound],
    ["data", "Data & privacy", Database],
    ["shortcuts", "Shortcuts", Keyboard],
  ];
  let active_tab = $state("appearance"),
    key_draft = $state(""),
    visible = $state(false),
    saving = $state(false),
    error = $state("");
  $effect(() => {
    active_tab = initial_tab;
  });
  $effect(() => {
    key_draft = api_key;
  });
  async function save(event) {
    event.preventDefault();
    if (saving) return;
    saving = true;
    error = "";
    try {
      await on_api_save(key_draft.trim());
      visible = false;
    } catch (err) {
      error = err.message;
    } finally {
      saving = false;
    }
  }
</script>

<header class="settings-header">
  <h2>Settings</h2>
  <button class="icon-btn" aria-label="Close settings" onclick={on_close}
    ><X /></button
  >
</header>
<div class="settings-layout">
  <nav class="settings-nav" aria-label="Settings categories">
    {#each tabs as [id, label, Icon]}<button
        class:active={active_tab === id}
        aria-current={active_tab === id ? "page" : undefined}
        onclick={() => (active_tab = id)}><Icon />{label}</button
      >{/each}
  </nav>
  <div class="settings-body">
    <section class="settings-section">
      {#if active_tab === "appearance"}<h2>Make yourself at home.</h2>
        <div class="settings-row">
          <label for="interface_style">Interface</label><select
            id="interface_style"
            value={settings.interface_style}
            onchange={(event) =>
              on_change({ interface_style: event.currentTarget.value })}
            >{#each interface_options as option}<option value={option.id}
                >{option.name}</option
              >{/each}</select
          >
        </div>
        <p>A workspace that feels right for you.</p>
        {#each [["theme", "Theme", [["system", "System"], ["light", "Light"], ["dark", "Dark"]]], ["font_size", "Font size", [["small", "Small"], ["default", "Default"], ["large", "Large"]]], ["density", "Density", [["comfortable", "Comfortable"], ["compact", "Compact"]]]] as [key, label, options]}<div
            class="settings-row"
          >
            <label for={`setting_${key}`}>{label}</label><select
              id={`setting_${key}`}
              value={settings[key]}
              onchange={(e) => on_change({ [key]: e.currentTarget.value })}
              >{#each options as [value, name]}<option {value}>{name}</option
                >{/each}</select
            >
          </div>{/each}{:else if active_tab === "behavior"}<h2>
          A little more your way.
        </h2>
        <p>Choose how each conversation flows.</p>
        {#each [["enter_to_send", "Enter to send", "Use Shift + Enter for a new line."], ["auto_scroll", "Auto-scroll responses", "Follow the response until you scroll up."], ["default_thinking", "Extended thinking by default", "Take more time on involved questions."]] as [key, label, description]}<div
            class="settings-row"
          >
            <div>
              <label for={`setting_${key}`}>{label}</label>
              <p>{description}</p>
            </div>
            <input
              class="toggle"
              id={`setting_${key}`}
              type="checkbox"
              checked={settings[key]}
              onchange={(e) => on_change({ [key]: e.currentTarget.checked })}
            />
          </div>{/each}
        <div class="settings-row">
          <label for="default_model">Default model</label><select
            id="default_model"
            value={settings.default_model}
            onchange={(e) =>
              on_change({ default_model: e.currentTarget.value })}
            >{#each models as model}<option value={model.id}
                >{model.name}</option
              >{/each}</select
          >
        </div>{:else if active_tab === "api"}<h2>Bring your own Claude.</h2>
        <p>
          Connect directly to Anthropic to start a conversation. Your API
          account controls model access, billing, and rate limits.
        </p>
        {#if !api_key}<div class="status-note">
            <strong>Let’s get connected</strong>
            <ol>
              <li>
                Create an API key in the <a
                  href="https://console.anthropic.com/settings/keys"
                  target="_blank"
                  rel="noopener noreferrer"
                  >Anthropic Console <ExternalLink /></a
                >.
              </li>
              <li>Paste it below, then save and validate.</li>
              <li>Return to your conversation and send your message.</li>
            </ol>
          </div>{/if}
        <form onsubmit={save}>
          <label class="field" for="api_key_input"
            >API key
            <div class="key-field">
              <input
                id="api_key_input"
                aria-label="API key"
                type={visible ? "text" : "password"}
                autocomplete="off"
                spellcheck="false"
                bind:value={key_draft}
                placeholder="sk-ant-…"
              /><button
                type="button"
                class="icon-btn"
                aria-label={visible ? "Hide API key" : "Show API key"}
                onclick={() => (visible = !visible)}
                >{#if visible}<EyeOff />{:else}<Eye />{/if}</button
              >
            </div></label
          ><button
            class="btn btn-primary"
            disabled={saving || (!key_draft && !api_key)}
            type="submit">{saving ? "Validating…" : "Save & validate"}</button
          >{#if api_key}<button
              class="btn"
              type="button"
              disabled={saving}
              onclick={async () => {
                key_draft = "";
                try {
                  await on_api_save("");
                } catch (err) {
                  error = err.message;
                }
              }}>Remove key</button
            >{/if}
        </form>
        <div class="status-note" role="status">{error || api_status}</div>
        <p class="security-note">
          Your key is held only in this page’s memory. It is never saved in
          browser storage, chat exports, or VibeMux’s database, and is cleared
          when you reload. Messages and selected attachments are sent directly
          to api.anthropic.com when you send. Browser memory is not an encrypted
          credential vault.
        </p>{:else if active_tab === "data"}<h2>Your ideas stay yours.</h2>
        <p>
          {conversation_count} conversations saved in this browser. Conversation
          history and attachments are stored locally on this device and are separate
          from VibeMux’s daemon state.
        </p>
        <h3>Take a copy with you</h3>
        <button class="btn" onclick={() => on_export("md")}
          >Export all as Markdown</button
        ><button class="btn" onclick={() => on_export("json")}
          >Export all as JSON</button
        >
        <h3>Start fresh</h3>
        <p>
          Clear local conversation history. You can undo from the notification.
        </p>
        <button
          class="btn btn-danger"
          disabled={!conversation_count}
          onclick={on_clear}>Clear conversations</button
        >{:else}<h2>Keep your flow.</h2>
        <p>
          Shortcuts work throughout the workspace and avoid browser defaults.
        </p>
        <div class="shortcut-list">
          {#each shortcuts as [label, key]}<div>
              <span>{label}</span><kbd>{key}</kbd>
            </div>{/each}
        </div>{/if}
    </section>
  </div>
</div>
