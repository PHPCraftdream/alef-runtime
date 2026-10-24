// SPDX-License-Identifier: MIT OR Apache-2.0
// node --test scripts/ci/no-extract-detect.test.mjs: the pure detector of the `no-extract` e2e
// scenario provably finds a copied package member and ignores unrelated files. No processes.
import { mkdtempSync, rmSync, writeFileSync, mkdirSync } from 'node:fs';
import { createHash } from 'node:crypto';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import assert from 'node:assert/strict';

import { extractedMembers } from '../../tests/e2e/lib.mjs';
import { test } from 'node:test';

const hash = bytes => createHash('sha256').update(bytes).digest('hex');

test('the no-extract detector finds a copied member by content and name, and nothing else', () => {
  const dir = mkdtempSync(join(tmpdir(), 'no-extract-detect-'));
  try {
    const members = [
      { name: 'dist/index.html', hash: hash(Buffer.from('member one')) },
      { name: 'dist/page.js', hash: hash(Buffer.from('member two')) },
    ];

    // (a) a file whose content equals member 1 under an unrelated name: found by hash.
    writeFileSync(join(dir, 'copied.bin'), Buffer.from('member one'));
    // (b) unrelated content with a non-member name: never found.
    writeFileSync(join(dir, 'unrelated.txt'), Buffer.from('nothing to see'));
    // (c) an empty directory: contributes nothing.
    mkdirSync(join(dir, 'empty'));

    const files = [
      { path: join(dir, 'copied.bin'), hash: members[0].hash },
      { path: join(dir, 'unrelated.txt'), hash: hash(Buffer.from('nothing to see')) },
    ];
    const offenders = extractedMembers(files, members);
    assert.deepEqual(offenders.map(({ path }) => path), [join(dir, 'copied.bin')]);
    assert.equal(offenders[0].matched, 'index.html');
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});
