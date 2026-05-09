# Deploying to production

**Last reviewed:** 2026-05-08

The web app at `app.simlin.com` runs on Google App Engine standard. GAE serves the static React SPA built from `src/app` and runs the Express backend in `src/server` (Firebase Auth, models persisted in Firestore as protobuf). `@simlin/mcp`, `@simlin/serve`, `pysimlin`, and `simlin-cli` are released separately to npm/PyPI -- they aren't part of this deploy.

Read this before your first deploy after a long gap: the deploy is one local command with no CI gate, the production config (`.app.prod.yaml`) isn't in the repo, and the only rollback is a GAE traffic split.

## Prerequisites

- `gcloud` authenticated against the production project (`gcloud auth login`, `gcloud config set project ...`).
- A Rust toolchain with the `wasm32-unknown-unknown` target (the toolchain file pins it; `rustup show` to check) and `wasm-opt` on `PATH`. Without `wasm-opt` the deploy still works but ships an unoptimized ~5.6 MB WASM blob. `./scripts/dev-init.sh` sets most of this up.
- Node 24 locally, matching the GAE runtime.
- A clean working tree. The deploy scripts copy build output into the tracked `public/` directory and `git checkout` it back afterward; starting dirty makes that unreliable.
- `.app.prod.yaml` in the repo root. It's gitignored -- you keep it locally. See [The two app.yaml files](#the-two-appyaml-files) below.

## The deploy command

```bash
pnpm deploy
```

Which expands to:

```
export NODE_ENV=production
  && pnpm clean                              # cargo clean + each package's clean script
  && pnpm build                              # pnpm -r run build: Rust+WASM, then every TS package
  && pnpm --filter @simlin/app deploy        # copy build/ and build-component/ into public/; drop symlinks
  && gcloud app deploy ./.app.prod.yaml      # upload the repo minus .gcloudignore, switch traffic
  && pnpm --filter @simlin/app deploy-clean  # git checkout the symlinks and index.html; rm build artifacts
```

`pnpm build` builds *every* workspace package, including `website` (rspress) and `@simlin/serve-web` (vite), neither of which ships. A failure in any of them aborts the deploy before `gcloud` runs. If you've touched only the app or server, `pnpm --filter "@simlin/app..." --filter "@simlin/server..." run build` is a narrower equivalent -- but `pnpm deploy` always runs the full build.

The `src/app deploy` step ends by `rm`-ing the `public` / `default_projects` symlinks (`src/app/public` -> repo `public/`, and likewise under `src/server/`) so `gcloud` doesn't traverse the same content twice; `deploy-clean` restores them with `git checkout`. The upshot: if you interrupt `pnpm deploy` partway, run `git checkout -- public src/server && rm -rf src/app/build src/app/build-component` before retrying.

### Recommended: deploy without promoting, smoke-test, then switch traffic

`pnpm deploy` switches production traffic the moment `gcloud app deploy` finishes. For a routine change that's fine. After a long gap, or when the toolchain or dependencies have moved, split it:

```bash
export NODE_ENV=production
pnpm clean && pnpm build && pnpm --filter @simlin/app deploy
gcloud app deploy ./.app.prod.yaml --no-promote
# note the version URL it prints, e.g. https://<version>-dot-<project>.appspot.com
# run the post-deploy smoke test against that URL
gcloud app services set-traffic default --splits=<version>=1
pnpm --filter @simlin/app deploy-clean
```

## What gets uploaded, and what runs on the instance

`gcloud app deploy` uploads the whole repo except `.gcloudignore` entries: `node_modules`, `target/`, `test/`, `/build*`, `scripts/`, `.github/`, `website/`, `examples/`, `src/jupyter/`, `src/app/public`, `src/app/build*`, `src/server/public`, `src/server/config`, `src/app/firebase.json`, and `.app.prod.yaml` itself. Not excluded, and load-bearing:

- `src/server/lib/`, `src/core/lib/`, `src/engine/lib/`, `src/engine/lib.browser/`, `src/engine/core/libsimlin.wasm` -- the compiled output of `pnpm build`. Gitignored but uploaded; skip the build and you ship stale or missing code.
- `pnpm-lock.yaml`, `pnpm-workspace.yaml`, `.npmrc` -- GAE's Node buildpack runs `pnpm install` at the repo root on the instance, which recreates the `node_modules/@simlin/*` workspace symlinks pointing at the uploaded `src/*/lib`.
- `config/default.json` + `config/production.json` -- the server's config layering.
- `default_projects/` -- example models copied into each new account at signup.
- `public/` -- the SPA static assets, after `pnpm --filter @simlin/app deploy` populates it from `build/`.

On the instance GAE runs `pnpm install`, then the root `start` script: `node src/server/lib`. The server loads the engine WASM (`node_modules/@simlin/engine/core/libsimlin.wasm`) *before* it starts listening, so a missing or unresolvable WASM file crash-loops the instance -- that's the whole site down, not just model previews. The `--no-promote` smoke test catches this.

## The two app.yaml files

`app.yaml` is committed; `.app.prod.yaml` is gitignored and lives only on your machine, and `pnpm deploy` deploys `.app.prod.yaml`. Treat the committed `app.yaml` as the reference -- it carries the handler routes and the `runtime` / `instance_class` / scaling block production should match. Diff `.app.prod.yaml` against it before deploying:

- `runtime`: `nodejs24`. (`nodejs18` is EOL on GAE; `nodejs16`, the runtime of the last deploy before May 2026, is gone entirely -- which is why there's no "redeploy the old commit" rollback.)
- The handler list: `/static`, `/`, `/new`, `/legal*`, `/privacy`, `robots.txt`, `ads.txt`, favicon, `manifest.json`, then `/.*` -> `script: auto`. The SPA's dynamic routes like `/:username/:projectName` fall through to `/.*`, i.e. the Express server.
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
- [ ] `.app.prod.yaml` reconciled against `app.yaml` (`runtime: nodejs24`, handlers, `authentication__seshcookie__key` set to the value already in use).
- [ ] `wasm-opt --version` works; `rustup show` lists the `wasm32-unknown-unknown` target.

## Post-deploy smoke test

Against the `--no-promote` version URL, then again on production:

- `curl -sI https://<host>/` -> 200 HTML. View source: it links a hashed `/static/js/index.<hash>.js` (literal `<%= PUBLIC_URL %>` means the build was skipped) and `/static/css/index.<hash>.css`.
- `curl -sI https://<host>/static/js/sd-component.js` -> 200 -- the embeddable web component; external sites `<script src>` this exact path.
- `curl -sI https://<host>/static/wasm/<hash>.module.wasm` -> 200, `content-type: application/wasm`.
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

## Rough edges

Things to know that don't have a clean fix yet:

- `pnpm deploy` isn't idempotent and has no CI gate. Ctrl-C after `gcloud` starts uploading and `deploy-clean` never runs, leaving built files in `public/` and the symlinks dropped (recover as described under [The deploy command](#the-deploy-command)).
- `deploy-clean` doesn't remove everything the build wrote into `public/` (`public/static/css`, `public/static/media`), so `git status` is noisy after a deploy.
- GAE's Node buildpack installs the full dependency set on the instance -- pnpm v10 with `NODE_ENV=production` no longer skips devDependencies -- which is large and slow, and `.npmrc`'s `strict-peer-dependencies=true` makes an unmet transitive peer abort the build.
- Server-side PNG preview (`src/server/render.ts`) parses and rasterizes user-uploaded models in-process with no size cap beyond the 10 MB request body limit and no timeout.
- There's no error reporting or alerting. Cloud Logging and the GAE metrics dashboard are it.
