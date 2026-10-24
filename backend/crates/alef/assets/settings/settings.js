// The settings page runs only in the runtime-owned settings process.
const capability = new URLSearchParams(location.hash.slice(1)).get('capability') ?? '';
let token;
export async function call(command, args = null) {
  const response = await fetch(`native://call/${encodeURIComponent(command)}`, {
    method: 'POST', headers: { Authorization: `Bearer ${token ?? capability}`, 'Content-Type': 'application/json' },
    body: command === 'runtime.hello' ? '{}' : JSON.stringify(args),
  });
  if (!response.ok) {
    let payload;
    try { payload = await response.json(); } catch { payload = null; }
    throw new Error(payload?.message ?? payload?.error?.message ?? `${command}: ${response.status}`);
  }
  return response.json();
}
const el = (tag, props = {}, ...children) => {
  const node = document.createElement(tag);
  for (const [key, value] of Object.entries(props)) {
    if (key === 'text') node.textContent = value;
    else if (key === 'class') node.className = value;
    else if (key === 'disabled') node.disabled = Boolean(value);
    else node.setAttribute(key, value);
  }
  node.append(...children); return node;
};
export let ready;
let busy = 0;
let wake = () => {};
export let settled = Promise.resolve();
function begin() {
  if (busy === 0) settled = new Promise(resolve => { wake = resolve; });
  busy += 1;
}
function end() {
  busy = Math.max(0, busy - 1);
  if (busy === 0) { const done = wake; wake = () => {}; done(); }
}
function apply(view) {
  document.documentElement.lang = view.language;
  document.documentElement.dir = view.rtl ? 'rtl' : 'ltr';
  document.title = view.texts.title;
  const root = document.getElementById('root');
  const list = el('div', { class: 'languages', role: 'group', 'aria-label': view.texts.section });
  const error = el('p', { class: 'error', role: 'alert' });
  for (const option of view.languages) {
    const button = el('button', {
      type: 'button', text: option.nativeName, 'data-code': option.code,
      'aria-pressed': String(option.code === view.language),
    });
    button.addEventListener('click', async () => {
      begin(); for (const other of list.children) other.disabled = true; error.textContent = '';
      try {
        await call('runtime.settings.set', { code: option.code });
        apply(await call('runtime.settings.get'));
      } catch (problem) {
        error.textContent = problem instanceof Error ? problem.message : String(problem);
      }
      finally { for (const other of list.children) other.disabled = false; end(); }
    });
    list.append(button);
  }
  root.replaceChildren(el('h1', { text: view.texts.section }), el('p', { class: 'sub', text: view.texts.label }), list, error);
}
async function main() {
  try {
    const hello = await call('runtime.hello');
    token = hello.token;
    apply(await call('runtime.settings.get'));
  } catch (error) {
    document.getElementById('root').textContent = `Settings cannot be shown: ${error.message}`;
  }
}
ready = main();
