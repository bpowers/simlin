import assert from 'node:assert/strict';
import { describe, it } from 'node:test';

import {
  hostFromUrl,
  addAuthorizedDomain,
  removeAuthorizedDomain,
  firstNonEmptyLine,
  extractUrl,
  parseArgs,
  versionTrafficShare,
} from '../deploy-canary.mjs';

describe('hostFromUrl', () => {
  it('strips scheme and trailing slash', () => {
    assert.equal(hostFromUrl('https://foo.appspot.com/'), 'foo.appspot.com');
  });

  it('accepts http as well as https', () => {
    assert.equal(hostFromUrl('http://foo.appspot.com'), 'foo.appspot.com');
  });

  it('strips a path and query', () => {
    assert.equal(
      hostFromUrl('https://20260627t123456-dot-myproj.uc.r.appspot.com/some/path?q=1'),
      '20260627t123456-dot-myproj.uc.r.appspot.com',
    );
  });

  it('drops the port (authorizedDomains entries are bare hostnames)', () => {
    assert.equal(hostFromUrl('http://localhost:5000/'), 'localhost');
  });

  it('accepts a bare host with no scheme', () => {
    assert.equal(hostFromUrl('foo-dot-bar.appspot.com'), 'foo-dot-bar.appspot.com');
  });

  it('trims surrounding whitespace', () => {
    assert.equal(hostFromUrl('  https://foo.appspot.com/  '), 'foo.appspot.com');
  });

  it('throws on empty / non-string input', () => {
    assert.throws(() => hostFromUrl(''));
    assert.throws(() => hostFromUrl(undefined));
  });
});

describe('addAuthorizedDomain', () => {
  it('appends a new host', () => {
    assert.deepEqual(addAuthorizedDomain(['localhost', 'app.simlin.com'], 'v1-dot-p.appspot.com'), [
      'localhost',
      'app.simlin.com',
      'v1-dot-p.appspot.com',
    ]);
  });

  it('is a no-op when the host is already present (idempotent)', () => {
    const domains = ['localhost', 'app.simlin.com'];
    assert.deepEqual(addAuthorizedDomain(domains, 'app.simlin.com'), ['localhost', 'app.simlin.com']);
  });

  it('dedups pre-existing duplicates while preserving order', () => {
    assert.deepEqual(addAuthorizedDomain(['a', 'a', 'b'], 'c'), ['a', 'b', 'c']);
  });

  it('does not mutate the input list', () => {
    const domains = ['localhost'];
    addAuthorizedDomain(domains, 'new.example.com');
    assert.deepEqual(domains, ['localhost']);
  });

  it('tolerates an undefined/empty current list', () => {
    assert.deepEqual(addAuthorizedDomain(undefined, 'x.appspot.com'), ['x.appspot.com']);
    assert.deepEqual(addAuthorizedDomain([], 'x.appspot.com'), ['x.appspot.com']);
  });
});

describe('removeAuthorizedDomain', () => {
  it('removes the host', () => {
    assert.deepEqual(removeAuthorizedDomain(['localhost', 'v1-dot-p.appspot.com'], 'v1-dot-p.appspot.com'), [
      'localhost',
    ]);
  });

  it('is a no-op when the host is absent (idempotent)', () => {
    assert.deepEqual(removeAuthorizedDomain(['localhost'], 'nope.appspot.com'), ['localhost']);
  });

  it('removes every occurrence if duplicated', () => {
    assert.deepEqual(removeAuthorizedDomain(['a', 'b', 'a'], 'a'), ['b']);
  });

  it('does not mutate the input list', () => {
    const domains = ['localhost', 'x.appspot.com'];
    removeAuthorizedDomain(domains, 'x.appspot.com');
    assert.deepEqual(domains, ['localhost', 'x.appspot.com']);
  });

  it('tolerates an undefined/empty current list', () => {
    assert.deepEqual(removeAuthorizedDomain(undefined, 'x'), []);
    assert.deepEqual(removeAuthorizedDomain([], 'x'), []);
  });
});

describe('firstNonEmptyLine', () => {
  it('returns the first non-blank trimmed line', () => {
    assert.equal(firstNonEmptyLine('\n  \n  20260627t1\n more\n'), '20260627t1');
  });

  it('trims a single value', () => {
    assert.equal(firstNonEmptyLine('  myproject  \n'), 'myproject');
  });

  it('returns empty string for blank / non-string input', () => {
    assert.equal(firstNonEmptyLine('   \n  '), '');
    assert.equal(firstNonEmptyLine(undefined), '');
  });
});

describe('extractUrl', () => {
  it('pulls the first http(s) URL out of noisy output', () => {
    const out =
      'Did not detect your browser. Go to this link to view your app:\n' +
      'https://20260627t1-dot-myproj.uc.r.appspot.com\n';
    assert.equal(extractUrl(out), 'https://20260627t1-dot-myproj.uc.r.appspot.com');
  });

  it('handles a plain URL', () => {
    assert.equal(extractUrl('https://foo.appspot.com/'), 'https://foo.appspot.com/');
  });

  it('returns undefined when there is no URL', () => {
    assert.equal(extractUrl('no url here'), undefined);
    assert.equal(extractUrl(undefined), undefined);
  });
});

describe('parseArgs', () => {
  it('defaults to deploy mode', () => {
    assert.deepEqual(parseArgs([]), { mode: 'deploy', project: undefined, version: undefined });
  });

  it('parses --project with a separate value', () => {
    assert.equal(parseArgs(['--project', 'myproj']).project, 'myproj');
  });

  it('parses --project=value inline', () => {
    assert.equal(parseArgs(['--project=myproj']).project, 'myproj');
  });

  it('parses --cleanup <version> into cleanup mode', () => {
    const args = parseArgs(['--cleanup', '20260627t1']);
    assert.equal(args.mode, 'cleanup');
    assert.equal(args.version, '20260627t1');
  });

  it('parses --cleanup=<version> inline', () => {
    assert.equal(parseArgs(['--cleanup=20260627t1']).version, '20260627t1');
  });

  it('combines --cleanup and --project', () => {
    const args = parseArgs(['--cleanup', '20260627t1', '--project', 'p']);
    assert.equal(args.mode, 'cleanup');
    assert.equal(args.version, '20260627t1');
    assert.equal(args.project, 'p');
  });

  it('maps --help/-h to help mode', () => {
    assert.equal(parseArgs(['--help']).mode, 'help');
    assert.equal(parseArgs(['-h']).mode, 'help');
  });

  it('throws when --cleanup has no version', () => {
    assert.throws(() => parseArgs(['--cleanup']), /requires a version/);
    assert.throws(() => parseArgs(['--cleanup', '--project', 'p']), /requires a version/);
  });

  it('throws on an unknown argument', () => {
    assert.throws(() => parseArgs(['--bogus']), /unknown argument/);
  });

  it('throws when --project has no value', () => {
    // A prod-mutating tool must not silently fall back to the default project
    // when an override was clearly intended (e.g. `--project $EMPTY`).
    assert.throws(() => parseArgs(['--project']), /--project requires a value/);
    assert.throws(() => parseArgs(['--project', '--cleanup', 'v']), /--project requires a value/);
    assert.throws(() => parseArgs(['--project', '']), /--project requires a value/);
    assert.throws(() => parseArgs(['--project=']), /--project requires a value/);
    assert.throws(() => parseArgs(['--project=   ']), /--project requires a value/);
  });
});

describe('versionTrafficShare', () => {
  it('returns the fraction allocated to the version', () => {
    assert.equal(versionTrafficShare({ '20260627t1': 1 }, '20260627t1'), 1);
    assert.equal(versionTrafficShare({ a: 0.3, b: 0.7 }, 'b'), 0.7);
  });

  it('returns 0 when the version is absent (safe to stop)', () => {
    assert.equal(versionTrafficShare({ other: 1 }, '20260627t1'), 0);
  });

  it('returns 0 for a missing/empty allocations map', () => {
    assert.equal(versionTrafficShare(undefined, 'v'), 0);
    assert.equal(versionTrafficShare(null, 'v'), 0);
    assert.equal(versionTrafficShare({}, 'v'), 0);
  });

  it('treats a non-numeric allocation as 0 rather than truthy', () => {
    assert.equal(versionTrafficShare({ v: '1' }, 'v'), 0);
    assert.equal(versionTrafficShare({ v: NaN }, 'v'), 0);
  });
});
