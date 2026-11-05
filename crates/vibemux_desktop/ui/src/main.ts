import { mount } from 'svelte';
import { parse_task_window_bootstrap } from './lib/task_window_state';
import './style.css';

const target = document.getElementById('app');
const bootstrap = parse_task_window_bootstrap(window.location.search);

async function start() {
  if (!target) return;
  if (bootstrap.kind === 'main') {
    const { default: App } = await import('./app.svelte');
    mount(App, { target });
  } else if (bootstrap.kind === 'task') {
    const { default: TaskWindow } = await import('./components/task_window.svelte');
    mount(TaskWindow, { target, props: { binding: bootstrap.binding } });
  } else {
    show_error();
  }
}

function show_error() {
  if (!target) return;
  const message = document.createElement('p');
  message.setAttribute('role', 'alert');
  message.textContent = 'This window could not load its task. Close it and open the task again.';
  target.replaceChildren(message);
}

void start().catch(show_error);
