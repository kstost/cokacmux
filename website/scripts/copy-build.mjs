import {
  copyFile,
  cp,
  lstat,
  mkdir,
  mkdtemp,
  readdir,
  readFile,
  rename,
  rm
} from 'node:fs/promises';
import { randomUUID } from 'node:crypto';
import { basename, dirname, join, posix, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const scriptPath = fileURLToPath(import.meta.url);
const __dirname = dirname(scriptPath);
const websiteRoot = resolve(__dirname, '..');
const defaultRepoRoot = resolve(websiteRoot, '..');
const defaultDistRoot = resolve(websiteRoot, 'dist');

async function optionalLstat(path) {
  try {
    return await lstat(path);
  } catch (error) {
    if (error?.code === 'ENOENT') return null;
    throw error;
  }
}

async function collectAssetFiles(root, current = root, prefix = '') {
  const files = new Map();
  for (const entry of await readdir(current, { withFileTypes: true })) {
    const relativePath = prefix ? posix.join(prefix, entry.name) : entry.name;
    const sourcePath = join(current, entry.name);
    if (entry.isSymbolicLink()) {
      throw new Error(`Refusing to publish symlinked asset: ${relativePath}`);
    }
    if (entry.isDirectory()) {
      const nested = await collectAssetFiles(root, sourcePath, relativePath);
      for (const [path, source] of nested) files.set(path, source);
    } else if (entry.isFile()) {
      files.set(relativePath, sourcePath);
    } else {
      throw new Error(`Refusing to publish special asset: ${relativePath}`);
    }
  }
  return files;
}

async function assertIndexAssets(indexPath, assetFiles) {
  const html = await readFile(indexPath, 'utf8');
  const references = html.matchAll(/(?:src|href)=["']\.\/assets\/([^"'#?]+)(?:[?#][^"']*)?["']/g);
  for (const match of references) {
    const relativePath = decodeURIComponent(match[1]);
    if (relativePath.startsWith('../') || !assetFiles.has(relativePath)) {
      throw new Error(`Built index references a missing asset: ${relativePath}`);
    }
  }
}

async function filesEqual(first, second) {
  const [left, right] = await Promise.all([readFile(first), readFile(second)]);
  return left.equals(right);
}

async function preflightDestination(assetFiles, assetsRoot) {
  const assetsStat = await optionalLstat(assetsRoot);
  if (assetsStat && !assetsStat.isDirectory()) {
    throw new Error(`Refusing to replace non-directory assets path: ${assetsRoot}`);
  }

  for (const [relativePath, source] of assetFiles) {
    const destination = join(assetsRoot, ...relativePath.split('/'));
    const destinationStat = await optionalLstat(destination);
    if (!destinationStat) continue;
    if (!destinationStat.isFile() || !(await filesEqual(source, destination))) {
      throw new Error(
        `Asset name collision with different content: ${relativePath}. ` +
          'Generated assets must use content-hashed names.'
      );
    }
  }
}

async function copyAssets(assetFiles, assetsRoot) {
  await mkdir(assetsRoot, { recursive: true });
  for (const [relativePath, source] of assetFiles) {
    const destination = join(assetsRoot, ...relativePath.split('/'));
    if (await optionalLstat(destination)) continue;
    await mkdir(dirname(destination), { recursive: true });
    const temporary = join(
      dirname(destination),
      `.${posix.basename(relativePath)}.${randomUUID()}.tmp`
    );
    try {
      await copyFile(source, temporary);
      await rename(temporary, destination);
    } finally {
      await rm(temporary, { force: true });
    }
  }
}

async function replaceFileAtomically(source, destination) {
  const temporary = join(dirname(destination), `.${basename(destination)}.${randomUUID()}.tmp`);
  try {
    await copyFile(source, temporary);
    await rename(temporary, destination);
  } finally {
    await rm(temporary, { force: true });
  }
}

async function pruneAssets(root, expected, current = root, prefix = '') {
  for (const entry of await readdir(current, { withFileTypes: true })) {
    const relativePath = prefix ? posix.join(prefix, entry.name) : entry.name;
    const path = join(current, entry.name);
    if (entry.isDirectory()) {
      await pruneAssets(root, expected, path, relativePath);
      if ((await readdir(path)).length === 0) await rm(path, { recursive: true });
    } else if (!expected.has(relativePath)) {
      await rm(path, { force: true });
    }
  }
}

export async function publishBuild({
  distRoot = defaultDistRoot,
  repoRoot = defaultRepoRoot
} = {}) {
  const sourceIndex = resolve(distRoot, 'index.html');
  const sourceAssets = resolve(distRoot, 'assets');
  const indexStat = await optionalLstat(sourceIndex);
  const assetsStat = await optionalLstat(sourceAssets);
  if (!indexStat?.isFile() || !assetsStat?.isDirectory()) {
    throw new Error(`Incomplete website build output: ${distRoot}`);
  }

  await mkdir(repoRoot, { recursive: true });
  const destinationIndex = resolve(repoRoot, 'index.html');
  const destinationIndexStat = await optionalLstat(destinationIndex);
  if (destinationIndexStat && !destinationIndexStat.isFile()) {
    throw new Error(`Refusing to replace non-file index path: ${destinationIndex}`);
  }
  const stagingRoot = await mkdtemp(join(repoRoot, '.website-publish-'));
  try {
    const stagedIndex = join(stagingRoot, 'index.html');
    const stagedAssets = join(stagingRoot, 'assets');
    await copyFile(sourceIndex, stagedIndex);
    await cp(sourceAssets, stagedAssets, { recursive: true });

    const assetFiles = await collectAssetFiles(stagedAssets);
    if (assetFiles.size === 0) throw new Error('Website build contains no assets');
    await assertIndexAssets(stagedIndex, assetFiles);

    const destinationAssets = resolve(repoRoot, 'assets');
    await preflightDestination(assetFiles, destinationAssets);
    await copyAssets(assetFiles, destinationAssets);

    // Keep the old index valid until every new hashed asset is in place.
    await replaceFileAtomically(stagedIndex, destinationIndex);
    await pruneAssets(destinationAssets, new Set(assetFiles.keys()));
  } finally {
    await rm(stagingRoot, { recursive: true, force: true });
  }
}

if (process.argv[1] && resolve(process.argv[1]) === resolve(scriptPath)) {
  await publishBuild();
}
