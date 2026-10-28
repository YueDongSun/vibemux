<script>
  import { onMount } from "svelte";
  import {
    PanelsTopLeft,
    SquareTerminal,
    MessageSquare,
    Check,
  } from "@lucide/svelte";
  import { interface_options } from "../interface_policy.js";
  let { value, on_select } = $props();
  let menu;
  function choose(style) {
    on_select(style);
    menu.open = false;
    menu.querySelector("summary").focus();
  }
  function on_key(event) {
    if (event.key === "Escape" && menu.open) {
      event.preventDefault();
      event.stopPropagation();
      menu.open = false;
      menu.querySelector("summary").focus();
    }
  }
  onMount(() => {
    const outside = (event) => {
      if (!menu.contains(event.target)) menu.open = false;
    };
    document.addEventListener("pointerdown", outside);
    document.addEventListener("keydown", on_key);
    return () => {
      document.removeEventListener("pointerdown", outside);
      document.removeEventListener("keydown", on_key);
    };
  });
</script>

<details bind:this={menu} class="interface-switcher">
  <summary
    class="icon-btn"
    aria-label="Switch interface"
    title={`Interface: ${value === "codex" ? "Codex" : "Claude"}`}
    ><PanelsTopLeft /></summary
  >
  <div class="popover interface-menu">
    <div class="popover-label">CHOOSE YOUR WORKSPACE</div>
    {#each interface_options as option}
      <button
        class="interface-option"
        aria-pressed={value === option.id}
        onclick={() => choose(option.id)}
      >
        {#if option.id === "codex"}<SquareTerminal />{:else}<MessageSquare
          />{/if}
        <span
          ><strong>{option.name}</strong><small>{option.description}</small
          ></span
        >
        {#if value === option.id}<Check />{/if}
      </button>
    {/each}
    <p>Same conversations. Your preferred view.</p>
  </div>
</details>
