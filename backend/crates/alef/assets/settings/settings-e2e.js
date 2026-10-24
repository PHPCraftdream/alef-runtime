// The settings page automation runs only in a feature=e2e binary with ALEF_E2E=1.
import { call, ready, settled } from './settings.js';
const emit = data => call('e2e.report', { line: JSON.stringify(data) });
const need = (node, name) => { if (!node) throw new Error(`missing UI control: ${name}`); return node; };
try {
  await ready;
  await settled;
  const steps = await call('runtime.settings.automation');
  if (!Array.isArray(steps)) throw new Error('ALEF_E2E_SETTINGS_UI must be a JSON step array');
  for (const step of steps) {
    if (step.choose) {
      const button = [...document.querySelectorAll('button[data-code]')].find(node => node.dataset.code === step.choose);
      need(button, `language ${step.choose}`).click();
      await settled;
    }
    if (step.snapshot) {
      await emit({
        type: 'snapshot',
        name: step.snapshot,
        language: document.documentElement.lang,
        dir: document.documentElement.dir,
        title: document.title,
        heading: document.querySelector('h1')?.textContent ?? '',
        options: [...document.querySelectorAll('button[data-code]')].map(node => node.textContent),
        selected: document.querySelector('button[aria-pressed="true"]')?.dataset.code ?? null,
      });
    }
    if (step.close) await call('runtime.settings.close');
  }
} catch (error) { await emit({ type: 'failure', message: error?.message ?? String(error) }); }
