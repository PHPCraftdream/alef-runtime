// SPDX-License-Identifier: MIT OR Apache-2.0
// A repeated launch of `alef settings --ui` must not open a second window: the first process raises
// its window, the second exits quietly, and after the first one stops a fresh window opens again.
import { mkdirSync, mkdtempSync, rmSync } from 'node:fs';
import os from 'node:os';
import { join } from 'node:path';

import { startApp } from '../../lib.mjs';

const snapshotReport = line => {
  if (!line.startsWith('ALEF_E2E ')) return null;
  try { return JSON.parse(line.slice('ALEF_E2E '.length)); } catch { return null; }
};

export function singletonScenarios({ exe, verbose }) {
  return {
    async 'window-singleton'() {
      const problems = [];
      const directory = mkdtempSync(join(os.tmpdir(), 'alef-window-singleton-'));
      const home = join(directory, 'home');
      mkdirSync(home, { recursive: true });
      const env = { ALEF_HOME: home, ALEF_E2E: '1', ALEF_E2E_SETTINGS_UI: JSON.stringify([{ snapshot: 'initial' }]) };
      let first;
      let second;
      let third;
      try {
        first = startApp({ exe, args: ['settings', '--ui'], env, verbose });
        const deadline = Date.now() + 30000;
        while (!first.lines.some(line => snapshotReport(line)?.type === 'snapshot')) {
          if (Date.now() > deadline) { problems.push('the first window never reported its snapshot'); break; }
          await new Promise(resolvePromise => setTimeout(resolvePromise, 100));
        }
        second = startApp({ exe, args: ['settings', '--ui'], env, verbose });
        const exit = await second.waitForExit(3000);
        if (exit?.code !== 0) {
          problems.push(`the second launch did not exit with 0 within 3 s (${exit?.code ?? 'still running'})`);
          second.stop();
          await second.waitForExit(30000);
        }
        const echoes = second.lines.map(snapshotReport).filter(Boolean);
        if (second.lines.some(line => line.includes('ALEF_READY'))) problems.push('the second launch printed ALEF_READY');
        if (echoes.some(report => report.type === 'snapshot' || report.type === 'failure')) problems.push('the second launch opened its own window');
        if (!first.lines.some(line => line.includes('ALEF_E2E raise-received'))) problems.push('the first window did not report raise-received');
        first.stop();
        await first.waitForExit(30000);
        first = undefined;
        third = startApp({ exe, args: ['settings', '--ui'], env: { ...env, ALEF_E2E_SETTINGS_UI: JSON.stringify([{ snapshot: 'initial' }, { close: true }]) }, verbose });
        const thirdExit = await third.waitForExit(60000);
        if (thirdExit?.code !== 0) problems.push(`the third launch ended with ${thirdExit?.code ?? 'no exit'}`);
        if (!third.lines.some(line => snapshotReport(line)?.type === 'snapshot')) problems.push('the third launch never reported a snapshot: the endpoint was not released');
      } catch (error) { problems.push(error.message); }
      finally {
        first?.stop();
        second?.stop();
        third?.stop();
        rmSync(directory, { recursive: true, force: true });
      }
      return { problems, lines: [] };
    },
  };
}
