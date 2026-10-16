import { mount } from 'svelte';
import App from './app.svelte';
import './style.css';
import { registerSW } from 'virtual:pwa-register';

mount(App, { target: document.getElementById('app')! });

let update_requested = false;
const update_service_worker = registerSW({
  immediate: true,
  onNeedRefresh() {
    window.dispatchEvent(new CustomEvent('vibemux:update_available'));
  },
  onNeedReload: () => undefined,
});

window.addEventListener('vibemux:apply_update', () => {
  update_requested = true;
  void update_service_worker(true);
});

if ('serviceWorker' in navigator) {
  navigator.serviceWorker.addEventListener('controllerchange', () => {
    if (!update_requested) return;
    update_requested = false;
    window.location.reload();
  });
}
