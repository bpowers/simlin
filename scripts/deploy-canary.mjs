#!/usr/bin/env node
//
// Deploy HEAD to Google App Engine as a NON-TRAFFIC canary, then authorize the
// canary's versioned host in Firebase so the operator can exercise the REAL
// product end-to-end -- including Google sign-in -- before switching 100% of
// production traffic.
//
// Why this script exists:
// - `gcloud app deploy --no-promote` creates a version reachable only at a
//   versioned `*.appspot.com` URL (e.g. https://<version>-dot-<project>...).
//   That host is NOT in Firebase's Authorized domains list, so Firebase OAuth
//   (signInWithRedirect via auth.simlin.com) rejects the calling origin and the
//   operator cannot actually log in to test the canary.
// - The fix is to add the canary's host to the Identity Toolkit
//   `authorizedDomains` list for the duration of the test, then remove it.
//
// This deliberately uses the OPERATOR's own credentials (`gcloud auth
// print-access-token`), NOT the CI deploy service account: mutating Firebase
// auth config requires roles/firebaseauth.admin, which is an operator-level
// grant we intentionally keep off the CI deploy SA.
//
// It does NOT promote traffic. Promoting is a separate, explicit command the
// script prints for you to run after the smoke test passes.
//
// Usage:
//   node scripts/deploy-canary.mjs [--project <id>]   # deploy canary + authorize
//   node scripts/deploy-canary.mjs --cleanup <version> [--project <id>]
//
// The build/deploy itself is delegated to scripts/deploy-web-staged.sh (the
// self-contained staged deploy); this script adds only the --no-promote canary
// orchestration + Firebase authorized-domain management around it.
//
// ---------------------------------------------------------------------------
// The file is split into a PURE CORE (exported, unit-tested in
// scripts/tests/deploy-canary.test.mjs) and an IMPERATIVE SHELL (all the
// gcloud/fetch side effects). main() runs only when the file is executed
// directly, so importing it for tests has no side effects.
// ---------------------------------------------------------------------------

import { spawnSync } from 'node:child_process';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const SCRIPT_DIR = path.dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = path.resolve(SCRIPT_DIR, '..');

const IDENTITY_TOOLKIT_BASE = 'https://identitytoolkit.googleapis.com/admin/v2';

// ===========================================================================
// PURE CORE -- no I/O, deterministic, unit-tested.
// ===========================================================================

/**
 * Reduce a URL (or already-bare host) to its bare hostname: scheme, path,
 * query, and any port are stripped. Firebase `authorizedDomains` entries are
 * bare hostnames (the OAuth check compares window.location.hostname), so the
 * port is intentionally dropped. Accepts input without a scheme so a host can
 * be passed straight through. Throws on empty/non-string input.
 */
export function hostFromUrl(input) {
  if (typeof input !== 'string' || input.trim() === '') {
    throw new Error('hostFromUrl: expected a non-empty URL or host string');
  }
  const trimmed = input.trim();
  // Give the URL parser a scheme to chew on when one is absent, so a bare host
  // like "foo.appspot.com" parses instead of throwing.
  const hasScheme = /^[a-zA-Z][a-zA-Z0-9+.-]*:\/\//.test(trimmed);
  const url = new URL(hasScheme ? trimmed : `https://${trimmed}`);
  return url.hostname;
}

/**
 * Return a new authorized-domains list that includes `host`, deduped and with
 * existing order preserved. A no-op (returns an equivalent deduped copy) when
 * `host` is already present. Never mutates the input.
 *
 * This is the read side of the read-modify-write that guarantees the PATCH
 * never wipes the existing list: the caller passes the full current list (from
 * a successful GET); this appends to it rather than replacing it.
 */
export function addAuthorizedDomain(domains, host) {
  const base = Array.isArray(domains) ? domains : [];
  const seen = new Set();
  const out = [];
  for (const d of [...base, host]) {
    if (!seen.has(d)) {
      seen.add(d);
      out.push(d);
    }
  }
  return out;
}

/**
 * Return a new authorized-domains list with every occurrence of `host`
 * removed. A no-op (returns a copy) when `host` is absent. Never mutates the
 * input. Like addAuthorizedDomain, this operates on the FULL current list so
 * the resulting PATCH replaces the field without losing other entries.
 */
export function removeAuthorizedDomain(domains, host) {
  const base = Array.isArray(domains) ? domains : [];
  return base.filter((d) => d !== host);
}

/** First non-blank, trimmed line of text (or '' for blank/non-string input). */
export function firstNonEmptyLine(text) {
  if (typeof text !== 'string') return '';
  for (const line of text.split('\n')) {
    const t = line.trim();
    if (t !== '') return t;
  }
  return '';
}

/**
 * Extract the first http(s) URL from arbitrary command output. `gcloud app
 * browse --no-launch-browser` prints a human sentence followed by the URL, so
 * we cannot just trim the whole output. Returns undefined when none is found.
 */
export function extractUrl(text) {
  if (typeof text !== 'string') return undefined;
  const m = text.match(/https?:\/\/\S+/);
  return m ? m[0] : undefined;
}

/**
 * Fraction of `default`-service traffic currently allocated to `versionId`,
 * read from the `split.allocations` map (versionId -> fraction) returned by
 * `gcloud app services describe`. Returns 0 when the version is absent or the
 * map is missing/malformed, and treats a non-finite-number allocation as 0
 * (never truthy) so the caller's serving check can't be fooled by a string.
 *
 * Cleanup uses this to REFUSE to stop a version that is serving traffic: once
 * the operator promotes the canary, it IS production, and stopping it is a full
 * outage. De-authorizing the host is always safe; only the stop is dangerous.
 */
export function versionTrafficShare(allocations, versionId) {
  if (!allocations || typeof allocations !== 'object') return 0;
  const share = allocations[versionId];
  return typeof share === 'number' && Number.isFinite(share) ? share : 0;
}

/**
 * Parse argv (without node/script) into { mode, project, version }.
 *   mode: 'deploy' (default) | 'cleanup' | 'help'
 * Supports `--flag value` and `--flag=value`. Throws on unknown flags and on
 * --cleanup without a version id (failing loud beats a confusing later error).
 */
export function parseArgs(argv) {
  const args = { mode: 'deploy', project: undefined, version: undefined };
  const list = [...argv];
  while (list.length > 0) {
    const token = list.shift();
    const eq = token.startsWith('--') ? token.indexOf('=') : -1;
    const flag = eq >= 0 ? token.slice(0, eq) : token;
    const inlineVal = eq >= 0 ? token.slice(eq + 1) : undefined;
    // Do not consume a following flag as this flag's value: `--cleanup
    // --project p` must leave version unset (and report the missing version),
    // not swallow `--project`.
    const takeVal = () => {
      if (inlineVal !== undefined) return inlineVal;
      return list.length > 0 && !list[0].startsWith('--') ? list.shift() : undefined;
    };
    switch (flag) {
      case '--cleanup':
        args.mode = 'cleanup';
        args.version = takeVal();
        break;
      case '--project':
        args.project = takeVal();
        break;
      case '--help':
      case '-h':
        args.mode = 'help';
        break;
      default:
        throw new Error(`unknown argument: ${token}`);
    }
  }
  if (args.mode === 'cleanup' && (!args.version || args.version.startsWith('--'))) {
    throw new Error('--cleanup requires a version id, e.g. --cleanup 20260627t123456');
  }
  return args;
}

// ===========================================================================
// IMPERATIVE SHELL -- gcloud + fetch side effects. Kept deliberately small and
// obvious; the testable logic lives in the pure core above.
// ===========================================================================

function die(msg) {
  console.error(`\nERROR: ${msg}`);
  process.exit(1);
}

/**
 * Run a command, inheriting stdio by default so the operator watches progress.
 * With { capture: true } stdout is returned as a string (stderr still streams).
 * Fails fast with a clear message on a missing binary or non-zero exit.
 */
function run(cmd, cmdArgs, { capture = false } = {}) {
  const res = spawnSync(cmd, cmdArgs, {
    cwd: REPO_ROOT,
    encoding: 'utf8',
    stdio: capture ? ['inherit', 'pipe', 'inherit'] : 'inherit',
  });
  if (res.error) {
    if (res.error.code === 'ENOENT') {
      const hint = cmd === 'gcloud' ? ' Install the Google Cloud SDK and authenticate: gcloud auth login.' : '';
      die(`'${cmd}' was not found on PATH.${hint}`);
    }
    die(`failed to run ${cmd}: ${res.error.message}`);
  }
  if (res.status !== 0) {
    die(`\`${cmd} ${cmdArgs.join(' ')}\` exited with status ${res.status}`);
  }
  return capture ? res.stdout : '';
}

/** Resolve the GCP project: explicit override wins, else the gcloud default. */
function resolveProject(override) {
  if (override) return override;
  const project = firstNonEmptyLine(run('gcloud', ['config', 'get-value', 'project'], { capture: true }));
  if (!project || project === '(unset)') {
    die('no gcloud project configured. Pass --project <id> or run: gcloud config set project <id>');
  }
  return project;
}

/** The operator's own OAuth access token (NOT the CI deploy SA's). */
function accessToken() {
  const token = firstNonEmptyLine(run('gcloud', ['auth', 'print-access-token'], { capture: true }));
  if (!token) {
    die('could not obtain an access token. Run: gcloud auth login');
  }
  return token;
}

/** Build + deploy the staged server with --no-promote (traffic stays put). */
function deployStagedNoPromote() {
  console.log('\n==> Building and deploying the staged server with --no-promote (no traffic switch)\n');
  // Reuse the proven staged build/deploy orchestration; do NOT pass --version,
  // so gcloud auto-generates the version id we then discover below.
  run('bash', [path.join(REPO_ROOT, 'scripts/deploy-web-staged.sh'), '--no-promote']);
}

/**
 * The id of the most-recently-created `default` version. We query by createTime
 * immediately after our deploy rather than scraping the deploy output (gcloud's
 * human output format is not a stable contract). Tradeoff: if another deploy of
 * the same service races this one, the newest version could be theirs -- this
 * is a single-operator manual canary tool, so that race is acceptable; the
 * printed URL lets the operator sanity-check the id before promoting.
 */
function latestVersionId(project) {
  const out = run(
    'gcloud',
    [
      'app',
      'versions',
      'list',
      '--service=default',
      '--sort-by=~version.createTime',
      '--limit=1',
      '--format=value(id)',
      `--project=${project}`,
    ],
    { capture: true },
  );
  const id = firstNonEmptyLine(out);
  if (!id) {
    die('could not determine the deployed version id from `gcloud app versions list`');
  }
  return id;
}

/**
 * The canary's URL + bare host, via `gcloud app browse` (region-aware -- we do
 * NOT hand-construct the appspot host, since the region segment varies).
 */
function versionUrlAndHost(project, id) {
  const out = run(
    'gcloud',
    ['app', 'browse', '--no-launch-browser', '--service=default', `--version=${id}`, `--project=${project}`],
    { capture: true },
  );
  const url = extractUrl(out);
  if (!url) {
    die(`could not extract the canary URL from \`gcloud app browse\` output:\n${out}`);
  }
  return { url, host: hostFromUrl(url) };
}

/** Stop a version's instances (cleanup). Delete is mentioned as an option. */
function stopVersion(project, id) {
  run('gcloud', ['app', 'versions', 'stop', id, '--service=default', `--project=${project}`]);
}

/**
 * The `default` service's current traffic split as a { versionId: fraction }
 * map. Read from `gcloud app services describe` so cleanup can avoid stopping a
 * version that is serving traffic. Returns {} if the service has no split yet.
 */
function serviceTrafficAllocations(project) {
  const out = run('gcloud', ['app', 'services', 'describe', 'default', '--format=json', `--project=${project}`], {
    capture: true,
  });
  let parsed;
  try {
    parsed = JSON.parse(out);
  } catch {
    die(`could not parse \`gcloud app services describe default\` JSON output:\n${out}`);
  }
  return parsed?.split?.allocations ?? {};
}

/**
 * GET the project's Identity Toolkit config. This MUST succeed before any
 * PATCH: the read-modify-write below depends on a real current list, never an
 * assumed-empty one. Returns the parsed config object.
 */
async function getIdentityConfig(project, token) {
  const res = await fetch(`${IDENTITY_TOOLKIT_BASE}/projects/${encodeURIComponent(project)}/config`, {
    headers: { authorization: `Bearer ${token}` },
  });
  if (!res.ok) {
    const body = await res.text();
    const hint =
      res.status === 403
        ? ' (does your account have roles/firebaseauth.admin? this needs the operator creds, not the CI deploy SA)'
        : '';
    die(`Identity Toolkit GET config failed: ${res.status} ${res.statusText}${hint}\n${body}`);
  }
  // Parse defensively: a 200 whose body isn't JSON is a real (if rare) failure
  // we want to surface clearly, not a confusing low-level throw.
  const text = await res.text();
  try {
    return JSON.parse(text);
  } catch {
    die(`Identity Toolkit GET config returned ${res.status} with a non-JSON body:\n${text}`);
  }
}

/**
 * PATCH authorizedDomains with `?updateMask=authorizedDomains`. The mask scopes
 * the write to exactly that field, but the API REPLACES the whole repeated
 * field with the body's list -- so `domains` MUST be the full desired list
 * (current entries + the change), which the pure add/remove helpers produce
 * from a freshly GET-ed current list. Sending only the new host here would wipe
 * localhost / firebaseapp.com / app.simlin.com.
 */
async function patchAuthorizedDomains(project, token, domains) {
  const res = await fetch(
    `${IDENTITY_TOOLKIT_BASE}/projects/${encodeURIComponent(project)}/config?updateMask=authorizedDomains`,
    {
      method: 'PATCH',
      headers: { authorization: `Bearer ${token}`, 'content-type': 'application/json' },
      body: JSON.stringify({ authorizedDomains: domains }),
    },
  );
  if (!res.ok) {
    const body = await res.text();
    die(`Identity Toolkit PATCH config failed: ${res.status} ${res.statusText}\n${body}`);
  }
  // The response body is discarded, so do NOT parse it: a successful PATCH whose
  // body isn't JSON must not be reported as a failure AFTER the change applied.
}

function printDomains(label, domains) {
  console.log(`    ${label}:`);
  for (const d of domains) {
    console.log(`      - ${d}`);
  }
  if (domains.length === 0) {
    console.log('      (none)');
  }
}

function printHelp() {
  console.log(
    [
      'Deploy a NON-TRAFFIC canary to App Engine and authorize its host in Firebase.',
      '',
      'Usage:',
      '  node scripts/deploy-canary.mjs [--project <id>]',
      '      Build + deploy HEAD with --no-promote, then add the canary host to',
      '      Firebase authorizedDomains so you can log in and smoke-test it.',
      '',
      '  node scripts/deploy-canary.mjs --cleanup <version> [--project <id>]',
      '      Remove that version host from authorizedDomains, then stop the version',
      '      UNLESS it is serving traffic (a promoted canary is production; the stop',
      '      is refused so cleanup can never cause an outage).',
      '',
      'Project defaults to `gcloud config get-value project`; override with',
      '--project or the SIMLIN_CANARY_PROJECT env var. Mutating Firebase auth',
      'config requires roles/firebaseauth.admin on YOUR account (this uses your',
      'own gcloud credentials, not the CI deploy service account). Traffic is',
      'never promoted; the deploy + promote are separate steps.',
    ].join('\n'),
  );
}

async function runDeployMode(project, token) {
  // Preflight: validate Firebase access BEFORE the (long) deploy, so a missing
  // firebaseauth.admin grant fails in seconds rather than after an upload.
  console.log('==> Preflight: reading current Firebase authorized domains');
  const preflight = await getIdentityConfig(project, token);
  printDomains('current authorizedDomains', preflight.authorizedDomains ?? []);

  deployStagedNoPromote();

  const versionId = latestVersionId(project);
  const { url, host } = versionUrlAndHost(project, versionId);
  console.log(`\n==> Canary version: ${versionId}`);
  console.log(`    Canary URL:     ${url}`);
  console.log(`    Canary host:    ${host}`);

  // Read-modify-write against a FRESH GET (the config could have changed during
  // the deploy), then PATCH the full merged list.
  console.log('\n==> Authorizing the canary host in Firebase');
  const current = await getIdentityConfig(project, token);
  const before = current.authorizedDomains ?? [];
  const after = addAuthorizedDomain(before, host);
  printDomains('before', before);
  if (after.length === before.length) {
    console.log(`    ${host} is already authorized -- no change.`);
  } else {
    await patchAuthorizedDomains(project, token, after);
    printDomains('after', after);
  }

  printPostDeploy(project, versionId, url);
}

function printPostDeploy(project, versionId, url) {
  const projectFlag = `--project=${project}`;
  console.log(
    [
      '',
      '============================================================',
      'Canary is deployed and authorized -- traffic NOT switched.',
      '============================================================',
      '',
      `Canary URL: ${url}`,
      '',
      'Smoke test the canary (against the URL above):',
      `  - curl -sI ${url}/  -> 200 HTML, links a hashed /static/js/index.<hash>.js`,
      `  - curl -sI ${url}/static/js/sd-component.js  -> 200 (the embed component)`,
      `  - curl -sI ${url}/static/wasm/<hash>.module.wasm  -> 200, content-type: application/wasm`,
      '  - In a browser: LOG IN WITH GOOGLE (this is what the authorized-domain step enables),',
      '    land on Home with no console errors.',
      '  - Open an example model and confirm it simulates; edit + save + reload persists.',
      '',
      'When the smoke test PASSES, switch 100% of traffic to the canary:',
      `  gcloud app services set-traffic default --splits=${versionId}=1 ${projectFlag}`,
      '',
      'Cleanup -- two cases:',
      `  (a) ABANDONING the canary (did NOT promote): de-authorizes the host AND stops`,
      '      the version, which is safe because it serves no traffic:',
      `        node scripts/deploy-canary.mjs --cleanup ${versionId} ${projectFlag}`,
      '  (b) AFTER PROMOTING: this version is now production. The same cleanup command',
      '      is safe to run -- it will de-authorize the host but REFUSE to stop a',
      '      serving version. To reclaim resources, stop the PREVIOUS (now-idle)',
      '      version instead, not this one:',
      `        gcloud app versions list --service=default ${projectFlag}`,
      `        gcloud app versions stop <previous-version> --service=default ${projectFlag}`,
      '',
      'NOTE: keep the canary host authorized only as long as you are testing it;',
      'leaving stale appspot hosts in authorizedDomains widens the OAuth surface.',
    ].join('\n'),
  );
}

async function runCleanupMode(project, token, versionId) {
  // Derive the host the same way the deploy path did (browse is region-aware).
  // Do this BEFORE stopping the version so browse can still resolve it.
  const { host } = versionUrlAndHost(project, versionId);
  console.log(`==> Cleaning up canary version ${versionId} (host ${host})`);

  console.log('\n==> Removing the canary host from Firebase authorized domains');
  const current = await getIdentityConfig(project, token);
  const before = current.authorizedDomains ?? [];
  const after = removeAuthorizedDomain(before, host);
  printDomains('before', before);
  if (after.length === before.length) {
    console.log(`    ${host} was not authorized -- no change.`);
  } else {
    await patchAuthorizedDomains(project, token, after);
    printDomains('after', after);
  }

  // Guard the stop: if the operator promoted this canary, it is now serving
  // production traffic and stopping it is a full-site outage. De-authorizing the
  // host above is always safe; only the stop is dangerous, so we refuse it here
  // rather than blindly running it.
  const share = versionTrafficShare(serviceTrafficAllocations(project), versionId);
  if (share > 0) {
    const pct = (share * 100).toFixed(share < 0.01 ? 2 : 0);
    console.log(
      [
        '',
        `REFUSING TO STOP version ${versionId}: it is serving ${pct}% of default-service traffic.`,
        'De-authorized the host only. If you promoted the canary, it IS production now --',
        'stopping it would take the site down. To reclaim resources, stop the PREVIOUS',
        '(now-idle) version instead:',
        `  gcloud app versions list --service=default --project=${project}`,
        `  gcloud app versions stop <previous-version> --service=default --project=${project}`,
      ].join('\n'),
    );
    return;
  }

  console.log('\n==> Stopping the canary version (it is serving no traffic)');
  stopVersion(project, versionId);
  console.log(
    [
      '',
      `Done. Version ${versionId} is stopped and its host is de-authorized.`,
      'To remove the version entirely (frees the slot toward the GAE version cap):',
      `  gcloud app versions delete ${versionId} --service=default --project=${project}`,
    ].join('\n'),
  );
}

async function main() {
  const args = parseArgs(process.argv.slice(2));
  if (args.mode === 'help') {
    printHelp();
    return;
  }

  const project = resolveProject(args.project ?? process.env.SIMLIN_CANARY_PROJECT);

  console.log('============================================================');
  console.log(`Project: ${project}`);
  console.log('WARNING: this touches PRODUCTION Firebase auth config and');
  console.log('deploys to production App Engine. It does NOT promote traffic.');
  console.log('============================================================\n');

  const token = accessToken();

  if (args.mode === 'cleanup') {
    await runCleanupMode(project, token, args.version);
  } else {
    await runDeployMode(project, token);
  }
}

// Run only when executed directly, so importing the pure helpers in tests has
// no side effects (mirrors the guard-free build-deploy-staging.mjs by gating
// the entrypoint instead).
const invokedDirectly = process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href;
if (invokedDirectly) {
  main().catch((err) => die(err.message));
}
