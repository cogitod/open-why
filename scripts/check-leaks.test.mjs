import assert from 'node:assert/strict';
import { execFileSync, spawnSync } from 'node:child_process';
import { mkdtempSync, mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import test from 'node:test';

const scanner = readFileSync(new URL('../hooks/check-leaks.sh', import.meta.url), 'utf8');
const brand = 'Cogi' + 'to';
const attribution = `An open-source project by [${brand}](https://${brand.toLowerCase()}.cv). Apache-2.0.`;

function scan(path, content, mode) {
  const directory = mkdtempSync(join(tmpdir(), 'open-why-leak-test-'));
  try {
    execFileSync('git', ['init', '--quiet', directory]);
    mkdirSync(join(directory, dirname(path)), { recursive: true });
    writeFileSync(join(directory, path), content);
    execFileSync('git', ['add', '--', path], { cwd: directory });
    return spawnSync('bash', ['-c', scanner, 'check-leaks', mode], { cwd: directory, encoding: 'utf8' });
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
}

for (const mode of ['staged', 'tracked']) {
  test(`${mode}: only the exact README attribution is public`, () => {
    assert.equal(scan('README.md', attribution, mode).status, 0);
    assert.equal(scan('src/example.rs', attribution, mode).status, 1);
    assert.equal(scan('README.md', `${attribution} private runtime`, mode).status, 1);
    assert.equal(scan('README.md', `${attribution}\n${brand} private runtime`, mode).status, 1);
  });

  test(`${mode}: attribution cannot hide secrets or private paths`, () => {
    const privatePath = '/' + 'Users/' + 'fixture/project';
    const token = 'gh' + 'p_' + 'a'.repeat(36);
    const runtime = 'Breathe' + 'MCP';
    for (const forbidden of [privatePath, token, runtime]) {
      assert.equal(scan('README.md', `${attribution}\n${forbidden}`, mode).status, 1);
    }
  });
}
