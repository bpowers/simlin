# Deploying to production

**Last reviewed:** 2026-07-01

The web app at `app.simlin.com` runs on Google App Engine standard. GAE serves the static React SPA built from `src/app` and runs the Express backend in `src/server` (Firebase Auth, models persisted in Firestore as protobuf). `@simlin/mcp`, `@simlin/serve`, `pysimlin`, and `simlin-cli` are released separately to npm/PyPI -- they aren't part of this deploy.

Read this before your first deploy after a long gap: the deploy is one local command (with a [CI smoke gate](#ci-coverage-of-the-deploy-build) on the assembly path), the production config (`.app.prod.yaml`) isn't in the repo, and the only rollback is a GAE traffic split.

## Prerequisites

- `gcloud` authenticated against the production project (`gcloud auth login`, `gcloud config set project ...`).
- A Rust toolchain with the `wasm32-unknown-unknown` target (the toolchain file pins it; `rustup show` to check) and `wasm-opt` on `PATH`. Without `wasm-opt` the deploy still works but ships an unoptimized ~5.6 MB WASM blob. `./scripts/dev-init.sh` sets most of this up.
- Node 24 locally, matching the GAE runtime.
- A clean working tree. The deploy scripts copy build output into the tracked `public/` directory and `git checkout` it back afterward; starting dirty makes that unreliable.
- `.app.prod.yaml` in the repo root. It's gitignored -- you keep it locally. See [The two app.yaml files](#the-two-appyaml-files) below.

## The deploy command

```bash
pnpm deploy:web
```

The script lives at [`scripts/deploy-web.sh`](/scripts/deploy-web.sh) and runs:

```
export NODE_ENV=production
pnpm clean                                       # cargo clean + each package's clean script
pnpm build                                       # pnpm -r run build: Rust+WASM, then every TS package
pnpm --filter @simlin/app run deploy:assemble    # copy build/ and build-component/ into public/; drop symlinks
gcloud app deploy ./.app.prod.yaml               # upload the repo minus .gcloudignore, switch traffic
# A bash trap runs the cleanup below on EXIT/INT/TERM, even if any step above fails:
pnpm --filter @simlin/app run deploy:clean       # git checkout the symlinks and index.html; rm build artifacts
```

`pnpm build` builds *every* workspace package, including `website` (rspress) and `@simlin/serve-web` (vite), neither of which ships. A failure in any of them aborts the deploy before `gcloud` runs. If you've touched only the app or server, `pnpm --filter "@simlin/app..." --filter "@simlin/server..." run build` is a narrower equivalent -- but `pnpm deploy:web` always runs the full build.

The `deploy:assemble` step ends by `rm`-ing the `public` / `default_projects` symlinks (`src/app/public` -> repo `public/`, and likewise under `src/server/`) so `gcloud` doesn't traverse the same content twice; `deploy:clean` restores them with `git checkout`. Because cleanup is now under a `trap`, a Ctrl-C or failure during `gcloud app deploy` still restores the symlinks and removes the build artifacts before the script exits -- you don't have to recover by hand. (If for some reason the trap itself doesn't run, recover with `git checkout -- public src/server && rm -rf src/app/build src/app/build-component public/static/{js,wasm,css,media} public/asset-manifest.json`.)

The script names `deploy:assemble` / `deploy:clean` / `deploy:web` use a colon because pnpm 10's built-in `pnpm deploy` subcommand silently shadowed any plain `deploy` script; `pnpm deploy` ran the built-in (which errors with `ERR_PNPM_NOTHING_TO_DEPLOY`) instead of our pipeline. Colon-separated names sidestep the collision.

Pass extra flags through to `gcloud` after `--`, e.g. `pnpm deploy:web --no-promote`.

### Smaller deploy: `pnpm deploy:web:staged` (proven locally, pending a real gcloud test)

`pnpm deploy:web` deploys from the workspace root, so GAE's instance `pnpm install` installs every workspace package's dependency closure (~590 MB / 1171 packages) even though the server's runtime closure is ~8 packages. App Engine standard always reinstalls from the deployed `package.json` + lockfile (no vendored-`node_modules` option), so the fix is to deploy a self-contained directory whose `package.json` is just the server's prod closure.

`pnpm deploy:web:staged` does that: it runs the same `clean` -> `build` -> `deploy:assemble` -> `verify-deploy-build.sh` as `deploy:web`, then [`scripts/build-deploy-staging.mjs`](/scripts/build-deploy-staging.mjs) assembles `deploy-staging/` (gitignored) containing `lib/`, `config/`, `default_projects/`, the assembled `public/`, a minimal `package.json`, a `pnpm-lock.yaml`, `.gcloudignore`, and a copy of `.app.prod.yaml` as `app.yaml`; finally it runs `gcloud app deploy deploy-staging/app.yaml`. The two unpublished workspace packages (`@simlin/core`, `@simlin/engine`) are vendored into `deploy-staging/vendor/` and referenced via `file:` deps (a registry install would 404). The `pnpm-lock.yaml` is **seeded from the committed root lockfile** (its `packages`/`snapshots` cache with `importers` dropped) so the deploy pins the exact CI-tested versions instead of re-resolving the semver ranges fresh against the registry; a post-generation guard fails the build if any version drifted from the root lockfile. The pure transforms live in [`scripts/deploy-staging-manifests.mjs`](/scripts/deploy-staging-manifests.mjs) (unit-tested via `pnpm test:scripts`); `node scripts/build-deploy-staging.mjs` can also be run standalone (after a build) to inspect the artifact.

Result locally: **80 MB / 230 packages** installed (vs 590 MB / 1171), a 28.7 MB upload payload; the staged server boots and serves `/`, `/api/user` (401), the embed component, and static assets. This also keeps the upload well under GAE's 10k-file cap.

What's NOT yet verified: the real `gcloud` upload and the nodejs24 buildpack honoring `packageManager: pnpm@10.6.0` (corepack) + accepting the frozen lockfile. Run `pnpm deploy:web:staged --no-promote`, watch the build log to confirm the instance ran a successful frozen `pnpm install`, then run the post-deploy smoke test before switching traffic. Until that passes, `pnpm deploy:web` is the default/fallback.

### Recommended: deploy without promoting, smoke-test, then switch traffic

`pnpm deploy:web` switches production traffic the moment `gcloud app deploy` finishes. For a routine change that's fine. After a long gap, or when the toolchain or dependencies have moved, split it:

```bash
pnpm deploy:web --no-promote
# note the version URL it prints, e.g. https://<version>-dot-<project>.appspot.com
# run the post-deploy smoke test against that URL
gcloud app services set-traffic default --splits=<version>=1
```

(`--no-promote` flows through to `gcloud app deploy` via the script's `"$@"` passthrough; the script still runs `deploy:clean` afterward via the trap.)

### Canary deploy + Firebase login: `pnpm deploy:canary`

The `--no-promote` flow above lets you `curl` the canary, but you cannot actually **log in** to it: Firebase OAuth (`signInWithRedirect` via `auth.simlin.com`) rejects any origin not in Firebase's *Authorized domains*, and the canary is reachable only at a versioned `https://<version>-dot-<project>...appspot.com` URL that isn't on that list. So a full end-to-end test (including Google sign-in, the new-user flow, and saving a model) isn't possible on a bare `--no-promote` version.

[`scripts/deploy-canary.mjs`](/scripts/deploy-canary.mjs) (`pnpm deploy:canary`) closes that gap. It:

1. Reads the target project (`gcloud config get-value project`; override with `--project <id>` or `SIMLIN_CANARY_PROJECT`) and prints a warning that it touches **production** Firebase config and deploys (but does **not** promote traffic).
2. Builds + deploys via `scripts/deploy-web-staged.sh --no-promote` (it does **not** pass `--version`, so gcloud auto-generates the version id).
3. Discovers the just-deployed version id (`gcloud app versions list --sort-by=~version.createTime --limit=1`) and its region-aware URL (`gcloud app browse --no-launch-browser --version=<id>`).
4. Adds that version's host to the Identity Toolkit `authorizedDomains` list via a **read-modify-write**: GET the config, append the host to the full existing list, then PATCH it back with `?updateMask=authorizedDomains`. The PATCH replaces the whole repeated field, so it always sends the complete current-plus-new list -- it never wipes `localhost` / `firebaseapp.com` / `app.simlin.com`. It prints the before/after list.
5. Prints the canary URL, the smoke-test checklist, the exact `gcloud app services set-traffic default --splits=<id>=1` promote command, and the exact cleanup command. **It does not promote traffic.**

After testing, clean up:

```bash
pnpm deploy:canary --cleanup <version>   # de-authorize the host + delete the version
```

Cleanup is the inverse: it removes the host from `authorizedDomains` (same read-modify-write) and deletes the version (freeing its slot toward the GAE version cap). It deletes rather than `stop`s because production uses `automatic_scaling`, for which `gcloud app versions stop` is rejected. The delete is guarded by the version's current traffic share: if you have already promoted the canary it is now production, so cleanup refuses to delete it (de-authorizing the host only) and tells you to delete the previous, now-idle version instead -- cleanup can never cause an outage.

This deliberately uses **your own** credentials (`gcloud auth print-access-token`), not the CI deploy service account: mutating Firebase auth config needs `roles/firebaseauth.admin`, an operator-level grant intentionally kept off the CI SA. The deploy/authorize and the traffic promote are separate, explicit steps so traffic is never switched implicitly. Keep the canary host authorized only while you are testing it -- leaving stale appspot hosts in `authorizedDomains` needlessly widens the OAuth surface.

## What gets uploaded, and what runs on the instance

`gcloud app deploy` uploads the whole repo except `.gcloudignore` entries: `node_modules`, `target/`, `test/`, `/build*`, `scripts/`, `.github/`, `website/`, `examples/`, `src/jupyter/`, `src/app/public`, `src/app/build*`, `src/server/public`, `src/server/config`, `src/app/firebase.json`, and `.app.prod.yaml` itself. Not excluded, and load-bearing:

- `src/server/lib/`, `src/core/lib/`, `src/engine/lib/`, `src/engine/lib.browser/`, `src/engine/core/libsimlin.wasm` -- the compiled output of `pnpm build`. Gitignored but uploaded; skip the build and you ship stale or missing code. The engine builds two WASM artifacts: `libsimlin.wasm` (full, with `png_render` -- the server's preview pipeline needs it) and `libsimlin-browser.wasm` (slim, no PNG rasterization stack, ~28% smaller -- what rsbuild bundles into `public/static/wasm/`). The image carries exactly one copy of each: the slim source artifact and the `*.wasm.raw` build caches are excluded in `.gcloudignore` since browsers fetch the hashed bundled copy.
- `pnpm-lock.yaml`, `pnpm-workspace.yaml`, `.npmrc` -- GAE's Node buildpack runs `pnpm install` at the repo root on the instance, which recreates the `node_modules/@simlin/*` workspace symlinks pointing at the uploaded `src/*/lib`.
- `config/default.json` + `config/production.json` -- the server's config layering.
- `default_projects/` -- example models copied into each new account at signup.
- `public/` -- the SPA static assets, after `pnpm --filter @simlin/app run deploy:assemble` populates it from `build/`.

On the instance GAE runs `pnpm install`, then the root `start` script: `node src/server/lib`. The server loads the engine WASM (`node_modules/@simlin/engine/core/libsimlin.wasm`) *before* it starts listening, so a missing or unresolvable WASM file crash-loops the instance -- that's the whole site down, not just model previews. The `--no-promote` smoke test catches this.

## The two app.yaml files

`app.yaml` is committed; `.app.prod.yaml` is gitignored and lives only on your machine, and `pnpm deploy:web` deploys `.app.prod.yaml`. Treat the committed `app.yaml` as the reference -- it carries the handler routes and the `runtime` / `instance_class` / scaling block production should match. Diff `.app.prod.yaml` against it before deploying:

- `runtime`: `nodejs24`. (`nodejs18` is EOL on GAE; `nodejs16`, the runtime of the last deploy before May 2026, is gone entirely -- which is why there's no "redeploy the old commit" rollback.)
- `build_env_variables.GOOGLE_NODE_RUN_SCRIPTS`: `''`. The local deploy script already runs the monorepo build and stages the exact artifact to upload; this prevents App Engine's Node buildpack from running the root `build` script again during staging.
- `automatic_scaling.max_instances`: `8`. Cost cap so a render storm or crash loop can't fan out F4 instances without bound (issue #694). **Mirror this into `.app.prod.yaml`** -- both deploy scripts run `scripts/validate-app-prod-config.mjs`, which fails the deploy if it's missing or not a positive integer. Raise it deliberately if organic traffic ever needs more instances.
- The `/static` handler's `http_headers` with `Access-Control-Allow-Origin: "*"`. Third-party pages hotlink `sd-component.js`, and its engine worker's WASM fetch (plus fonts/CSS assets) are cross-origin CORS requests against `/static` (issue #688); without this header embeds load their data but the engine never initializes. The assets are public, immutable, and served without credentials, so the wildcard adds no risk. **Mirror this into `.app.prod.yaml`** -- `scripts/validate-app-prod-config.mjs` fails the deploy if the header is missing.
- The handler list: `/static`, `/`, `/new`, `/legal*`, `/privacy`, `robots.txt`, `ads.txt`, favicon, `manifest.json`, then `/.*` -> `script: auto`. The `/` and `/new` static HTML handlers carry CSP/HSTS headers because they bypass Express Helmet. The SPA's dynamic routes like `/:username/:projectName` fall through to `/.*`, i.e. the Express server.
- `env_variables`: the committed `app.yaml` has none; the server needs a couple (next section). They live in `.app.prod.yaml`.

GAE ignores `app.yaml`'s `skip_files` when a `.gcloudignore` exists (it does), so `skip_files` in `.app.prod.yaml` is dead -- maintain `.gcloudignore` instead.

## Environment variables

The server loads `config/default.json`, then `config/production.json` when `NODE_ENV=production`, then overlays every `process.env` key into Express settings with `__` for nesting. What `.app.prod.yaml` must set:

| Variable | Why |
|----------|-----|
| `NODE_ENV=production` | Loads `config/production.json`; enables the HTTPS redirect and Cloud Trace agent; serves static assets from `public/`. |
| `authentication__seshcookie__key` | The AES key sealing the `_Secure-model` session cookie (`config/production.json` has `"key": "IN ENV"`). **Keep the same value across deploys** -- changing it logs everyone out (recoverable: the SPA shows Login, a fresh Firebase sign-in works). Unset, `seshcookie` falls back to the literal string `"IN ENV"` as the key, which is forgeable. |

`PORT` and `GOOGLE_CLOUD_PROJECT` come from GAE. Firebase Admin and Firestore authenticate with the instance's ambient service-account credentials -- no key file. `config/production.json` also has `"IN ENV"` placeholders for `authentication.google.clientID` and `userAllowlist`; current server code reads neither, so they're inert. The GAE default service account needs Firestore read/write; the deploy adds no GCP API surface beyond Firestore and Cloud Trace, so no new IAM grants.

## Pre-deploy checklist

- [ ] CI is green on the commit you're deploying.
- [ ] `git status` is clean.
- [ ] `gcloud config get-value project` is the production project.
- [ ] `gcloud app versions list --service=default` shows a known-good current version -- note its ID, that's your rollback target. (If GAE has garbage-collected it, there is no rollback; see below.)
- [ ] `.app.prod.yaml` reconciled against `app.yaml` (`runtime: nodejs24`, `build_env_variables.GOOGLE_NODE_RUN_SCRIPTS: ''`, `automatic_scaling.max_instances: 8`, handlers including the `/static` `Access-Control-Allow-Origin: "*"` header, `authentication__seshcookie__key` set to the value already in use).
- [ ] `wasm-opt --version` works; `rustup show` lists the `wasm32-unknown-unknown` target.

## Post-deploy smoke test

Against the `--no-promote` version URL, then again on production:

- `curl -sI https://<host>/` -> 200 HTML. View source: it links a hashed `/static/js/index.<hash>.js` (literal `<%= PUBLIC_URL %>` means the build was skipped) and `/static/css/index.<hash>.css`.
- `curl -s https://<host>/healthz` -> 200 `ok`. This is the only check that exercises the Node server: `/` is a GAE static handler and stays green even when every Express instance is crash-looping (e.g. `ServerInitError`). A WASM preload failure aborts boot before the route mounts, so it shows up as a non-responding instance (connection failure / GAE 5xx), not a 503 -- treat any non-200 here as down. (The route's 503 branch is defense-in-depth, not the expected failure signal.)
- `curl -sI https://<host>/static/js/sd-component.js` -> 200 -- the embeddable web component; external sites `<script src>` this exact path.
- `curl -H "Origin: https://example.com" -sI https://<host>/static/js/sd-component.js` -> response includes `access-control-allow-origin: *`. Cross-origin embeds need this on everything under `/static` (worker chunk, WASM -- issue #688). GAE emits `http_headers` unconditionally (the request `Origin` header doesn't matter), so any curl shows it; the earlier smoke checks missed its absence only because they never asserted on response headers. Without it, embeds load data but never initialize the engine.
- `curl -sI https://<host>/static/wasm/<hash>.module.wasm` -> 200, `content-type: application/wasm`.
- Full embed check (the header curl only proves CORS, not the blob-trampoline worker boot): serve `<script src="https://<host>/static/js/sd-component.js"></script><sd-model username="..." projectName="..."></sd-model>` from a different origin (e.g. `python3 -m http.server` on localhost) and confirm the diagram renders and simulates with no console errors.
- `curl -sI` on `/robots.txt`, `/manifest.json`, `/favicon.ico`, `/legal/`, `/privacy/` -> 200; `curl -I http://<host>/` -> 301 to https.
- Browser: log in with Google, land on Home, no console errors.
- New-user flow: sign in with a fresh account, claim a username, confirm the example projects appear and one opens and simulates.
- Open an existing model, edit a variable, save, reload -- the change persisted.
- GAE console: instances ramping with no crash loop; Logs filtered to `severity>=ERROR` clean (no `ServerInitError`, `StaticConfigError`, `renderToPNG:` errors); Dashboard 5xx near zero.

## Rollback

There's no "redeploy the old commit." The last deploy before May 2026 targeted `nodejs16`, which GAE no longer accepts, and that era's dependency tree won't build on a supported runtime. Rollback is a traffic split to a version GAE is still holding:

```bash
gcloud app versions list --service=default
gcloud app services set-traffic default --splits=<known-good-version>=1
```

Instant, no rebuild -- but **lossy** for any model a user edited via the new app: the new engine writes protobuf fields the old engine doesn't recognize, and the old engine drops them on the next save. The Firestore document envelopes (`user`, `project`, `file`, `preview` schemas in `src/server/schemas/`) are unchanged, so the documents themselves stay readable both ways; only the model content inside `File.projectContents` is at risk. If GAE no longer holds a good version, roll forward from the deployed commit instead.

## CI coverage of the deploy build

The `frontend` job in [`.github/workflows/ci.yaml`](/.github/workflows/ci.yaml) runs the deploy assembly path on every push and PR: after the regular `pnpm build` and TypeScript tests, it runs `pnpm --filter @simlin/app run deploy:assemble`, then [`scripts/verify-deploy-build.sh`](/scripts/verify-deploy-build.sh), then `deploy:clean`, then asserts `git status` is empty. The verify script checks that `public/index.html` substituted the `<%= PUBLIC_URL %>` template, references a hashed `/static/js/index.<hash>.js`, that `public/static/js/sd-component.js` exists at the single-level path (the doubled-path regression caught in commit 831392fc), that `public/static/wasm/` has a WASM and that it is the slim browser artifact (no `simlin_project_render_png` export), that `src/engine/core/libsimlin.wasm` is present, non-trivial, and the full artifact (has that export -- a slim WASM there would break every server-side preview render), and that `src/server/lib/index.js` exists. CI does NOT run `gcloud app deploy` -- it only proves the local artifacts the deploy uploads are well-formed.

## Rough edges

Things to know that don't have a clean fix yet:

- `pnpm deploy:web` deploys from the workspace root, so GAE's Node buildpack installs the *whole workspace's* dependency set on the instance -- `@rsbuild/*`, `jest`, `slate`, `radix`, rspress, vite, and every other package's deps (~590 MB / 1171 packages), none needed by the server at runtime. App Engine standard always reinstalls from the deployed `package.json` + lockfile and has no vendored-`node_modules` escape hatch, so the only lever is the deployed manifest. The smaller-deploy fix is implemented as **`pnpm deploy:web:staged`** (see below); it is locally proven but still pending a real `gcloud --no-promote` test, so `deploy:web` remains the default. Tracked in [docs/tech-debt.md](/docs/tech-debt.md) "Web deploy uploads the whole monorepo and GAE installs the full dep set".
- Server-side PNG preview (`src/server/render.ts`) renders user-uploaded models in per-request `worker_threads` workers (each with its own WASM instance) with a 10 s total wall-clock budget per request (queue wait included) and at most 2 concurrent renders -- restoring the isolation the 2022 deploy had (issue #694). What remains rough: there's no model-complexity cap below the 10 MB request body limit, so a pathological model still costs a bounded 10 s worker per attempt before failing with a 500.
- There's no error reporting or alerting. Cloud Logging and the GAE metrics dashboard are it. The Express `/healthz` route exists as an uptime-check target (see the smoke test above), but no Cloud Monitoring notification channel, uptime check, or alerting policy points at it yet -- that ops-side setup is tracked in [issue #693](https://github.com/bpowers/simlin/issues/693).
