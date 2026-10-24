// The apps page automation runs only in a feature=e2e binary with ALEF_E2E=1.
import { call, ready, settled } from './apps.js';
const emit = data => call('e2e.report', { line: JSON.stringify(data) });
const need = (node, name) => { if (!node) throw new Error(`missing UI control: ${name}`); return node; };
// Native dialogs are answered from ALEF_E2E_DIALOGS by the runtime host: a JSON array like
// [{"save":["C:/tmp/out.zip"]},{"open":["C:/tmp/in.zip"]}] — exactly one answer per dialog
// call, consumed in order (Save for export, Open for import).
const waitFor = async (probe, ms = 10000) => {
  const deadline = Date.now() + ms;
  for (;;) {
    const value = probe();
    if (value) return value;
    if (Date.now() > deadline) throw new Error('waitFor: timed out');
    await new Promise(resolve => setTimeout(resolve, 100));
  }
};
const visibleStatus = () => { const node = document.querySelector('.status'); return node && !node.hidden && node.textContent.trim() ? node.textContent : null; };
const visibleConfirm = () => { const node = document.querySelector('.confirm'); return node && !node.hidden ? node : null; };
try {
  await ready;
  await settled;
  const ui = await import('./apps.js');
  const steps = await call('runtime.apps.automation');
  if (!Array.isArray(steps)) throw new Error('ALEF_E2E_APPS_UI must be a JSON step array');
  for (const step of steps) {
    if (step.search !== undefined) {
      const search = need(document.querySelector('input[type=search]'), 'search');
      search.value = step.search; search.dispatchEvent(new Event('input', { bubbles: true }));
    }
    if (step.select) {
      const button = [...document.querySelectorAll('.app')].find(node => node.textContent.includes(step.select));
      need(button, `app ${step.select}`).click(); await ui.settled;
    }
    if (step.choose) {
      const row = [...document.querySelectorAll('.right')].find(node => node.querySelector('.what')?.textContent === step.choose);
      const radio = need(row?.querySelector(`input[value="${step.decision}"]`), `${step.choose} ${step.decision}`);
      radio.checked = true; radio.dispatchEvent(new Event('change', { bubbles: true }));
    }
    if (step.export !== undefined) {
      const selected = ui.view?.selected();
      if (selected && !(selected.id.includes(step.export) || (selected.name ?? '').includes(step.export))) throw new Error(`selected ${selected.id} is not ${step.export}`);
      const button = [...document.querySelectorAll('button')].find(node => node.textContent === 'Экспорт данных');
      need(button, `export ${step.export}`).click();
      await waitFor(() => (/Экспортировано: \d+/.test(visibleStatus() ?? '') ? visibleStatus() : null));
      await emit({ type: 'check', name: 'export-ok', ok: true, text: visibleStatus() });
    }
    if (step.import !== undefined) {
      const selected = ui.view?.selected();
      if (selected && !(selected.id.includes(step.import) || (selected.name ?? '').includes(step.import))) throw new Error(`selected ${selected.id} is not ${step.import}`);
      const button = [...document.querySelectorAll('button')].find(node => node.textContent === 'Импорт данных');
      need(button, `import ${step.import}`).click();
      await waitFor(visibleConfirm);
      if (!step.confirm) {
        await emit({ type: 'check', name: 'import-confirmation-required', ok: true, text: visibleConfirm()?.textContent ?? '' });
      } else {
        const check = need(visibleConfirm()?.querySelector('input[type=checkbox]'), 'import confirmation');
        check.checked = true; check.dispatchEvent(new Event('change', { bubbles: true }));
        const replace = [...document.querySelectorAll('button')].find(node => node.textContent === 'Заменить данные');
        need(replace, 'replace data').click();
        await waitFor(() => (/Данные заменены: \d+/.test(visibleStatus() ?? '') ? visibleStatus() : null));
        await emit({ type: 'check', name: 'import-ok', ok: true, text: visibleStatus() });
      }
    }
    if (step.confirm && step.import === undefined) {
      const row = [...document.querySelectorAll('.right')].find(node => node.querySelector('.what')?.textContent === step.confirm);
      const check = need(row?.querySelector('input[type=checkbox]'), `confirm ${step.confirm}`);
      check.checked = true; check.dispatchEvent(new Event('change', { bubbles: true }));
    }
    if (step.save) {
      const row = [...document.querySelectorAll('.right')].find(node => node.querySelector('.what')?.textContent === step.save);
      need(row?.querySelector('button'), `save ${step.save}`).click(); await ui.settled;
    }
    if (step.reset) {
      const button = [...document.querySelectorAll('button')].find(node => node.textContent === 'Reset decisions');
      need(button, 'reset').click(); await ui.settled;
    }
    if (step.negativeRed) {
      try {
        await call('runtime.apps.set', { identity: step.identity ?? ui.view?.selected()?.identity, right: step.negativeRed, decision: 'allow', confirm_red: false });
        await emit({ type: 'check', name: 'red-confirmation-required', ok: false, error: 'the backend accepted an unconfirmed red right' });
      } catch (error) {
        const text = error instanceof Error ? error.message : String(error);
        await emit({ type: 'check', name: 'red-confirmation-required', ok: text.includes('confirm-red'), error: text });
      }
    }
    if (step.snapshot) {
      await emit({
        type: 'snapshot',
        name: step.snapshot,
        selected: ui.view?.selected()?.identity,
        list: ui.view?.visible().map(app => ({ id: app.id, name: app.name, version: app.version, source: app.source, availability: app.availability, summary: app.summary })),
        rows: [...document.querySelectorAll('.right')].map(row => ({
          right: row.querySelector('.what')?.textContent,
          checked: row.querySelector('input[type=radio]:checked')?.value ?? null,
          red: row.classList.contains('red'),
          confirmed: Boolean(row.querySelector('input[type=checkbox]')?.checked),
          error: row.querySelector('.error')?.textContent ?? '',
        })),
      });
    }
    if (step.stability !== undefined) {
      // Flash regression guard: the clicked list button must survive selection, and the
      // details panel must never show an intermediate heading-only state during a load.
      const button = [...document.querySelectorAll('.app')].find(node => node.textContent.includes(step.stability));
      need(button, `stability ${step.stability}`);
      const content = need(document.getElementById('details'), 'details panel');
      let flash = null;
      const observer = new MutationObserver(() => {
        if (content.querySelector('h2') && !content.querySelector('.toolbar') && !content.querySelector('.empty')) {
          flash = 'details panel showed an intermediate heading-only state';
        }
      });
      observer.observe(content, { childList: true, subtree: true });
      button.click();
      await ui.settled;
      observer.disconnect();
      if (!flash && content.childElementCount && !content.querySelector('.toolbar')) flash = 'details panel finished without a toolbar';
      await emit({ type: 'check', name: 'selection-node-stable', ok: button.isConnected, ...(button.isConnected ? {} : { error: 'the clicked list button was recreated' }) });
      await emit({ type: 'check', name: 'details-no-flash', ok: !flash, ...(flash ? { error: flash } : {}) });
    }
    if (step.close) await call('runtime.apps.close');
  }
} catch (error) { await emit({ type: 'failure', message: error?.message ?? String(error) }); }
