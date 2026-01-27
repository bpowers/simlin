'use strict';

const fs = require('fs');
const path = require('path');
const os = require('os');

const {
  checkRequiredFiles,
  measureFileSizesBeforeBuild,
  printFileSizesAfterBuild,
  _formatBytes: formatBytes,
  _canReadAsset: canReadAsset,
  _removeFileNameHash: removeFileNameHash,
  _walkDir: walkDir,
} = require('../config/build-utils');

// Helpers to create and clean up temp directories for filesystem tests.
let tmpDir;
beforeEach(() => {
  tmpDir = fs.mkdtempSync(path.join(os.tmpdir(), 'build-utils-test-'));
});
afterEach(() => {
  fs.rmSync(tmpDir, { recursive: true, force: true });
});

// ---------------------------------------------------------------------------
// formatBytes
// ---------------------------------------------------------------------------

describe('formatBytes', () => {
  test('returns "0 B" for zero', () => {
    expect(formatBytes(0)).toBe('0 B');
  });

  test('formats bytes', () => {
    expect(formatBytes(500)).toBe('500.00 B');
  });

  test('formats kilobytes', () => {
    expect(formatBytes(1024)).toBe('1.00 KB');
    expect(formatBytes(1536)).toBe('1.50 KB');
  });

  test('formats megabytes', () => {
    expect(formatBytes(1048576)).toBe('1.00 MB');
  });

  test('formats negative values with a single minus sign', () => {
    expect(formatBytes(-1024)).toBe('-1.00 KB');
    expect(formatBytes(-512)).toBe('-512.00 B');
    expect(formatBytes(-51200)).toBe('-50.00 KB');
  });
});

// ---------------------------------------------------------------------------
// canReadAsset
// ---------------------------------------------------------------------------

describe('canReadAsset', () => {
  test('accepts .js files', () => {
    expect(canReadAsset('main.abc123.js')).toBe(true);
  });

  test('accepts .css files', () => {
    expect(canReadAsset('styles.abc123.css')).toBe(true);
  });

  test('rejects service-worker.js', () => {
    expect(canReadAsset('service-worker.js')).toBe(false);
  });

  test('rejects non-js/css files', () => {
    expect(canReadAsset('logo.png')).toBe(false);
    expect(canReadAsset('index.html')).toBe(false);
    expect(canReadAsset('data.json')).toBe(false);
  });
});

// ---------------------------------------------------------------------------
// removeFileNameHash
// ---------------------------------------------------------------------------

describe('removeFileNameHash', () => {
  test('strips content hash from JS filenames', () => {
    expect(removeFileNameHash('static/js/main.abc12345.js')).toBe('static/js/main.js');
  });

  test('strips content hash from chunk JS filenames', () => {
    expect(removeFileNameHash('static/js/2.abc12345.chunk.js')).toBe('static/js/2.js');
  });

  test('strips content hash from CSS filenames', () => {
    expect(removeFileNameHash('static/css/main.abc12345.css')).toBe('static/css/main.css');
  });

  test('returns filename unchanged when there is no hash', () => {
    expect(removeFileNameHash('static/js/runtime.js')).toBe('static/js/runtime.js');
  });
});

// ---------------------------------------------------------------------------
// walkDir
// ---------------------------------------------------------------------------

describe('walkDir', () => {
  test('returns empty array for nonexistent directory', () => {
    expect(walkDir('/nonexistent/path')).toEqual([]);
  });

  test('returns files in a flat directory', () => {
    fs.writeFileSync(path.join(tmpDir, 'a.js'), 'a');
    fs.writeFileSync(path.join(tmpDir, 'b.css'), 'b');

    const result = walkDir(tmpDir);
    expect(result.sort()).toEqual([
      path.join(tmpDir, 'a.js'),
      path.join(tmpDir, 'b.css'),
    ].sort());
  });

  test('returns files recursively', () => {
    const sub = path.join(tmpDir, 'sub');
    fs.mkdirSync(sub);
    fs.writeFileSync(path.join(tmpDir, 'root.js'), 'r');
    fs.writeFileSync(path.join(sub, 'nested.js'), 'n');

    const result = walkDir(tmpDir);
    expect(result.sort()).toEqual([
      path.join(tmpDir, 'root.js'),
      path.join(sub, 'nested.js'),
    ].sort());
  });

  test('returns empty array for empty directory', () => {
    expect(walkDir(tmpDir)).toEqual([]);
  });

  test('propagates non-ENOENT errors', () => {
    // Verify that errors other than ENOENT bubble up. We pass a file path
    // (not a directory) to readdirSync, which throws ENOTDIR.
    const filePath = path.join(tmpDir, 'file.txt');
    fs.writeFileSync(filePath, 'x');

    expect(() => walkDir(filePath)).toThrow();
  });
});

// ---------------------------------------------------------------------------
// checkRequiredFiles
// ---------------------------------------------------------------------------

describe('checkRequiredFiles', () => {
  test('returns true when all files exist', () => {
    const f1 = path.join(tmpDir, 'a.txt');
    const f2 = path.join(tmpDir, 'b.txt');
    fs.writeFileSync(f1, 'a');
    fs.writeFileSync(f2, 'b');

    expect(checkRequiredFiles([f1, f2])).toBe(true);
  });

  test('returns false when a file is missing', () => {
    const existing = path.join(tmpDir, 'a.txt');
    fs.writeFileSync(existing, 'a');
    const missing = path.join(tmpDir, 'missing.txt');

    expect(checkRequiredFiles([existing, missing])).toBe(false);
  });

  test('returns true for empty array', () => {
    expect(checkRequiredFiles([])).toBe(true);
  });
});

// ---------------------------------------------------------------------------
// measureFileSizesBeforeBuild
// ---------------------------------------------------------------------------

describe('measureFileSizesBeforeBuild', () => {
  test('returns empty sizes for nonexistent build folder', () => {
    const result = measureFileSizesBeforeBuild('/nonexistent/build');
    expect(result).toEqual({ root: '/nonexistent/build', sizes: {} });
  });

  test('measures gzipped sizes of .js and .css files', () => {
    const jsFile = path.join(tmpDir, 'main.abc12345.js');
    const cssFile = path.join(tmpDir, 'main.abc12345.css');
    const pngFile = path.join(tmpDir, 'logo.png');

    fs.writeFileSync(jsFile, 'console.log("hello world");');
    fs.writeFileSync(cssFile, 'body { margin: 0; }');
    fs.writeFileSync(pngFile, 'fake png data');

    const result = measureFileSizesBeforeBuild(tmpDir);
    expect(result.root).toBe(tmpDir);
    // JS and CSS should be measured (hash stripped from key).
    // Use array syntax because jest interprets dots as nested paths.
    expect(result.sizes).toHaveProperty(['main.js']);
    expect(result.sizes).toHaveProperty(['main.css']);
    // PNG should not appear
    expect(result.sizes).not.toHaveProperty(['logo.png']);
    // Sizes should be positive integers
    expect(result.sizes['main.js']).toBeGreaterThan(0);
    expect(result.sizes['main.css']).toBeGreaterThan(0);
  });

  test('handles nested static directories', () => {
    const jsDir = path.join(tmpDir, 'static', 'js');
    fs.mkdirSync(jsDir, { recursive: true });
    fs.writeFileSync(path.join(jsDir, 'main.abc12345.js'), 'var x = 1;');

    const result = measureFileSizesBeforeBuild(tmpDir);
    expect(result.sizes).toHaveProperty(['static/js/main.js']);
  });
});

// ---------------------------------------------------------------------------
// printFileSizesAfterBuild
// ---------------------------------------------------------------------------

describe('printFileSizesAfterBuild', () => {
  // Capture console.log output for assertions.
  let logOutput;
  const origLog = console.log;

  beforeEach(() => {
    logOutput = [];
    console.log = (...args) => logOutput.push(args.join(' '));
  });
  afterEach(() => {
    console.log = origLog;
  });

  function makeStats(assetNames) {
    return {
      assets: assetNames.map(name => ({ name })),
    };
  }

  test('prints sizes for build assets', () => {
    const jsDir = path.join(tmpDir, 'static', 'js');
    fs.mkdirSync(jsDir, { recursive: true });
    fs.writeFileSync(path.join(jsDir, 'main.abc12345.js'), 'console.log("hi");');

    const stats = makeStats(['static/js/main.abc12345.js']);
    const previousSizes = { root: tmpDir, sizes: {} };

    printFileSizesAfterBuild(stats, previousSizes, tmpDir, 512 * 1024, 1024 * 1024);

    // Should have printed at least one line with the asset name
    const output = logOutput.join('\n');
    expect(output).toContain('main.abc12345.js');
  });

  test('skips missing asset files without crashing', () => {
    const stats = makeStats(['static/js/gone.abc12345.js']);
    const previousSizes = { root: tmpDir, sizes: {} };

    // Should not throw even though the file doesn't exist
    expect(() => {
      printFileSizesAfterBuild(stats, previousSizes, tmpDir, 512 * 1024, 1024 * 1024);
    }).not.toThrow();
  });

  test('shows size difference when previous sizes exist', () => {
    const jsDir = path.join(tmpDir, 'static', 'js');
    fs.mkdirSync(jsDir, { recursive: true });
    const content = 'x'.repeat(1000);
    fs.writeFileSync(path.join(jsDir, 'main.abc12345.js'), content);

    const stats = makeStats(['static/js/main.abc12345.js']);
    // Pretend the previous size was 1 byte (virtually everything will be "larger")
    const previousSizes = { root: tmpDir, sizes: { 'static/js/main.js': 1 } };

    printFileSizesAfterBuild(stats, previousSizes, tmpDir, 512 * 1024, 1024 * 1024);

    const output = logOutput.join('\n');
    // Should contain a "+" diff indicator
    expect(output).toContain('+');
  });

  test('warns only when asset exceeds its type-appropriate limit', () => {
    const jsDir = path.join(tmpDir, 'static', 'js');
    fs.mkdirSync(jsDir, { recursive: true });
    // Create a file whose gzipped size will be small (well under any limit)
    fs.writeFileSync(path.join(jsDir, 'chunk.abc12345.js'), 'var x = 1;');

    const stats = makeStats(['static/js/chunk.abc12345.js']);
    const previousSizes = { root: tmpDir, sizes: {} };

    // Set both limits very high so nothing exceeds them
    printFileSizesAfterBuild(stats, previousSizes, tmpDir, 10 * 1024 * 1024, 10 * 1024 * 1024);

    const output = logOutput.join('\n');
    expect(output).not.toContain('significantly larger');
  });

  test('warns when a main bundle exceeds the bundle limit', () => {
    const jsDir = path.join(tmpDir, 'static', 'js');
    fs.mkdirSync(jsDir, { recursive: true });
    // Write enough content so gzipped size exceeds our tiny limit
    const bigContent = 'console.log(' + JSON.stringify('x'.repeat(1000)) + ');\n';
    fs.writeFileSync(path.join(jsDir, 'main.abc12345.js'), bigContent);

    const stats = makeStats(['static/js/main.abc12345.js']);
    const previousSizes = { root: tmpDir, sizes: {} };

    // Set bundle limit to 1 byte so main.* definitely exceeds it
    printFileSizesAfterBuild(stats, previousSizes, tmpDir, 1, 10 * 1024 * 1024);

    const output = logOutput.join('\n');
    expect(output).toContain('significantly larger');
  });

  test('warns when a chunk exceeds the chunk limit but not the bundle limit', () => {
    const jsDir = path.join(tmpDir, 'static', 'js');
    fs.mkdirSync(jsDir, { recursive: true });
    const bigContent = 'console.log(' + JSON.stringify('x'.repeat(1000)) + ');\n';
    fs.writeFileSync(path.join(jsDir, 'vendor.abc12345.js'), bigContent);

    const stats = makeStats(['static/js/vendor.abc12345.js']);
    const previousSizes = { root: tmpDir, sizes: {} };

    // Bundle limit is huge (main.* would pass), but chunk limit is 1 byte
    printFileSizesAfterBuild(stats, previousSizes, tmpDir, 10 * 1024 * 1024, 1);

    const output = logOutput.join('\n');
    expect(output).toContain('significantly larger');
  });
});
