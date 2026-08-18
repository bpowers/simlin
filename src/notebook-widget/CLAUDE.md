# @simlin/notebook-widget

The anywidget front-end module (AFM) that hosts `@simlin/diagram`'s `Editor` inside a notebook cell for pysimlin's `ModelWidget` (JupyterLab 4, Notebook 7, VS Code, Colab). It builds to ONE self-contained ES module, `dist/widget.js`, which pysimlin ships inside its wheel as the widget's `_esm`; the engine wasm (`src/engine/core/libsimlin-browser.wasm`) ships beside it in the wheel and reaches the browser over the widget comm at runtime. Design: [docs/design-plans/2026-08-17-pysimlin-widget.md](/docs/design-plans/2026-08-17-pysimlin-widget.md).

For global development standards, see the root [CLAUDE.md](/CLAUDE.md). For the TypeScript conventions, see [docs/dev/typescript.md](/docs/dev/typescript.md).

## Hosting model (why everything here looks the way it does)

anywidget loads `_esm` by writing the module text into a `blob:` URL and `import()`ing it (anywidget `load.ts`), once per widget instance (`runtime.ts` builds a `Runtime` per model, each with a fresh blob URL). Three consequences drive this package:

- **No relative assets.** `import.meta.url` is a blob URL, so nothing next to the module can be fetched: no worker chunk, no `.wasm`, no `.css`, no font files. The bundle injects its CSS from JS (`output.injectStyles`), inlines fonts as data URIs, and receives the wasm as bytes over the comm. Anything that would emit a second output file is a bug in this package's build; rspack's ESM runtime also defines its base URI by resolving `./` against `import.meta.url`, which THROWS for blob URLs at module init -- `tools.cssLoader.esModule: false` keeps `url()` handling out of that runtime (see rsbuild.config.ts).
- **The page is someone else's.** The CSS lands in the notebook's `<head>` (no shadow root), so every global stylesheet the Editor needs -- `theme.css` (design tokens on `:root`, a universal reduced-motion rule) and katex (`body { counter-reset }`) -- is rewritten at build time to live under the widget root class `.simlin-notebook-widget` (`build/scope-css.ts`, a PostCSS plugin wired through `tools.postcss`; CSS Modules are skipped, their classes are hashed already). `build/scope-css.test.ts` runs the plugin over the real theme.css and katex.min.css and pins that no top-level selector survives without the root class, and the Playwright journey checks the page root sees none of the tokens. N displayed widgets still inject N (identical, scoped) copies of the styles -- harmless, and unavoidable while each instance is its own module.
- **Module state is per widget instance.** Every displayed widget gets its own copy of this module, its own React, and its own `@simlin/engine` wasm singleton. The only page-wide sharing possible is through `globalThis`, so `engine-bootstrap.ts` caches the COMPILED `WebAssembly.Module` (the expensive part) under `globalThis.__simlinWidgetWasmModule` and each instance instantiates its own engine from it via `ready(module)`. Two cells displaying models compile the wasm once and ask the kernel for it once.
- **`initialize` must return fast.** anywidget rejects a model whose module load plus `initialize` exceed ~2 s (`runtime.ts`, `AbortSignal.timeout(2000)`), after which every comm message for that model is dropped. `initialize` only kicks off the engine bootstrap; `render` awaits it. anywidget also queues comm messages until the module has loaded (`_handle_comm_msg` awaits `runtime.ready`), so an early kernel reply is safe.

## Engine strategy

- Wasm flavor: `@simlin/engine/internal/wasm` is aliased to `@simlin/engine/internal/wasm.supplied` -- no bundled artifact, no asset fetch; `init()` without a source throws. The bootstrap hands `ready()` the shared compiled module.
- Backend: `@simlin/engine/internal/backend-factory` is aliased to `@simlin/engine/backend-factory.direct` -- libsimlin runs on the main thread. Chosen over the Web Worker backend because a worker needs a second file (or an inline-blob worker built in a second compilation stage plus a new engine factory that spawns from source), and the widget's simulations are the Editor's sparklines and small models -- heavy runs happen in the kernel. Cost: opening/compiling a very large model blocks the notebook UI for the duration (seconds for a C-LEARN-sized model). The measured worker chunk is small (`SIMLIN_WIDGET_BACKEND=worker pnpm build`: ~38 KB raw / ~10 KB gz, self-contained), so an inline-blob worker is a contained follow-up if main-thread jank becomes a problem -- it is not a hack, just a second build stage; the as-built worker variant is NOT loadable from a blob URL (its chunk URL is resolved against `import.meta.url`).
- Wasm handshake (`engine-bootstrap.ts`): the first instance on the page `model.send({type:'wasm'})`; the kernel answers with a custom message `{type:'wasm'}` whose first binary buffer is the artifact (`model.on('msg:custom', (msg, buffers) => ...)`, buffers arrive as `DataView`s from ipywidgets), or `{type:'wasm', error}` when it cannot. A reply without a buffer is an error, not a hang; no reply within 60 s is an error; a failed shared promise is dropped so a later widget retries.

## Model contract

Trait names live in `widget-core.ts` `TRAITS`. Kernel-owned: `project_json` (whole-project engine-native JSON snapshot), `revision` (integer), `height`, `theme` (`auto|light|dark`), `read_only`. Widget-written: `project_json` + `pending_base` (in one sync), `selection`. Custom messages: `{type:'wasm'}` (widget -> kernel), `{type:'wasm'}` + binary buffer / `{type:'wasm', error}` (kernel -> widget), `{type:'notice', text, level?}` (kernel -> widget).

### Kernel obligations (MUST)

1. **Seed**: set `project_json` and `revision` before the widget is displayed, and on every accepted change afterwards.
2. **Accept**: after accepting a widget snapshot, push `project_json` as the EXACT bytes the widget sent (no re-serialization -- the widget matches echoes by string equality) and `revision` incremented by EXACTLY one, in ONE sync message (traitlets `hold_sync()`; because the trait already holds the widget's bytes, assigning the equal value is silent, so `send_state('project_json')` explicitly).
3. **Reject** (stale `pending_base`): never write the file from the snapshot; re-push the authoritative `project_json` (and the unchanged `revision`) so the widget re-seeds from it. A `{type:'notice', level:'warn'}` custom message may accompany it.
4. **Notices are custom messages, not a trait**: `{type:'notice', text: string, level?: 'info'|'warn'}` -- a notice is an event (a second identical "Updated on disk" must show again), which a trait cannot express. Sent alongside a disk reload or a reject.
5. **Wasm**: answer `{type:'wasm'}` with a custom message `{type:'wasm'}` whose first binary buffer is `libsimlin-browser.wasm`, or `{type:'wasm', error: string}` when the artifact cannot be supplied.
6. Only the kernel ever sets `revision`; the widget never does.

### Widget guarantees

- Every save is one `save_changes()` carrying `project_json` (the snapshot) and `pending_base` (the revision it was edited from). A snapshot equal to the trait's current value is not sent (ipywidgets sends only changed keys, so no echo could come back) and does not advance the Editor's acknowledged version.
- `onSave` returns `pending_base + 1` optimistically (`optimisticVersionAfterSave`) so consecutive edits chain without a round trip. This assumes obligation 2; if a kernel ever fails it (or a write fails and `revision` stays put), the next snapshot is rejected as stale and obligation 3 re-seeds the widget -- a remount, never a wrong write, because the kernel is authoritative.
- Both `change:revision` and `change:project_json` (skipping the widget's own synchronous sets) read the current pair and run the idempotent `reconcileRevision`, so it does not matter whether a push surfaces as one change event or two, in either order:
  - the pushed JSON equals a snapshot this widget sent and has not seen echoed -> **ack**: adopt the revision, keep the live Editor and its undo history; the OLDEST pending entry is dropped (position, not value: an edit/undo/redo burst is `[A, B, A]`);
  - the pair is exactly what the widget already knows and nothing is pending -> nothing (the second change event of a push already handled);
  - anything else -> **remount** `<Editor key=revision#generation>` on the pushed snapshot: a Python `edit()`, a disk reload, a revision bump, or a reject re-seed -- including a reject at an UNCHANGED revision (the trait held our bytes, the kernel re-pushed its own, so a change event fires; `generation` is what makes the key change). Every pending snapshot is discarded then; the kernel has moved past them.
- Pending snapshots are a bounded queue (`MAX_PENDING_SNAPSHOTS`); a burst of quick edits against a busy kernel can leave several in flight.
- `selection` is set (and `save_changes()`d) at most every 150 ms.
- Notices auto-hide after 5 s; a repeat restarts the timer. `theme: 'auto'` follows JupyterLab's `body[data-jp-theme-light]` (a `MutationObserver`) else `prefers-color-scheme` (a `matchMedia` listener), live, and both are unsubscribed on unmount.

### Known limitation: two views of one model

anywidget runs `render` once per VIEW; two views of the same model (`display(w)` twice, or a JupyterLab "create new view for output") each mount their own Editor and their own pending queue against ONE model. Their saves interleave on the same traits, so an accept echo for view A is a foreign push for view B (it remounts, losing B's undo history and any edit it has not yet autosaved). Correctness holds -- the file is only ever written from a snapshot the kernel accepted -- but two live editors of one model on one page is not a supported editing arrangement; display two `ModelWidget` instances (two cells) instead, which each carry their own model.

## Files

- `src/index.tsx` -- AFM entry, `export default { initialize, render }`. Imports `@simlin/diagram/theme.css` and KaTeX's `katex.min.css` explicitly and deep-imports `@simlin/diagram/Editor` -- the package root would also pull in `reset.css`, a global page reset that must never land in someone else's notebook page. Mounts React 19 into a div appended to `el`; placeholder while the engine loads; failure text in the cell.
- `src/WidgetApp.tsx` -- The React shell: wrapper `<div class="simlin-notebook-widget" data-lm-suppress-shortcuts data-theme=... style={position:relative;height:Npx;width:100%}>` (JupyterLab checks `data-lm-suppress-shortcuts` via `closest()` to keep notebook shortcuts out of the widget; the Editor's chrome is absolutely positioned against the wrapper), the notice toast, and `<Editor inputFormat="json" ...>` keyed by revision + generation. Subscribes to the trait change events, `msg:custom` notices, and the host theme signals; `onSave` / `onSelectionChanged` wiring.
- `src/widget-root-class.ts` -- `WIDGET_ROOT_CLASS`, shared by the shell and the build (the CSS scoping selector).
- `src/engine-bootstrap.ts` -- Page-global compiled-module cache + per-instance `ready()`; the wasm request/reply over the comm with timeout.
- `src/widget-core.ts` -- Functional core: trait coercion (`readTraits`), `resolveTheme`, `wrapperStyle`, `parseWasmReply`, `parseNoticeMessage`, the revision reconciliation (`SyncState`, `recordSentSnapshot`, `reconcileRevision`), `optimisticVersionAfterSave`. No DOM, no React, no model.
- `build/scope-css.ts` -- The PostCSS plugin that confines global stylesheets to the widget root (see Hosting model).
- `src/anywidget-model.ts` -- The slice of anywidget's `AnyModel` this package uses (verified against anywidget 0.11.0), so no `@anywidget/types` dependency.
- `rsbuild.config.ts` -- The single-file ESM build (see its header comment for every choice). `SIMLIN_WIDGET_BACKEND=worker` builds the measurement-only worker variant.
- `e2e/` -- Playwright feasibility journey (`pnpm test:e2e`): serves `dist/widget.js`, the harness page, and the wasm from disk via `page.route()`, imports the bundle through a blob URL per mount exactly as anywidget does, plays the kernel's side of the wasm handshake with a fake `AnyModel` (`e2e/harness/fake-anywidget-model.js`), adds a variable through the real Editor UI, asserts the snapshot lands on the model, that a second module instance reuses the compiled wasm (no second request), that a kernel push remounts, and that nothing outside the harness is fetched. Requires a prior `pnpm build` here and in `src/engine`. On a host without Playwright's system libraries, point `PLAYWRIGHT_BROWSERS_PATH` at a matching browser tree (nixpkgs `playwright-driver.browsers` matches the pinned Playwright revision).
- `src/test-utils/` -- `fake-model.ts` (unit-test twin of the e2e shim), `editor-mock.tsx`, `engine-mock.ts`; wired by `rstest.config.mts` aliases so the shell tests never touch the wasm.

## Tests

`pnpm test` (rstest, jsdom): `widget-core.test.ts` covers every coercion/decision arm of the core (including the `[A, B, A]` and `[S1, S2, S3]` echo bursts and the unchanged-revision reject); `engine-bootstrap.test.ts` the handshake, timeout (and its clearing), page-global cache and retry semantics; `index.test.tsx` the AFM lifecycle and the model protocol (save sets, equal-JSON save sends nothing, echo keeps the Editor, foreign push remounts once per push, the unchanged-revision reject remounts and re-seeds the version, notices via custom message, height/theme/read_only, live theme switching, selection debounce, cleanup removes every listener and observer); `build/scope-css.test.ts` the CSS scoping over the real stylesheets. `pnpm test:e2e` is the bundle-level journey above; it is not part of `pnpm test`.

## Size

Measured 2026-08-17 (rsbuild 2.1.5 production build, `gzip -9`; wasm-opt version 125 with build.sh's `-O3`):

| Artifact | raw | gzip |
|---|---|---|
| `dist/widget.js` | 1,570 KB | 617 KB |
| of which inlined KaTeX woff2 faces (20 files, base64) | ~350 KB (260 KB of font bytes) | ~262 KB |
| `libsimlin-browser.wasm`, wasm-opt'd (mode stamp `opt`) | 5,298 KB | 1,769 KB |
| `libsimlin-browser.wasm`, unoptimized (`DISABLE_WASM_OPT=1`, what the pre-commit hook leaves in the engine's core directory) | 6,592 KB | 1,578 KB |

The wasm-opt'd blob is 20% smaller raw but 12% LARGER gzipped: cargo already builds wasm32 at `opt-level = "z"` (`.cargo/config.toml`), whose output is repetitive and compresses well, while `wasm-opt -O3` trades that regularity for speed (inlining, unrolling) -- fewer bytes, higher entropy. Which number matters depends on the channel: the widget comm carries the raw bytes (websocket frames are not compressed by default), so the wasm-opt'd blob is the one to ship; the wheel is a zip, where the difference is a wash. Roboto is deliberately NOT bundled: the Editor's stylesheets fall back through `Roboto, Helvetica, Arial, sans-serif`, hosts that have Roboto (JupyterLab does not by default) use it, and shipping the four woff2 subsets simlin-serve self-hosts would add ~60 KB for a font the notebook chrome does not use.
