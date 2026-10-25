// The manager page runs only in the runtime-owned apps process.
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
    else if (key === 'disabled' || key === 'checked' || key === 'hidden' || key === 'open') node[key] = Boolean(value);
    else node.setAttribute(key, value);
  }
  node.append(...children); return node;
};
const rightText = right => right.scope == null ? right.permission : `${right.permission}:${right.scope}`;
const nameOf = app => app.name ?? app.id;
export let ready;
let busy = 0;
let wake = () => {};
export let settled = Promise.resolve();
export let view;
function begin() {
  if (busy === 0) settled = new Promise(resolve => { wake = resolve; });
  busy += 1;
}
function end() {
  busy = Math.max(0, busy - 1);
  if (busy === 0) { const done = wake; wake = () => {}; done(); }
}
function showError(node, error) { node.textContent = error instanceof Error ? error.message : String(error); }
function render(root, initial) {
  let entries = initial; let selected; let visible = [];
  root.replaceChildren(el('h1', { text: 'Applications' }), el('p', { class: 'sub', text: 'Review and change what each application may do.' }));
  const filter = el('input', { type: 'search', 'aria-label': 'Search applications', placeholder: 'Search name, id, version or source' });
  const list = el('div', { class: 'apps', role: 'list', 'aria-label': 'Applications' });
  const content = el('section', { class: 'panel', id: 'details', 'aria-label': 'Application details' });
  root.append(el('div', { class: 'layout' }, el('section', { class: 'panel' }, filter, list), content));
  const sameIdentity = (a, b) => a?.identity.app_id === b?.identity.app_id && a?.identity.signer === b?.identity.signer;
  const keyOf = app => `${app.identity.app_id}\u0000${app.identity.signer}`;
  const buttons = new Map();
  function fillButton(button, app) {
    button.replaceChildren(
      el('strong', { text: nameOf(app) }),
      el('small', { text: `${app.id} · ${app.version ?? '—'} · ${app.source ?? '—'}` }),
      el('small', { text: `allow ${app.summary.allow} · substitute ${app.summary.substitute} · deny ${app.summary.deny} · undecided ${app.summary.undecided} · red ${app.summary.red}` }));
  }
  // Reuse list buttons keyed by identity; only content and aria-current change in place.
  function listItems() {
    visible = entries.filter(app => `${nameOf(app)} ${app.id} ${app.version ?? ''} ${app.source ?? ''}`.toLocaleLowerCase().includes(filter.value.toLocaleLowerCase()));
    if (!visible.length) {
      list.replaceChildren(el('p', { class: 'empty', text: entries.length ? 'No applications match your search.' : 'No applications found.' }));
      return;
    }
    const nodes = visible.map(app => {
      const key = keyOf(app);
      let button = buttons.get(key);
      if (!button) {
        button = el('button', { class: 'app', type: 'button', role: 'listitem' });
        button.addEventListener('click', () => { const current = entries.find(row => keyOf(row) === key); if (current) select(current); });
        buttons.set(key, button);
      }
      fillButton(button, app);
      button.setAttribute('aria-current', String(sameIdentity(selected, app)));
      return button;
    });
    list.replaceChildren(...nodes);
  }
  async function refreshList() {
    entries = await call('runtime.apps.list');
    if (selected) selected = entries.find(app => sameIdentity(app, selected)) ?? selected;
    listItems();
  }
  // Selection only flips aria-current on existing buttons; the list is not rebuilt.
  function select(app) {
    begin(); selected = app;
    const key = keyOf(app);
    for (const [other, button] of buttons) button.setAttribute('aria-current', String(other === key));
    void loadDetails(app);
  }
  // Focus key: identify a focused control so it can be found again after the atomic swap.
  const focusKeyOf = node => {
    if (!(node instanceof HTMLElement)) return null;
    if (node.type === 'radio') return `radio ${node.name} ${node.value}`;
    if (node.tagName === 'BUTTON') return `button ${node.textContent}`;
    return null;
  };
  const restoreFocus = (nodes, key) => {
    if (!key) return;
    for (const root of nodes) {
      if (!(root instanceof HTMLElement)) continue;
      for (const candidate of root.querySelectorAll('input,button')) if (focusKeyOf(candidate) === key) { candidate.focus(); return; }
    }
  };
  async function loadDetails(app) {
    // Keep the previous panel on screen while loading; swap atomically once data arrives.
    content.classList.add('busy'); content.setAttribute('aria-busy', 'true');
    const focused = document.activeElement; const inPanel = content.contains(focused);
    const focusKey = focusKeyOf(focused); const scrollTop = content.scrollTop;
    const nodes = []; let stale = false;
    const topError = el('p', { class: 'error', role: 'alert' });
    try {
      const description = await call('runtime.apps.describe', { identity: app.identity });
      if (!sameIdentity(selected, app)) { stale = true; return; }
      const toolbar = el('div', { class: 'toolbar' });
      nodes.push(el('h2', { text: nameOf(app) }), el('p', { class: 'muted', text: `${app.id} · ${app.version ?? '—'} · ${app.source ?? '—'} · ${app.availability}` }));
      const status = el('p', { class: 'status', role: 'status' });
      const dataCheck = el('input', { type: 'checkbox' });
      const dataText = el('div', {});
      const replaceButton = el('button', { type: 'button', text: 'Replace data' });
      const dataConfirm = el('div', { class: 'confirm', hidden: true },
        el('div', { text: 'The existing data of the application will be replaced. The previous data are kept next to it in <dir>.before-import.' }),
        dataText,
        el('label', {}, dataCheck, document.createTextNode('I understand: the data of the application will be replaced.')),
        replaceButton);
      let importPath = null;
      const showStatus = (text, error) => { status.textContent = text; status.className = error ? 'status error' : 'status'; };
      const hideConfirm = () => { dataConfirm.hidden = true; dataCheck.checked = false; importPath = null; };
      const exportButton = el('button', { type: 'button', text: 'Export data', disabled: app.availability !== 'available' });
      exportButton.addEventListener('click', async () => {
        begin(); exportButton.disabled = true; showStatus(''); hideConfirm();
        try {
          const pick = await call('runtime.apps.data.pickExport', { identity: app.identity });
          if (!pick.path) return;
          const result = await call('runtime.apps.data.export', { identity: app.identity, path: pick.path });
          showStatus(`Exported: ${result.files} files, ${result.bytes} bytes`);
        } catch (error) {
          showStatus(error instanceof Error ? error.message : String(error), true);
        }
        finally { exportButton.disabled = app.availability !== 'available'; end(); }
      });
      const importButton = el('button', { type: 'button', text: 'Import data', disabled: app.availability !== 'available' });
      importButton.addEventListener('click', async () => {
        begin(); importButton.disabled = true; showStatus(''); hideConfirm();
        try {
          const pick = await call('runtime.apps.data.pickImport', { identity: app.identity });
          if (!pick.path) return;
          importPath = pick.path;
          dataText.textContent = `${pick.files} files, ${pick.bytes} bytes`;
          dataConfirm.hidden = false;
        } catch (error) {
          showStatus(error instanceof Error ? error.message : String(error), true);
        }
        finally { importButton.disabled = app.availability !== 'available'; end(); }
      });
      replaceButton.addEventListener('click', async () => {
        begin(); replaceButton.disabled = true; showStatus('');
        try {
          if (!dataCheck.checked) throw new Error('Tick the box to confirm the replacement of the data.');
          const result = await call('runtime.apps.data.import', { identity: app.identity, path: importPath, confirm_red: true });
          hideConfirm();
          showStatus(`Data replaced: ${result.files} files, ${result.bytes} bytes. Previous data: ${result.backup ?? '—'}`);
        } catch (error) {
          showStatus(error instanceof Error ? error.message : String(error), true);
        }
        finally { replaceButton.disabled = false; end(); }
      });
      const resetButton = el('button', { type: 'button', text: 'Reset decisions' });
      resetButton.addEventListener('click', async () => {
        begin(); resetButton.disabled = true; topError.textContent = '';
        try {
          await call('runtime.apps.reset', { identity: app.identity });
          await refreshList();
          await renderDetails(app);
        } catch (error) {
          showError(topError, error);
          resetButton.disabled = false;
        }
        finally { end(); }
      });
      const close = el('button', { type: 'button', text: 'Close' });
      close.addEventListener('click', () => call('runtime.apps.close').catch(error => showError(topError, error)));
      nodes.push(el('h2', { text: nameOf(app) }), el('p', { class: 'muted', text: `${app.id} · ${app.version ?? '—'} · ${app.source ?? '—'}` }));
      toolbar.append(exportButton, importButton, resetButton, close); nodes.push(toolbar, status, dataConfirm, topError);
      if (app.availability === 'damaged') nodes.push(el('p', { class: 'error', text: 'The installed application record is damaged.' }));
      if (app.availability !== 'available') nodes.push(el('p', { class: 'empty', text: 'The source is unavailable; saved rights are shown read-only. Reset is available, but changes cannot take effect until the source is available.' }));
      nodes.push(el('p', { class: 'note', text: 'Taking a right away narrows a running application within about 500 ms. Giving a right or widening it takes effect on the next launch (except webcrypto, whose decision is set when the document starts). Reset leaves rights undecided; the next launch asks again.' }));
      for (const entry of description.rights) {
        const label = rightText(entry.right); const rowError = el('p', { class: 'error', role: 'alert' });
        const choices = el('fieldset', { class: 'choices', 'aria-label': `Decision for ${label}` }); const controls = {};
        for (const decision of ['allow', 'substitute', 'deny']) {
          const radio = el('input', { type: 'radio', name: `right-${label}`, value: decision });
          controls[decision] = radio; radio.checked = entry.decision === decision;
          choices.append(el('label', {}, radio, document.createTextNode(decision[0].toUpperCase() + decision.slice(1))));
        }
        if (app.availability !== 'available') {
          for (const radio of Object.values(controls)) radio.disabled = true;
        }
        const checkbox = el('input', { type: 'checkbox' }); let explicitlyConfirmed = false;
        if (!entry.red) checkbox.disabled = true;
        const confirmation = el('div', { class: 'confirm' }, el('label', {}, checkbox, document.createTextNode('I understand this red right bypasses all protections, including the AI model keys.')));
        confirmation.hidden = !entry.red; checkbox.addEventListener('change', () => { explicitlyConfirmed = checkbox.checked; });
        const row = el('div', { class: entry.red ? 'right red' : 'right' }, el('div', { class: 'what', text: label }), choices);
        if (entry.description) row.append(el('div', { class: 'detail', text: entry.description }));
        if (entry.red) row.append(confirmation);
        const save = el('button', { type: 'button', text: 'Save decision', disabled: app.availability !== 'available' });
        save.addEventListener('click', async () => {
          begin(); const decision = Object.keys(controls).find(key => controls[key].checked);
          try {
            rowError.textContent = '';
            if (!decision) throw new Error('Choose a decision first.');
            save.disabled = true;
            await call('runtime.apps.set', {
              identity: app.identity,
              right: entry.right,
              decision,
              confirm_red: entry.red && explicitlyConfirmed,
            });
            explicitlyConfirmed = false; checkbox.checked = false;
            await refreshList();
            await renderDetails(app);
          } catch (error) {
            showError(rowError, error);
            save.disabled = app.availability !== 'available';
          }
          finally { end(); }
        });
        for (const radio of Object.values(controls)) {
          radio.addEventListener('change', () => {
            explicitlyConfirmed = false;
            checkbox.checked = false;
            rowError.textContent = '';
          });
        }
        row.append(save, rowError); nodes.push(row);
      }
    } catch (error) { if (!stale) nodes.push(el('p', { class: 'error', role: 'alert', text: `Application details cannot be shown: ${error.message}` })); }
    finally {
      if (!stale && nodes.length) {
        content.replaceChildren(...nodes);
        content.scrollTop = scrollTop;
        if (inPanel) restoreFocus(nodes, focusKey);
      }
      content.classList.remove('busy'); content.removeAttribute('aria-busy');
      end();
    }
  }
  async function renderDetails(app) {
    selected = entries.find(row => sameIdentity(row, app)) ?? app;
    begin();
    listItems();
    await loadDetails(selected);
  }
  filter.addEventListener('input', listItems); listItems();
  if (entries.length) select(entries[0]); else content.append(el('p', { class: 'empty', text: 'No applications found.' }));
  return { entries: () => entries, visible: () => visible, selected: () => selected };
}
async function main() {
  try { const hello = await call('runtime.hello'); token = hello.token; view = render(document.getElementById('root'), await call('runtime.apps.list')); return view; }
  catch (error) { document.getElementById('root').textContent = `Applications cannot be shown: ${error.message}`; }
}
ready = main();
