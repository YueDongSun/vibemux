<script>
  import { onMount } from "svelte";
  import { Plus, Trash2 } from "@lucide/svelte";

  let { profiles } = $props();
  let config = $state(null),
    saved = $state(""),
    loading = $state(true),
    saving = $state(false),
    error = $state("");
  let disposed = false;
  const dirty = $derived(config !== null && JSON.stringify(config) !== saved);

  async function load_profiles() {
    if (saving) return;
    loading = true;
    error = "";
    try {
      const view = await profiles.get();
      if (disposed) return;
      config = view.config;
      saved = JSON.stringify(view.config);
    } catch (failure) {
      if (!disposed) error = failure.message;
    } finally { if (!disposed) loading = false; }
  }
  function add_profile() {
    if (!config || loading || saving || config.profiles.length >= profiles.max_profiles) return;
    const ids = new Set(config.profiles.map((profile) => profile.id));
    let number = 1;
    while (ids.has(`profile_${number}`)) number += 1;
    config.profiles = [...config.profiles, {
      id: `profile_${number}`,
      name: `Provider ${number}`,
      api_type: "anthropic_messages",
      base_url: "https://api.anthropic.com",
      model: "",
      credential_env: "ANTHROPIC_API_KEY",
      auth_type: "api_key",
    }];
    error = "";
  }
  function remove_profile(index) {
    if (!config || loading || saving) return;
    if (config.orchestrator_profile_id === config.profiles[index].id) config.orchestrator_profile_id = null;
    config.profiles = config.profiles.filter((_, position) => position !== index);
    error = "";
  }
  function rename_id(index, value) {
    const previous = config.profiles[index].id;
    config.profiles[index].id = value;
    if (config.orchestrator_profile_id === previous) config.orchestrator_profile_id = value;
  }
  function set_auth(index, value) {
    config.profiles[index].auth_type = value;
    config.profiles[index].credential_env = value === "none" ? null : config.profiles[index].credential_env || "";
  }
  async function save_profiles(event) {
    event.preventDefault();
    if (!config || loading || saving) return;
    error = "";
    saving = true;
    try {
      const snapshot = $state.snapshot(config);
      profiles.validate(snapshot);
      const view = await profiles.save(snapshot);
      if (disposed) return;
      config = view.config;
      saved = JSON.stringify(view.config);
    } catch (failure) {
      if (!disposed) error = failure.message;
    } finally { if (!disposed) saving = false; }
  }
  onMount(() => {
    void load_profiles();
    return () => { disposed = true; };
  });
</script>

<h2>Provider profiles</h2>
<div class="status-note">
  <strong>Configuration only — not connected</strong>
  <p>Save API endpoints, model IDs and credential environment-variable names. No key values are read or stored here, and saving never calls a provider.</p>
  <p>Desktop chat is unavailable. This configuration is not connected to ADR 037’s separately gated, startup-pinned gateway. Selecting an orchestrator profile does not change a canonical route or authorize execution.</p>
</div>
{#if error}<div class="status-note" role="alert">{error}</div>{/if}
{#if loading}
  <p role="status">Loading provider configuration…</p>
{:else if !config}
  <p>The existing configuration must be read successfully before you can edit it.</p>
  <button type="button" class="btn" onclick={load_profiles}>Retry loading profiles</button>
{:else}
  <form onsubmit={save_profiles}>
    <fieldset class="provider-profiles" disabled={saving}>
      <legend class="sr-only">Provider configuration</legend>
      <div class="settings-row">
        <label for="orchestrator_profile">Orchestrator profile</label>
        <select id="orchestrator_profile" value={config.orchestrator_profile_id || ""}
          onchange={(event) => (config.orchestrator_profile_id = event.currentTarget.value || null)}>
          <option value="">Not selected</option>
          {#each config.profiles as profile}<option value={profile.id}>{profile.name || profile.id}</option>{/each}
        </select>
      </div>
      <p>Up to {profiles.max_profiles} profiles. Model IDs here are separate from local conversation records.</p>
      {#each config.profiles as profile, index}
        <fieldset class="provider-profile">
          <legend>{profile.name || `Provider ${index + 1}`}</legend>
          <div class="provider-fields">
            <label class="field" for={`profile_${index}_name`}>Name
              <input id={`profile_${index}_name`} type="text" maxlength="128" required bind:value={profile.name} />
            </label>
            <label class="field" for={`profile_${index}_id`}>Profile ID
              <input id={`profile_${index}_id`} type="text" maxlength="64" required pattern="[A-Za-z0-9_\-]+"
                value={profile.id} oninput={(event) => rename_id(index, event.currentTarget.value)} autocomplete="off" spellcheck="false" />
            </label>
            <label class="field" for={`profile_${index}_api`}>API type
              <select id={`profile_${index}_api`} bind:value={profile.api_type}>
                <option value="anthropic_messages">Anthropic Messages</option>
                <option value="openai_chat_completions">OpenAI-compatible Chat Completions</option>
              </select>
            </label>
            <label class="field" for={`profile_${index}_url`}>Base URL
              <input id={`profile_${index}_url`} type="text" maxlength="2048" required bind:value={profile.base_url} autocomplete="off" spellcheck="false" />
            </label>
            <p>Use HTTPS, or HTTP at a numeric loopback address. Keep any versioned base path; do not include credentials, a query or a fragment.</p>
            <label class="field" for={`profile_${index}_model`}>Model ID
              <input id={`profile_${index}_model`} type="text" maxlength="256" required bind:value={profile.model} autocomplete="off" spellcheck="false" placeholder="Provider model ID" />
            </label>
            <label class="field" for={`profile_${index}_auth`}>Authentication
              <select id={`profile_${index}_auth`} value={profile.auth_type} onchange={(event) => set_auth(index, event.currentTarget.value)}>
                <option value="api_key">API key from environment</option>
                <option value="bearer">Bearer token from environment</option>
                <option value="none">No authentication</option>
              </select>
            </label>
            {#if profile.auth_type !== "none"}
              <label class="field" for={`profile_${index}_env`}>Credential environment-variable name
                <input id={`profile_${index}_env`} type="text" maxlength="128" required pattern="[A-Za-z_][A-Za-z0-9_]*"
                  bind:value={profile.credential_env} autocomplete="off" spellcheck="false" placeholder="PROVIDER_API_KEY" />
              </label>
              <p>Enter a variable name only, never a secret. No environment value is resolved or checked by these settings.</p>
            {/if}
          </div>
          <button type="button" class="btn btn-danger" aria-label={`Remove ${profile.name || profile.id}`} onclick={() => remove_profile(index)}><Trash2 />Remove profile</button>
        </fieldset>
      {/each}
      <button type="button" class="btn" disabled={config.profiles.length >= profiles.max_profiles} onclick={add_profile}><Plus />Add profile</button>
      <button type="submit" class="btn btn-primary" disabled={!dirty}>{saving ? "Saving profiles…" : "Save profiles"}</button>
    </fieldset>
    <p role="status">{saving ? "Saving provider configuration…" : dirty ? "Unsaved provider changes" : "Saved configuration only — no provider connection has been tested."}</p>
  </form>
{/if}
