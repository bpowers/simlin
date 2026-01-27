'use strict';

const fs = require('fs');
const path = require('path');
const zlib = require('zlib');
const pc = require('picocolors');

/**
 * Check that all required files exist. Log an error and return false if any
 * are missing.
 */
function checkRequiredFiles(files) {
  try {
    files.forEach(filePath => {
      fs.accessSync(filePath, fs.constants.F_OK);
    });
    return true;
  } catch (err) {
    const filePath = err.path || '';
    console.log(pc.red('Could not find a required file.'));
    console.log(pc.red('  Name: ') + pc.cyan(path.basename(filePath)));
    console.log(pc.red('  Searched in: ') + pc.cyan(path.dirname(filePath)));
    return false;
  }
}

function canReadAsset(asset) {
  return /\.(js|css)$/.test(asset) && !/service-worker\.js/.test(asset);
}

function formatBytes(bytes) {
  if (bytes === 0) return '0 B';
  const sign = bytes < 0 ? '-' : '';
  const abs = Math.abs(bytes);
  const units = ['B', 'KB', 'MB'];
  const i = Math.floor(Math.log(abs) / Math.log(1024));
  const value = abs / Math.pow(1024, i);
  return sign + value.toFixed(2) + ' ' + units[Math.min(i, units.length - 1)];
}

function gzipSize(buf) {
  return zlib.gzipSync(buf).length;
}

function removeFileNameHash(fileName) {
  return fileName
    .replace(/\\/g, '/')
    .replace(
      /\/?(.*)(\.[0-9a-f]+)(\.chunk)?(\.js|\.css)/,
      (match, p1, p2, p3, p4) => p1 + p4
    );
}

/**
 * Walk a directory recursively and return all file paths.
 */
function walkDir(dir) {
  let results = [];
  try {
    const entries = fs.readdirSync(dir, { withFileTypes: true });
    for (const entry of entries) {
      const fullPath = path.join(dir, entry.name);
      if (entry.isDirectory()) {
        results = results.concat(walkDir(fullPath));
      } else {
        results.push(fullPath);
      }
    }
  } catch (err) {
    if (err.code !== 'ENOENT') {
      throw err;
    }
  }
  return results;
}

/**
 * Measure file sizes in the build folder before building, so we can show a
 * comparison afterward.
 */
function measureFileSizesBeforeBuild(buildFolder) {
  const fileNames = walkDir(buildFolder);
  const sizes = {};
  for (const fileName of fileNames) {
    const relativeName = path.relative(buildFolder, fileName);
    if (canReadAsset(relativeName)) {
      const contents = fs.readFileSync(fileName);
      const key = removeFileNameHash(relativeName);
      sizes[key] = gzipSize(contents);
    }
  }
  return { root: buildFolder, sizes };
}

/**
 * Print a table of build file sizes (gzipped), with optional comparison to
 * previous sizes.
 */
function printFileSizesAfterBuild(
  stats,
  previousSizeMap,
  buildFolder,
  maxBundleGzipSize,
  maxChunkGzipSize
) {
  const sizes = previousSizeMap.sizes;
  const statsData = (stats.stats || [stats]);
  const assets = [];

  for (const s of statsData) {
    let assetList;
    if (typeof s.toJson === 'function') {
      assetList = s.toJson({ all: false, assets: true }).assets;
    } else if (s.assets) {
      assetList = s.assets;
    } else {
      continue;
    }

    for (const asset of assetList) {
      if (!canReadAsset(asset.name)) continue;
      const filePath = path.join(buildFolder, asset.name);
      if (!fs.existsSync(filePath)) continue;

      const fileContents = fs.readFileSync(filePath);
      const size = gzipSize(fileContents);
      const previousSize = sizes[removeFileNameHash(asset.name)];
      let difference = '';
      if (previousSize != null) {
        const diff = size - previousSize;
        if (diff > 50 * 1024) {
          difference = pc.red('+' + formatBytes(diff));
        } else if (diff > 0) {
          difference = pc.yellow('+' + formatBytes(diff));
        } else if (diff < 0) {
          difference = pc.green(formatBytes(diff));
        }
      }
      const sizeLabel = formatBytes(size) + (difference ? ' (' + difference + ')' : '');

      assets.push({
        folder: path.join(path.basename(buildFolder), path.dirname(asset.name)),
        name: path.basename(asset.name),
        size,
        sizeLabel,
      });
    }
  }

  assets.sort((a, b) => b.size - a.size);

  for (const asset of assets) {
    const isMainBundle = asset.name.indexOf('main.') === 0;
    const maxRecommendedSize = isMainBundle ? maxBundleGzipSize : maxChunkGzipSize;
    const isLarge = maxRecommendedSize && asset.size > maxRecommendedSize;

    console.log(
      '  ' +
        (isLarge ? pc.yellow(asset.sizeLabel) : asset.sizeLabel) +
        '  ' +
        pc.dim(asset.folder + path.sep) +
        pc.cyan(asset.name)
    );
  }

  if (assets.some(a => a.size > maxBundleGzipSize)) {
    console.log();
    console.log(pc.yellow('The bundle size is significantly larger than recommended.'));
  }
}

module.exports = {
  checkRequiredFiles,
  measureFileSizesBeforeBuild,
  printFileSizesAfterBuild,
};
