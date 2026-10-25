// SPDX-License-Identifier: MIT OR Apache-2.0
// Scenarios of consent and substitution (docs/stages/m2b-consent.md, "Приёмка"): what the application
// finds when the user decided before the start, and what the launcher does with the decisions.
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { existsSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, realpathSync, rmSync, statSync, writeFileSync } from 'node:fs';
import os from 'node:os';
import { join } from 'node:path';

import { prepareSite, scratch, startApp, verdictOf } from '../lib.mjs';

const CONSENT_CHECKS = [
  'consent-an-environment-variable-is-real-a-stand-in-or-denied',
  'consent-a-denial-is-the-error-of-a-right-the-manifest-never-listed',
  'consent-the-list-of-variables-has-only-what-is-really-given',
  'consent-a-substituted-address-reports-success-and-opens-nothing',
  'consent-a-command-without-a-stand-in-refuses-the-substitute',
  'consent-the-folder-of-the-runtime-is-out-of-reach-of-every-right',
  'consent-the-commands-of-the-permission-window-are-not-the-applications',
];

/** The answers of the user to every right of the `consent` application. */
const ANSWERS = [
  'app.env:ALEF_E2E_ENV_ALLOWED=allow',
  'app.env:ALEF_E2E_ENV_SUBSTITUTED=substitute',
  'app.env:ALEF_E2E_ENV_DENIED=deny',
  'app.env:ALEF_E2E_ENV_UNSET=allow',
  'shell.openExternal:https://allowed.example/*=allow',
  'shell.openExternal:https://stand-in.example/*=substitute',
  'window.create=substitute',
  'fs.read:$TEMP/**=allow',
  'fs.read:$HOME/**=allow',
];

const VARIABLES = { ALEF_E2E_ENV_ALLOWED: 'yes', ALEF_E2E_ENV_SUBSTITUTED: 'really-set', ALEF_E2E_ENV_DENIED: 'secret-31337' };

const linesOf = text => text.split('\n').filter(Boolean);

/** One hash of a whole folder: the sorted paths and the bytes and sizes of their files. */
const dirHash = root => {
  const digest = createHash('sha256');
  const walk = prefix => {
    for (const entry of readdirSync(join(root, prefix), { withFileTypes: true }).sort((a, b) => a.name < b.name ? -1 : 1)) {
      const relative = `${prefix}${entry.name}`;
      if (entry.isDirectory()) { walk(`${relative}/`); continue; }
      const bytes = readFileSync(join(root, relative));
      digest.update(`${relative}:${bytes.length}:`); digest.update(bytes);
    }
  };
  walk('');
  return digest.digest('hex');
};

/** The same file whatever the spelling: links resolved, and the case of Windows ignored. */
const real = path => {
  const resolved = realpathSync.native(path);
  return process.platform === 'win32' ? resolved.toLowerCase() : resolved;
};

/** What the pretending shell was asked, compared as the same files whatever their spelling. */
const shellJudge = (log, note) => () => {
  const asked = existsSync(log) ? linesOf(readFileSync(log, 'utf8')).map(line => JSON.parse(line)) : [];
  const spelled = ({ operation, target }) => [operation, operation === 'openPath' ? real(target) : target];
  const seen = asked.map(spelled);
  const wanted = [['openExternal', 'https://allowed.example/page'], ['openPath', real(note)]];
  return JSON.stringify(seen) === JSON.stringify(wanted)
    ? []
    : [`the shell was asked ${JSON.stringify(seen)}, expected exactly ${JSON.stringify(wanted)}`];
};

export function consentScenarios({ drive, exe, verbose }) {
  /** Runs `alef` to its end with a command line; the answers of the end-to-end runs are off unless given. */
  const cli = (args, env = {}) => {
    // A run of the application from here never shows the permission window: nobody is there to answer it.
    const argv = args[0] === '--app' ? [...args, '--no-prompt'] : args;
    const result = spawnSync(exe, argv, {
      env: { ...process.env, ALEF_E2E: '0', ...env }, encoding: 'utf8', timeout: 60000,
    });
    return { code: result.status, out: result.stdout ?? '', err: result.stderr ?? '' };
  };

  return {
    // The user chose for each right before the start; the page sees the real thing, stand-ins that give
    // nothing away, denials that look like unlisted rights, and the runtime folder stays closed to it.
    async consent() {
      const directory = mkdtempSync(join(os.tmpdir(), 'alef-e2e-consent-'));
      const home = join(directory, 'home');
      const note = join(directory, 'note.txt');
      writeFileSync(note, 'note');
      const log = join(directory, 'shell.log');
      try {
        const result = await drive({
          name: 'consent', app: 'consent', replacements: { SECRETS: 'false' },
          targets: { home, decisions: join(home, 'consent'), note },
          env: { ALEF_HOME: home, ALEF_E2E_CONSENT: ANSWERS.join(';'), ALEF_E2E_SHELL_LOG: log, ...VARIABLES },
          expectedChecks: CONSENT_CHECKS,
          judge: shellJudge(log, note),
        });
        const saved = existsSync(join(home, 'consent')) ? readdirSync(join(home, 'consent')) : [];
        if (saved.length !== 1 || !saved[0].endsWith('.ktav')) result.problems.push(`the decisions were not kept in one .ktav file: ${JSON.stringify(saved)}`);
        return result;
      } finally {
        rmSync(directory, { recursive: true, force: true });
      }
    },

    // The user takes one right back and gives another while the application runs: the first applies at
    // once, the second at the next start.
    async narrowing() {
      const problems = [];
      const directory = mkdtempSync(join(os.tmpdir(), 'alef-e2e-narrowing-'));
      const home = join(directory, 'home');
      const environment = { ALEF_HOME: home };
      const app = prepareSite('narrowing', 'consent', {
        replacements: { SECRETS: 'false' },
        targets: { mode: 'narrow', home, decisions: join(home, 'consent'), note: join(directory, 'note.txt') },
      });
      let running;
      try {
        for (const answer of ANSWERS) {
          const [right, decision] = answer.split(/=(?=[a-z]+$)/);
          const set = cli(['permissions', 'set', app, right, decision], environment);
          if (set.code !== 0) problems.push(`set ${right}: ${set.err}`);
        }
        running = startApp({ exe, args: ['--app', app], env: { ...environment, ALEF_E2E_CONSENT: '', ...VARIABLES }, verbose });
        await running.waitFor(line => line.includes('ALEF_E2E narrowing-ready'), 120000, 'the page to be ready');
        const widened = cli(['permissions', 'set', app, 'app.env:ALEF_E2E_ENV_DENIED', 'allow'], environment);
        const narrowed = cli(['permissions', 'set', app, 'app.env:ALEF_E2E_ENV_ALLOWED', 'deny'], environment);
        for (const set of [widened, narrowed]) if (set.code !== 0) problems.push(`set while running: ${set.err}`);
        await running.waitFor(line => line.includes('ALEF_E2E RESULT'), 60000, 'the verdict of the page');
        const result = verdictOf(running.lines);
        if (result?.[1] !== 'PASS') problems.push(`verdict ${result?.[1]}: ${result?.[2]}`);
        if (!running.lines.some(line => / check consent-a-right-taken-back-applies-at-once-and-a-right-given-waits-for-the-next-start ok /.test(line))) {
          problems.push('the check of the page has no ok line');
        }
        const exit = await running.waitForExit(30000);
        if (exit?.code !== 0) problems.push(`the run ended with ${exit?.code ?? 'no exit'}`);
      } catch (error) {
        problems.push(error.message);
      } finally {
        running?.stop();
        rmSync(directory, { recursive: true, force: true });
      }
      return { problems, lines: running?.lines ?? [] };
    },

    // The permission window: a user's clicks played by the page itself decide every right; the window
    // shows all of them, flags the risky one and does not let it start before the risk is confirmed;
    // then the application starts and finds what was clicked.
    async ask() {
      const directory = mkdtempSync(join(os.tmpdir(), 'alef-e2e-ask-'));
      const home = join(directory, 'home');
      const note = join(directory, 'note.txt');
      writeFileSync(note, 'note');
      const log = join(directory, 'shell.log');
      const risky = 'fs.read:$HOME/**';
      const steps = [
        { report: true },
        { all: 'deny' },
        ...ANSWERS.map(answer => {
          const [right, decision] = answer.split(/=(?=[a-z]+$)/);
          return { choose: right, decision };
        }),
        { report: true },
        { confirm: risky },
        { report: true },
        { submit: true },
      ];
      try {
        const result = await drive({
          name: 'ask', app: 'consent', replacements: { SECRETS: 'false' },
          targets: { home, decisions: join(home, 'consent'), note },
          env: {
            ALEF_HOME: home, ALEF_E2E_CONSENT: undefined, ALEF_E2E_CONSENT_UI: JSON.stringify(steps),
            ALEF_E2E_CONSENT_HOLD_MS: '3000', ALEF_E2E_SHELL_LOG: log, ...VARIABLES,
          },
          expectedChecks: CONSENT_CHECKS,
          judge: lines => {
            const problems = shellJudge(log, note)();
            const pending = lines.map((line, index) => line.includes('ALEF_E2E consent-pending') ? index : -1).filter(index => index >= 0);
            const released = lines.map((line, index) => line.includes('ALEF_E2E consent-released') ? index : -1).filter(index => index >= 0);
            const releaseMatch = lines.map(line => /^ALEF_E2E consent-released elapsed_ms=(\d+)$/.exec(line)).find(Boolean);
            if (pending.length !== 1) problems.push(`consent-pending occurred ${pending.length} times, expected 1`);
            if (released.length !== 1) problems.push(`consent-released occurred ${released.length} times, expected 1`);
            if (!releaseMatch) problems.push('consent-released did not include an exact elapsed_ms duration');
            else {
              const elapsed = Number(releaseMatch[1]);
              if (elapsed < 3000 || elapsed > 10000) problems.push(`consent hold elapsed ${elapsed}ms, expected 3000–10000ms`);
            }
            const firstUi = lines.findIndex(line => line.startsWith('ALEF_E2E consent-ui '));
            if (pending.length === 1 && released.length === 1 && !(pending[0] < released[0] && released[0] < firstUi)) {
              problems.push('consent-pending and consent-released were not ordered before the first consent-ui report');
            }
            const reports = lines
              .map(line => /^ALEF_E2E consent-ui rows=(\d+) risky=(\[.*?\]) start=(\w+) picked=(\[.*\])$/.exec(line))
              .filter(Boolean)
              .map(([, rows, flagged, start, picked]) => ({ rows: Number(rows), flagged: JSON.parse(flagged), start, picked: JSON.parse(picked) }));
            if (reports.length !== 3) return [...problems, `the window reported ${reports.length} times, expected 3`];
            const [opened, unconfirmed, confirmed] = reports;
            if (opened.rows !== ANSWERS.length) problems.push(`the window shows ${opened.rows} rights, the manifest asks for ${ANSWERS.length}`);
            if (JSON.stringify(opened.flagged) !== JSON.stringify([risky])) problems.push(`risky rights flagged: ${JSON.stringify(opened.flagged)}`);
            if (opened.start !== 'disabled' || !opened.picked.every(item => item.endsWith('=none'))) problems.push('the window lets the application start before anything is decided');
            if (unconfirmed.start !== 'disabled') problems.push('a risky right allowed without its confirmation lets the application start');
            if (confirmed.start !== 'enabled') problems.push('everything is decided and confirmed, yet the application cannot start');
            const saved = existsSync(join(home, 'consent')) ? readdirSync(join(home, 'consent')) : [];
            if (saved.length !== 1 || !saved[0].endsWith('.ktav')) return [...problems, `the decisions were not kept in one .ktav file: ${JSON.stringify(saved)}`];
            // No Ktav parser in the runner: the decisions are checked through the CLI, which reads the same store.
            const listed = cli(['permissions', 'list', join(scratch, 'sites', 'ask')], { ALEF_HOME: home }).out;
            const kept = listed.split('\n').map(line => /^  (\w+)\t(\S+)/.exec(line)).filter(Boolean)
              .filter(([, word]) => word !== 'undecided').map(([, word, right]) => `${right}=${word}`).sort();
            if (JSON.stringify(kept) !== JSON.stringify([...ANSWERS].sort())) problems.push(`the store holds ${JSON.stringify(kept)}`);
            return problems;
          },
        });
        return result;
      } finally {
        rmSync(directory, { recursive: true, force: true });
      }
    },

    // Closing the window without deciding: the application does not start and nothing is kept.
    async cancel() {
      const problems = [];
      const directory = mkdtempSync(join(os.tmpdir(), 'alef-e2e-cancel-'));
      const home = join(directory, 'home');
      const app = prepareSite('cancel', 'consent', { replacements: { SECRETS: 'false' } });
      let running;
      try {
        running = startApp({
          exe, args: ['--app', app], verbose,
          env: { ALEF_HOME: home, ALEF_E2E_CONSENT: undefined, ALEF_E2E_CONSENT_UI: JSON.stringify([{ cancel: true }]) },
        });
        const exit = await running.waitForExit(120000);
        if (exit?.code !== 3) problems.push(`the run ended with ${exit?.code ?? 'no exit'}, expected 3`);
        if (!running.lines.some(line => line.includes('closed without a decision'))) problems.push('the refusal does not say that the window was closed');
        if (running.lines.some(line => line.startsWith('ALEF_READY'))) problems.push('the application started without a decision');
        if (existsSync(join(home, 'consent'))) problems.push('a decision was kept although none was made');
      } catch (error) {
        problems.push(error.message);
      } finally {
        running?.stop();
        rmSync(directory, { recursive: true, force: true });
      }
      return { problems, lines: running?.lines ?? [] };
    },

    // The launcher: no decision, no start; the user decides from the command line; a decision is kept;
    // a manifest that asks for more is asked again; `--grant`; `reset`; another folder inherits nothing.
    async decisions() {
      const problems = [];
      const directory = mkdtempSync(join(os.tmpdir(), 'alef-e2e-decisions-'));
      const home = join(directory, 'home');
      const note = join(directory, 'note.txt');
      writeFileSync(note, 'note');
      const log = join(directory, 'shell.log');
      const environment = { ALEF_HOME: home };
      const expect = (what, condition, detail = '') => {
        console.log(`    ${condition ? 'ok  ' : 'FAIL'} ${what}${condition ? '' : `: ${detail}`}`);
        if (!condition) problems.push(`${what}: ${detail}`);
      };
      const site = secrets => prepareSite('decisions', 'consent', { replacements: { SECRETS: secrets } });
      // The page of the consent scenario, started with no answers at all: whatever it finds is what was decided before.
      const run = (secrets, extra = []) => drive({
        name: 'decisions', app: 'consent', replacements: { SECRETS: secrets }, args: extra,
        targets: { home, decisions: join(home, 'consent'), note },
        env: { ...environment, ALEF_E2E_CONSENT: '', ALEF_E2E_SHELL_LOG: log, ...VARIABLES },
        expectedChecks: CONSENT_CHECKS,
      });
      try {
        let app = site('false');

        const refused = cli(['--app', app], environment);
        expect('an application whose rights nobody decided does not start', refused.code === 3, `exit ${refused.code}: ${refused.err}`);
        expect('the refusal names the rights and the way out',
          refused.err.includes('app.env:ALEF_E2E_ENV_ALLOWED') && refused.err.includes('window.create') && refused.err.includes('--grant'), refused.err);
        expect('the refusal opens no window and writes no decision', !refused.err.includes('ALEF_READY') && !existsSync(join(home, 'consent')), refused.err);

        const before = cli(['permissions', 'list', app], environment);
        expect('list shows every right undecided',
          before.code === 0 && linesOf(before.out).filter(line => line.includes('undecided')).length === ANSWERS.length, before.out);
        expect('list without an application has nothing yet', cli(['permissions', 'list'], environment).out.includes('no decisions yet'));

        for (const answer of ANSWERS) {
          const [right, decision] = answer.split(/=(?=[a-z]+$)/);
          const set = cli(['permissions', 'set', app, right, decision], environment);
          if (set.code !== 0) problems.push(`set ${right}: ${set.err}`);
        }
        const unknown = cli(['permissions', 'set', app, 'secrets', 'allow'], environment);
        expect('a right the manifest does not ask for cannot be decided', unknown.code === 2 && unknown.err.includes('does not ask for secrets'), unknown.err);
        const listed = cli(['permissions', 'list', app], environment).out;
        expect('list shows what was decided',
          listed.includes('substitute\tapp.env:ALEF_E2E_ENV_SUBSTITUTED') && listed.includes('deny\tapp.env:ALEF_E2E_ENV_DENIED') && !listed.includes('undecided'), listed);
        expect('list without an application names it',
          cli(['permissions', 'list'], environment).out.startsWith('org.alef.e2e.modules.consent\t'));

        // The decisions are kept: the start asks nothing, the stand-ins and denials hold.
        const kept = await run('false');
        expect('the decisions made from the command line hold at the next start', kept.problems.length === 0, kept.problems.join('; '));
        const asked = shellJudge(log, note)();
        expect('and the stand-in opened nothing', asked.length === 0, asked.join('; '));

        // The manifest asks for one more right: only that one is asked.
        app = site('true');
        const more = cli(['--app', app], environment);
        expect('a right the manifest added is asked again, and only that one',
          more.code === 3 && more.err.includes('secrets') && !more.err.includes('app.env'), `exit ${more.code}: ${more.err}`);

        // `--grant` decides what is not decided yet and keeps what was.
        const granted = await run('true', ['--grant', 'deny']);
        expect('--grant lets the application start with the new right decided', granted.problems.length === 0, granted.problems.join('; '));
        const after = cli(['permissions', 'list', app], environment).out;
        expect('the new right got the granted decision and the old ones were kept',
          after.includes('deny\tsecrets') && after.includes('substitute\tapp.env:ALEF_E2E_ENV_SUBSTITUTED') && after.includes('allow\tapp.env:ALEF_E2E_ENV_ALLOWED'), after);

        // Another folder with the same id inherits nothing.
        const other = prepareSite('decisions-other', 'consent', { replacements: { SECRETS: 'true' } });
        const inherited = cli(['permissions', 'list', other], environment).out;
        expect('another folder with the same id has decisions of its own', !inherited.includes('allow\t') && !inherited.includes('deny\t'), inherited);

        // `reset`: the next start asks again.
        const reset = cli(['permissions', 'reset', app], environment);
        expect('reset forgets the decisions and the next start asks again',
          reset.code === 0 && cli(['--app', app], environment).code === 3, reset.out + reset.err);
        expect('and the application is gone from the list', cli(['permissions', 'list'], environment).out.includes('no decisions yet'));
      } catch (error) {
        problems.push(error.message);
      } finally {
        rmSync(directory, { recursive: true, force: true });
      }
      return { problems, lines: [] };
    },
  };

}

// The manager window (`alef apps --ui`): the list, narrowing at once, the red confirmation and the
// reset, checked against the same store the command line reads.
const managerManifest = (id, right) => `id: ${id}
name: ${id}
version:: 1.0.0
windows: [
    {
        label: main
        url: /index.html
        width: 800
        height: 600
    }
]
external: {
    connect: []
    load: {
        scripts: []
        styles: []
        images: []
        fonts: []
        media: []
        frames: []
    }
}
permissions: {
    fs: {
        read: []
        write: []
    }
    cli: {
        exec: [ sidecar:tool ]
    }
    net: {
        http: []
        socket: []
    }
    shell: {
        openExternal: []
    }
    clipboard: {
        read: true
    }
    shortcut: {
        global: false
    }
    app: {
        env: []
    }
    ${right.startsWith('secrets:') ? '' : 'secrets: false\n    '}${right}
}
`;

export function appsUiScenarios({ exe, verbose }) {
  return {
    async 'apps-ui'() {
      const problems = [];
      const directory = mkdtempSync(join(os.tmpdir(), 'alef-apps-ui-'));
      const home = join(directory, 'home');
      const folders = ['alpha', 'beta'].map(name => join(directory, name));
      const [appA, appB] = folders;
      const red = 'cli.exec:sidecar:tool';
      mkdirSync(home, { recursive: true });
      folders.forEach(folder => mkdirSync(folder, { recursive: true }));
      writeFileSync(join(appA, 'alef.ktav'), managerManifest('org.example.apps.alpha', 'secrets: true'));
      writeFileSync(join(appB, 'alef.ktav'), managerManifest('org.example.apps.beta', 'tray: true'));
      const cli = args => spawnSync(exe, args, { encoding: 'utf8', timeout: 30000, env: { ...process.env, ALEF_HOME: home, ALEF_E2E: '0' } });
      let running;
      try {
        // The store is the CLI's own: the directory form loads the manifest of the folder, the
        // seeded decisions are what the window and `permissions list` both read.
        for (const [app, right, decision] of [[appA, 'clipboard.read', 'allow'], [appA, red, 'deny'], [appB, 'clipboard.read', 'substitute'], [appB, 'tray', 'deny']]) {
          const seeded = cli(['permissions', 'set', app, right, decision]);
          if (seeded.status !== 0) problems.push(`seed ${right}: ${seeded.stderr}`);
        }
        const steps = [
          { snapshot: 'initial' }, { search: 'no-such-app' }, { snapshot: 'search-empty' }, { search: '' },
          { select: 'org.example.apps.alpha' }, { snapshot: 'alpha' },
          { negativeRed: { permission: 'cli.exec', scope: 'sidecar:tool' } },
          { choose: 'clipboard.read', decision: 'deny' }, { save: 'clipboard.read' }, { snapshot: 'narrowed' },
          { choose: red, decision: 'allow' }, { save: red }, { snapshot: 'red-unconfirmed' },
          { confirm: red }, { save: red }, { snapshot: 'red-allowed' },
          { reset: true }, { snapshot: 'reset' }, { close: true },
        ];
        running = startApp({ exe, args: ['apps', '--ui'], env: { ALEF_HOME: home, ALEF_E2E: '1', ALEF_E2E_APPS_UI: JSON.stringify(steps) }, verbose });
        await running.waitFor(line => line.includes('red-confirmation-required'), 120000, 'red negative-control report');
        const exit = await running.waitForExit(120000);
        if (exit?.code !== 0) problems.push(`the window ended with ${exit?.code ?? 'no exit'}`);
        const reports = running.lines.filter(line => line.startsWith('ALEF_E2E '))
          .map(line => { try { return JSON.parse(line.slice('ALEF_E2E '.length)); } catch { return null; } })
          .filter(Boolean);
        problems.push(...reports.filter(report => report.type === 'failure').map(report => `UI automation: ${report.message}`));
        const snapshots = new Map(reports.filter(report => report.type === 'snapshot').map(report => [report.name, report]));
        for (const name of ['initial', 'search-empty', 'alpha', 'narrowed', 'red-unconfirmed', 'red-allowed', 'reset']) {
          if (!snapshots.has(name)) problems.push(`missing snapshot ${name}`);
        }
        if (snapshots.get('initial')?.list?.length !== 2) problems.push('initial list should include both applications');
        if (snapshots.get('search-empty')?.list?.length !== 0) problems.push('search should return an empty result');
        if (snapshots.get('alpha')?.rows?.length !== 3) problems.push('alpha should display three rights');
        const seededRow = snapshots.get('alpha')?.rows?.find(item => item.right === red);
        if (seededRow?.checked !== 'deny' || seededRow?.red !== true) problems.push('alpha should show the red right denied and flagged');
        if (!reports.some(report => report.name === 'red-confirmation-required' && report.ok)) problems.push('unconfirmed red expansion was not rejected');
        for (const [name, right, expected] of [['narrowed', 'clipboard.read', 'deny'], ['red-allowed', red, 'allow']]) {
          const row = snapshots.get(name)?.rows?.find(item => item.right === right);
          if (row?.checked !== expected) problems.push(`${name}: expected ${right}=${expected}, got ${row?.checked}`);
        }
        const unconfirmed = snapshots.get('red-unconfirmed')?.rows?.find(item => item.right === red);
        if (!unconfirmed?.error?.includes('confirm-red')) problems.push(`the unconfirmed red save did not show the policy refusal: ${unconfirmed?.error}`);
        if (snapshots.get('red-allowed')?.rows?.find(item => item.right === red)?.error !== '') problems.push('the confirmed red save left an error shown');
        if (snapshots.get('reset')?.rows?.some(row => row.checked !== null)) problems.push('reset should leave rights undecided');
        const after = cli(['permissions', 'list', appA]);
        if (after.status !== 0 || !after.stdout.includes('undecided\tclipboard.read') || !after.stdout.includes('undecided\tsecrets') || !after.stdout.includes(`undecided\t${red}`)) {
          problems.push(`CLI store mismatch after reset: ${after.stdout}${after.stderr}`);
        }
      } catch (error) { problems.push(error.message); }
      finally { running?.stop(); rmSync(directory, { recursive: true, force: true }); }
      return { problems, lines: [] };
    },
  };
}

// The data of an application (`alef apps export`/`import` and the buttons of the manager window):
// an archive of the data folder, the red confirmation before it replaces anything, a refusal while
// the application runs, and the refusal of an archive that lies about its paths.
const DATA_ID = 'org.alef.e2e.modules.store';
const DATA_APP = 'modules/data/store'; // the store fixture: its run writes the `store` file of the data folder

/** CRC-32 (the zip polynomial) of a buffer. */
const crc32 = bytes => {
  let table = crc32.table;
  if (!table) {
    table = crc32.table = new Int32Array(256);
    for (let n = 0; n < 256; n++) {
      let c = n;
      for (let k = 0; k < 8; k++) c = c & 1 ? 0xEDB88320 ^ (c >>> 1) : c >>> 1;
      table[n] = c;
    }
  }
  let crc = -1;
  for (const byte of bytes) crc = table[(crc ^ byte) & 0xFF] ^ (crc >>> 8);
  return (crc ^ -1) >>> 0;
};

/**
 * A minimal STORE-method zip whose every entry lies about where it goes (`name` is used as it is,
 * so `../` escapes): the local header, the central directory and the end record, hand-rolled.
 */
const slipZip = names => {
  const parts = [];
  const central = [];
  let offset = 0;
  for (const [name, content] of names) {
    const label = Buffer.from(name, 'utf8');
    const bytes = Buffer.from(content, 'utf8');
    const check = crc32(bytes);
    const local = Buffer.alloc(30);
    local.writeUInt32LE(0x04034b50, 0); local.writeUInt16LE(20, 4); local.writeUInt16LE(0, 6);
    local.writeUInt16LE(0, 8); local.writeUInt16LE(0, 10); local.writeUInt16LE(0, 12);
    local.writeUInt32LE(check, 14); local.writeUInt32LE(bytes.length, 18); local.writeUInt32LE(bytes.length, 22);
    local.writeUInt16LE(label.length, 26); local.writeUInt16LE(0, 28);
    parts.push(local, label, bytes);
    const entry = Buffer.alloc(46);
    entry.writeUInt32LE(0x02014b50, 0); entry.writeUInt16LE(20, 4); entry.writeUInt16LE(20, 6);
    entry.writeUInt16LE(0, 8); entry.writeUInt16LE(0, 10); entry.writeUInt16LE(0, 12); entry.writeUInt16LE(0, 14);
    entry.writeUInt32LE(check, 16); entry.writeUInt32LE(bytes.length, 20); entry.writeUInt32LE(bytes.length, 24);
    entry.writeUInt16LE(label.length, 28); entry.writeUInt16LE(0, 30); entry.writeUInt16LE(0, 32);
    entry.writeUInt16LE(0, 34); entry.writeUInt16LE(0, 36); entry.writeUInt32LE(0, 38); entry.writeUInt32LE(offset, 42);
    central.push(entry, label);
    offset += 30 + label.length + bytes.length;
  }
  const directory = Buffer.concat(central);
  const end = Buffer.alloc(22);
  end.writeUInt32LE(0x06054b50, 0); end.writeUInt16LE(0, 4); end.writeUInt16LE(0, 6);
  end.writeUInt16LE(names.length, 8); end.writeUInt16LE(names.length, 10);
  end.writeUInt32LE(directory.length, 12); end.writeUInt32LE(offset, 16); end.writeUInt16LE(0, 20);
  return Buffer.concat([...parts, directory, end]);
};

export function appsDataScenarios({ exe, verbose }) {
  return {
    // Export makes an archive of the data folder; import replaces it only after the red
    // confirmation, keeps the replaced data aside, refuses while the application runs and refuses
    // an archive that is broken or that writes outside the folder; the manager window offers the
    // same two buttons, driven by the page automation with the native dialogs answered from env.
    async 'apps-data'() {
      const problems = [];
      const directory = mkdtempSync(join(os.tmpdir(), 'alef-apps-data-'));
      const home = join(directory, 'home');
      const appHome = join(directory, 'apphome');
      const data = join(appHome, 'data', DATA_ID); // mirror of application_base_dirs: <ALEF_APP_HOME>/data/<id>
      const first = join(home, 'first.zip');
      const ui = join(home, 'ui.zip');
      const environment = { ALEF_HOME: home, ALEF_APP_HOME: appHome };
      const cli = args => {
        const result = spawnSync(exe, args, { encoding: 'utf8', timeout: 60000, env: { ...process.env, ALEF_E2E: '0', ...environment } });
        return { code: result.status, out: result.stdout ?? '', err: result.stderr ?? '' };
      };
      const expect = (what, condition, detail = '') => {
        console.log(`    ${condition ? 'ok  ' : 'FAIL'} ${what}${condition ? '' : `: ${detail}`}`);
        if (!condition) problems.push(`${what}: ${detail}`);
      };
      // The page of the run: `write` fills the store and quits; `hold` reports ready and stays.
      const page = mode => `import { api, report } from './harness.js';
${mode === 'hold' ? "await report('appsdata-holding');\n" : `await api.store.set('apps-data', 'first');\nawait report('appsdata-written');\nawait api.app.quit(0);\n`}`;
      const site = prepareSite('apps-data', DATA_APP);
      writeFileSync(join(site, 'dist', 'page.js'), '');
      // `clipboard.read` becomes a right the manifest asks for: seeding a decision for it is what
      // puts the application into the list of the manager window (the same store both read).
      const manifest = readFileSync(join(site, 'alef.ktav'), 'utf8');
      if (!manifest.includes('read: false')) { problems.push('the fixture manifest has no `read: false` to turn into an asked right'); }
      writeFileSync(join(site, 'alef.ktav'), manifest.replace('read: false', 'read: true'));
      const run = async mode => {
        writeFileSync(join(site, 'dist', 'page.js'), page(mode));
        const running = startApp({
          exe, args: ['--app', site], verbose,
          env: { ...process.env, ALEF_E2E: '1', ...environment },
        });
        try {
          await running.waitFor(line => line.includes(`appsdata-${mode === 'hold' ? 'holding' : 'written'}`), 120000, `the page of the ${mode} run`, { survivesExit: true });
          if (mode !== 'hold') {
            const exit = await running.waitForExit(30000);
            if (exit?.code !== 0) problems.push(`the ${mode} run ended with ${exit?.code ?? 'no exit'}`);
          }
        } catch (error) { problems.push(`${mode} run: ${error.message}`); }
        return running;
      };
      let holding;
      try {
        const seeded = cli(['permissions', 'set', site, 'clipboard.read', 'allow']);
        if (seeded.code !== 0) problems.push(`seed clipboard.read: ${seeded.err}`);

        // The application runs once and leaves its data behind: the store file of the page plus a
        // plain file, both in the data folder that ALEF_APP_HOME names.
        await run('write');
        writeFileSync(join(data, 'note.txt'), 'alef apps-data note');
        expect('the data folder holds the store and the note', existsSync(join(data, 'store')) && existsSync(join(data, 'note.txt')), dirHash(data));
        const original = dirHash(data);

        // Export: an archive that is not empty, and the data are untouched.
        const exported = cli(['apps', 'export', site, first]);
        expect('export succeeds', exported.code === 0 && /Exported \d+ files/.test(exported.out), `${exported.code}: ${exported.out}${exported.err}`);
        expect('the archive is not empty', existsSync(first) && statSync(first).size > 0, first);

        // The data change after the export: the import brings the old state back.
        writeFileSync(join(data, 'extra.txt'), 'written after the export');

        // Without the red confirmation nothing happens.
        const unconfirmed = cli(['apps', 'import', site, first]);
        expect('an import without --confirm-red is refused with exit 2', unconfirmed.code === 2, `exit ${unconfirmed.code}: ${unconfirmed.out}${unconfirmed.err}`);
        expect('the refusal names the way out', `${unconfirmed.out}${unconfirmed.err}`.includes('--confirm-red'), unconfirmed.out + unconfirmed.err);
        expect('the data are unchanged after the refusal', dirHash(data) !== original && existsSync(join(data, 'extra.txt')), dirHash(data));

        const confirmed = cli(['apps', 'import', site, first, '--confirm-red']);
        expect('the confirmed import succeeds', confirmed.code === 0 && confirmed.out.includes('Imported'), `${confirmed.code}: ${confirmed.out}${confirmed.err}`);
        expect('the replaced data are kept aside', confirmed.out.includes('Previous data kept in') && existsSync(`${data}.before-import`), confirmed.out);
        expect('the import restored the exported data', dirHash(data) === original, dirHash(data));

        // The import refuses while the application is running.
        const beforeRunning = dirHash(data);
        holding = await run('hold');
        const refused = cli(['apps', 'import', site, first, '--confirm-red']);
        expect('an import while the application runs is refused', refused.code !== 0 && /running/.test(`${refused.out}${refused.err}`), `${refused.code}: ${refused.out}${refused.err}`);
        expect('the data are unchanged after the refusal of a running application', dirHash(data) === beforeRunning, dirHash(data));
        holding.stop();
        const closed = await holding.waitForExit(30000);
        if (closed === undefined) problems.push('the holding application did not close');
        holding = undefined;

        // A broken archive and an archive that writes outside the folder are both refused.
        const broken = Buffer.from(readFileSync(first));
        broken[Math.floor(broken.length / 2)] ^= 0xFF;
        const brokenPath = join(home, 'broken.zip');
        writeFileSync(brokenPath, broken);
        const beforeBroken = dirHash(data);
        const corrupted = cli(['apps', 'import', site, brokenPath, '--confirm-red']);
        expect('a corrupted archive is refused', corrupted.code !== 0, `${corrupted.code}: ${corrupted.out}${corrupted.err}`);
        expect('the data are unchanged after the corrupted archive', dirHash(data) === beforeBroken, dirHash(data));
        const slipPath = join(home, 'slip.zip');
        writeFileSync(slipPath, slipZip([['../evil.txt', 'outside'], ['alef-data.ktav', 'format: 1\n']]));
        const slip = cli(['apps', 'import', site, slipPath, '--confirm-red']);
        expect('a zip-slip archive is refused', slip.code !== 0, `${slip.code}: ${slip.out}${slip.err}`);
        expect('the data are unchanged after the zip-slip archive', dirHash(data) === beforeBroken, dirHash(data));
        expect('no file escaped the folder', !existsSync(join(appHome, 'data', 'evil.txt')) && !existsSync(join(appHome, 'evil.txt')), appHome);

        // The manager window: the same export and the red-confirmed import, the native dialogs
        // answered from ALEF_E2E_DIALOGS in order (three answers: save for the export, then two opens for the two import attempts).
        const steps = [
          { select: DATA_ID }, { stability: DATA_ID },
          { export: DATA_ID }, { snapshot: 'exported' },
          { import: DATA_ID, confirm: false },
          { import: DATA_ID, confirm: true },
          { snapshot: 'imported' }, { close: true },
        ];
        const uiRunning = startApp({
          exe, args: ['apps', '--ui'], verbose,
          env: {
            ...process.env, ALEF_E2E: '1', ...environment,
            ALEF_E2E_APPS_UI: JSON.stringify(steps),
            ALEF_E2E_DIALOGS: JSON.stringify([{ save: ui }, { open: [ui] }, { open: [ui] }]),
          },
        });
        try {
          const exit = await uiRunning.waitForExit(120000);
          if (exit?.code !== 0) problems.push(`the manager window ended with ${exit?.code ?? 'no exit'}`);
          const reports = uiRunning.lines.filter(line => line.startsWith('ALEF_E2E '))
            .map(line => { try { return JSON.parse(line.slice('ALEF_E2E '.length)); } catch { return null; } })
            .filter(Boolean);
          problems.push(...reports.filter(report => report.type === 'failure').map(report => `UI automation: ${report.message}`));
          const ok = name => reports.some(report => report.type === 'check' && report.name === name && report.ok);
          for (const name of ['export-ok', 'import-confirmation-required', 'import-ok', 'selection-node-stable', 'details-no-flash']) {
            if (!ok(name)) problems.push(`the window never reported ${name} ok`);
          }
          expect('the window export wrote an archive', existsSync(ui) && statSync(ui).size > 0, ui);
          // Cross-check through the CLI: the archive of the window holds the exported data, and the
          // import that the window confirmed left them in place.
          expect('the windowed import left the exported data in place', dirHash(data) === original, dirHash(data));
        } catch (error) { problems.push(`manager window: ${error.message}`); }
      } catch (error) { problems.push(error.message); }
      finally { holding?.stop(); rmSync(directory, { recursive: true, force: true }); }
      return { problems, lines: [] };
    },
  };
}
