import { cp, mkdir, rm } from 'node:fs/promises';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const __dirname = dirname(fileURLToPath(import.meta.url));
const websiteRoot = resolve(__dirname, '..');
const repoRoot = resolve(websiteRoot, '..');
const distRoot = resolve(websiteRoot, 'dist');

await rm(resolve(repoRoot, 'index.html'), { force: true });
await rm(resolve(repoRoot, 'assets'), { recursive: true, force: true });
await mkdir(repoRoot, { recursive: true });
await cp(resolve(distRoot, 'index.html'), resolve(repoRoot, 'index.html'));
await cp(resolve(distRoot, 'assets'), resolve(repoRoot, 'assets'), {
  recursive: true
});
