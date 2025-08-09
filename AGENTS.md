# Repository Guidelines

## Project Structure & Module Organization
- Yarn workspaces under `src/`: `app` (React UI), `server` (Express API), `diagram` (editor components), `core` (shared TS utils), `engine`/`importer`/`xmutil-js` (WASM bindings). Site docs in `website/`.
- Rust workspace under `src/`: `simlin-engine` (core), `engine`, `importer`, `simlin-cli`, `simlin-compat`, `xmutil`.
- Tests: Playwright UI tests in `ui-tests/{visual,integration}`; model fixtures in `test/`; Playwright output in `test-results/`.
- Assets: `public/`, `stdlib/*.stmx` (compiled to protobuf), protobufs in `src/**/schemas`.

## Build, Test, and Development Commands
- `yarn start:firestore` / `yarn start:backend` / `yarn start:frontend`: Run emulators, API (3030), and UI (3000) locally.
- `yarn build` / `yarn clean`: Build or clean all workspaces; compiles Rust→WASM.
- `yarn test`: Run Playwright tests; `yarn test:visual`, `yarn test:integration` for specific suites.
- `cargo test`: Run Rust engine tests.
- `yarn lint` / `yarn format`: Lint and format TS and Rust (`cargo clippy`, `cargo fmt`).
- `yarn build:gen-protobufs` / `yarn rebuild-stdlib`: Regenerate TypeScript protobufs and stdlib model protobufs.

## Coding Style & Naming Conventions
- TypeScript/React formatted by Prettier; linted via ESLint configs (`eslint.config.js`). Prefer 2‑space indent, single quotes, and trailing commas where Prettier applies.
- Rust formatted with `cargo fmt`; keep clippy clean. Prefer explicit `unwrap()` vs defaults when assumptions must fail.
- File names: TS source in package folders (`src/<pkg>/`), tests `*.spec.ts` under `ui-tests/`.

## Testing Guidelines
- UI: Playwright projects `visual` and `integration` (naming: `ui-tests/visual/foo.spec.ts`). Update snapshots with `yarn test:visual:update`.
- Integration tests require services running; Playwright config can auto-start servers.
- Rust: Unit/integration tests live alongside crates; run via `cargo test`.

## Commit & Pull Request Guidelines
- Commit format: `<scope>: <imperative summary>`. Scopes from history include `engine`, `engine/expr2`, `build`, `rust`, `testing`, `doc`. Keep scope lowercase; keep summary concise.
- Examples: `engine: implement broader support for indexing`, `build: fix JS build`, `testing: add basic visual regression tests`, `doc: update design doc`. Use `Revert "..."` when reverting.
- Commits: Imperative mood, focused scope, reference issues (`Fix #123`). Example: "Add stock graph pan/zoom behavior".
- PRs: Clear description, linked issues, steps to test, and screenshots for UI changes. Note any schema or data migrations.
- Run `yarn precommit` (or install hooks via `yarn install-git-hooks`) before pushing.

## Security & Configuration Tips
- Do not commit secrets; use local emulators (Firestore 8092, Auth 9099). Cloud deploy uses `app.yaml`/`gcloud`.
- Ensure Rust toolchain from `rust-toolchain.toml`; Node with Yarn. Install `wasm-bindgen-cli` for WASM workflows.
