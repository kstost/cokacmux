import assert from 'node:assert/strict';
import { mkdtemp, mkdir, readFile, readdir, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';

import { publishBuild } from './copy-build.mjs';

async function fixture() {
  const root = await mkdtemp(join(tmpdir(), 'cokacmux-publish-test-'));
  const repoRoot = join(root, 'repo');
  const distRoot = join(root, 'dist');
  await mkdir(join(repoRoot, 'assets'), { recursive: true });
  await mkdir(join(distRoot, 'assets'), { recursive: true });
  await writeFile(join(repoRoot, 'index.html'), '<script src="./assets/old.js"></script>');
  await writeFile(join(repoRoot, 'assets', 'old.js'), 'old');
  return { root, repoRoot, distRoot };
}

test('publishes all new assets before replacing the index and removes stale assets', async (t) => {
  const paths = await fixture();
  t.after(() => rm(paths.root, { recursive: true, force: true }));
  await writeFile(
    join(paths.distRoot, 'index.html'),
    '<script src="./assets/new.js"></script>'
  );
  await writeFile(join(paths.distRoot, 'assets', 'new.js'), 'new');

  await publishBuild(paths);

  assert.equal(
    await readFile(join(paths.repoRoot, 'index.html'), 'utf8'),
    '<script src="./assets/new.js"></script>'
  );
  assert.equal(await readFile(join(paths.repoRoot, 'assets', 'new.js'), 'utf8'), 'new');
  assert.deepEqual(await readdir(join(paths.repoRoot, 'assets')), ['new.js']);
  assert.equal(
    (await readdir(paths.repoRoot)).some((name) => name.startsWith('.website-publish-')),
    false
  );
});

test('missing referenced assets leave the published site untouched', async (t) => {
  const paths = await fixture();
  t.after(() => rm(paths.root, { recursive: true, force: true }));
  await writeFile(
    join(paths.distRoot, 'index.html'),
    '<script src="./assets/missing.js"></script>'
  );
  await writeFile(join(paths.distRoot, 'assets', 'new.js'), 'new');

  await assert.rejects(() => publishBuild(paths), /missing asset/);

  assert.equal(
    await readFile(join(paths.repoRoot, 'index.html'), 'utf8'),
    '<script src="./assets/old.js"></script>'
  );
  assert.deepEqual(await readdir(join(paths.repoRoot, 'assets')), ['old.js']);
});

test('a hash-name collision cannot overwrite an asset used by the old index', async (t) => {
  const paths = await fixture();
  t.after(() => rm(paths.root, { recursive: true, force: true }));
  await writeFile(join(paths.repoRoot, 'index.html'), '<script src="./assets/app.js"></script>');
  await writeFile(join(paths.repoRoot, 'assets', 'app.js'), 'old-content');
  await writeFile(join(paths.distRoot, 'index.html'), '<script src="./assets/app.js"></script>');
  await writeFile(join(paths.distRoot, 'assets', 'app.js'), 'new-content');

  await assert.rejects(() => publishBuild(paths), /name collision/);

  assert.equal(await readFile(join(paths.repoRoot, 'assets', 'app.js'), 'utf8'), 'old-content');
  assert.equal(
    await readFile(join(paths.repoRoot, 'index.html'), 'utf8'),
    '<script src="./assets/app.js"></script>'
  );
});
