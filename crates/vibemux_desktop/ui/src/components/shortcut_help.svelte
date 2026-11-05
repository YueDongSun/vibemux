<script lang="ts">
  import { SHORTCUTS, modal_focus } from '../lib/keyboard_shortcuts';
  let { on_close }: { on_close: () => void } = $props();
</script>
<dialog class="shell_dialog shortcut_dialog" aria-labelledby="shortcut_title" use:modal_focus oncancel={(event) => { if (!event.defaultPrevented) on_close(); }}>
  <header class="dialog_header"><h2 id="shortcut_title">Keyboard shortcuts</h2><button class="icon_button" data-initial-focus aria-label="Close keyboard shortcuts" onclick={on_close}>×</button></header>
  <dl class="shortcut_list">{#each SHORTCUTS as shortcut}<div><dt>{shortcut.label}</dt><dd><kbd>{shortcut.keys}</kbd></dd></div>{/each}<div><dt>Send a text prompt</dt><dd><kbd>Enter</kbd></dd></div><div><dt>New line</dt><dd><kbd>Shift + Enter</kbd></dd></div></dl>
  <p class="settings_note">Shortcuts are ignored during IME composition. Closing a panel or changing selection does not cancel execution.</p>
</dialog>
