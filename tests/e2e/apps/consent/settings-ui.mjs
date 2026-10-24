// SPDX-License-Identifier: MIT OR Apache-2.0
// The `alef settings --ui` window: the automation drives language choice through JSON steps and
// judges the snapshots the page reports and the settings the runtime persists.
import { mkdirSync, mkdtempSync, readFileSync, rmSync } from 'node:fs';
import os from 'node:os';
import { join } from 'node:path';

import { startApp } from '../../lib.mjs';

export function settingsUiScenarios({ exe, verbose }) {
  return {
    async 'settings-ui'() {
      const problems = [];
      const directory = mkdtempSync(join(os.tmpdir(), 'alef-settings-ui-'));
      const home = join(directory, 'home');
      mkdirSync(home, { recursive: true });
      const steps = [
        { snapshot: 'initial' }, { choose: 'ru' }, { snapshot: 'ru' },
        { choose: 'ar' }, { snapshot: 'ar' }, { close: true },
      ];
      let running;
      try {
        running = startApp({ exe, args: ['settings', '--ui'], env: { ALEF_HOME: home, ALEF_E2E: '1', ALEF_E2E_SETTINGS_UI: JSON.stringify(steps) }, verbose });
        const exit = await running.waitForExit(120000);
        if (exit?.code !== 0) problems.push(`the window ended with ${exit?.code ?? 'no exit'}`);
        const reports = running.lines.filter(line => line.startsWith('ALEF_E2E '))
          .map(line => { try { return JSON.parse(line.slice('ALEF_E2E '.length)); } catch { return null; } })
          .filter(Boolean);
        problems.push(...reports.filter(report => report.type === 'failure').map(report => `UI automation: ${report.message}`));
        const snapshots = new Map(reports.filter(report => report.type === 'snapshot').map(report => [report.name, report]));
        const languages = ['English', '中文', 'Русский', 'Español', 'العربية', 'עברית'];
        for (const name of ['initial', 'ru', 'ar']) {
          if (!snapshots.has(name)) problems.push(`missing snapshot ${name}`);
        }
        const initial = snapshots.get('initial');
        if (initial?.language !== 'en') problems.push(`initial language should be en, got ${initial?.language}`);
        if (initial?.dir !== 'ltr') problems.push(`initial direction should be ltr, got ${initial?.dir}`);
        if (JSON.stringify(initial?.options) !== JSON.stringify(languages)) problems.push(`initial options mismatch: ${initial?.options}`);
        if (initial?.selected !== 'en') problems.push(`initial selection should be en, got ${initial?.selected}`);
        if (!initial?.heading) problems.push('initial heading is empty');
        const ru = snapshots.get('ru');
        if (ru?.language !== 'ru' || ru?.selected !== 'ru') problems.push('ru: wrong language or selection');
        if (ru?.dir !== 'ltr') problems.push('ru: direction should stay ltr');
        if (ru?.heading !== 'Язык') problems.push(`ru: heading should be Язык, got ${ru?.heading}`);
        if (ru?.title !== 'Настройки') problems.push(`ru: title should be Настройки, got ${ru?.title}`);
        if (JSON.stringify(ru?.options) !== JSON.stringify(languages)) problems.push('ru: options should not change');
        const ar = snapshots.get('ar');
        if (ar?.language !== 'ar' || ar?.selected !== 'ar') problems.push('ar: wrong language or selection');
        if (ar?.dir !== 'rtl') problems.push('ar: direction should be rtl');
        const stored = readFileSync(join(home, 'settings.ktav'), 'utf8');
        if (!stored.includes('language: ar')) problems.push('settings.ktav should record the last saved language: ar');
      } catch (error) { problems.push(error.message); }
      finally { running?.stop(); rmSync(directory, { recursive: true, force: true }); }
      return { problems, lines: [] };
    },
  };
}
