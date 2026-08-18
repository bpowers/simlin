# pysimlin Notebook Editor Widget Design

## Summary

`pip install pysimlin` gains a file-backed model (`simlin.open(path)`) and an interactive, in-cell diagram editor built on anywidget. The widget hosts the real `@simlin/diagram` `Editor` with the WASM engine in the browser; every human edit becomes a whole-project snapshot that the kernel writes to the model file on disk, and every programmatic or external change to that file (Python `edit()`, Claude Code editing the file, the `simlin` MCP server) flows back into the widget. The file on disk is the single source of truth, which is the only channel every collaborator -- human, Python, AI agent -- can see. No sidecar server is required, so it works in Colab. Nothing about JupyterLab's build system is involved: the widget is one prebuilt ESM bundle produced by the repo's existing rsbuild toolchain and shipped inside the pysimlin wheel.

## Definition of Done

1. `pip install pysimlin` (no extra packages, no sidecar process) gives `simlin.open(path)` -> a file-backed model that knows its path, saves in the same format it was loaded from (`.stmx`/`.xmile`, `.mdl`, native `.sd.json`), tracks a revision, and reloads when the file changes on disk (so Claude Code editing the file, or the `simlin` MCP server, is picked up).
2. Displaying it in a notebook cell (JupyterLab 4 / Notebook 7 / VS Code / Colab) shows the real `@simlin/diagram` Editor; every edit is written to the file on disk and `model.run()` in the next cell reflects it; programmatic edits (`model.edit()` or an external file write) update the widget.
3. Cheap read-only rendering (`_repr_svg_` / StaticDiagram) is included; result overlays / charts in the widget are out of scope for v1.
4. Build: one prebuilt ESM+wasm bundle shipped in the wheel, built by the existing rsbuild toolchain, covered by an end-to-end test that drives the widget headlessly (not just "it imports").

Decisions locked with the user on 2026-08-17: the file on disk is the authority when kernel and widget disagree; the primary Claude participation path is Claude Code running cells / editing the model file in the same repo (MCP must also work but is not the first-class channel); Colab is a target, so no local sidecar server may be required; native JSON, `.stmx`/`.xmile` and `.mdl` are all first-class on-disk formats and `.mdl` is written in place (the engine's Vensim writer exists; no sidecar); the widget assets ship inside the pysimlin wheel and `anywidget` is a hard dependency.

## Acceptance Criteria

Each criterion names success and failure behaviour. Test names use the prefix `pysimlin-widget.ACn.m`.

**AC1 -- file-backed model (DoD 1)**
- AC1.1 `simlin.open(p)` for `.stmx`, `.xmile`, `.xml`, `.mdl`, `.sd.json` (and `.json` sniffed native-vs-SD-AI) returns a `Model` whose `.project.path == Path(p)` and whose `.revision == 0`. Unknown suffix with unrecognisable content raises `SimlinImportError` naming the path.
- AC1.2 After `with m.edit() as (cur, patch): ...` on an autosaving model, the file at `m.path` is rewritten atomically in its own format (`.stmx`/`.xmile` -> XMILE, `.mdl` -> Vensim, `.sd.json` -> native JSON), `m.revision` increments by exactly 1, `m.dirty` is `False`, and re-opening the file yields the edited variable. With `autosave=False`, the file is untouched, `m.dirty` is `True`, and `m.save()` performs the same write.
- AC1.3 A variable created through `edit()` has a diagram element in the persisted view (incremental layout; existing element positions unchanged), so the widget and `render_svg` show it without a full relayout.
- AC1.4 When the file changes on disk (external writer; not our own echo), the model reloads within the poll interval, `revision` increments, `Model` handles previously obtained from the project remain valid and observe the new contents, cached `base_case` is invalidated, and `m.run()` reflects the change. A file rewritten with unparsable content is NOT loaded: the last-known-good project stays, a `RuntimeWarning` names the parse error, and a subsequent valid write is picked up. If the project has unsaved local edits (`autosave=False`, or a failed autosave), the external change is held back with a `RuntimeWarning` instead of destroying the local edits; `reload()` takes the on-disk version, and `save()` refuses to overwrite a file that changed underneath it (`SimlinRuntimeError`) unless called with `force=True` -- the file is never clobbered silently.
- AC1.5 Our own writes never round-trip as external changes (content-hash echo suppression), and `reload()` is idempotent when the file is unchanged.
- AC1.6 A project with `path is None` (`Project.new()`, `simlin.load()`) can still be displayed and edited; `save()` raises a clear error and `save_as(path)` adopts the path/format.

**AC2 -- interactive widget (DoD 2)**
- AC2.1 Displaying `m` in a notebook renders the `@simlin/diagram` `Editor` in the cell output at the requested height; the same output carries an SVG fallback so static renderers (nbconvert, GitHub) show the diagram.
- AC2.2 An edit made in the widget (e.g. adding an auxiliary, changing an equation, moving an element) is written to `m.path` in its format before the next cell executes; `m.revision` has advanced; `m.run()` in the next cell reflects it.
- AC2.3 `m.edit()` in a cell, or an external write to `m.path`, updates the displayed widget without re-executing the display cell (the Editor remounts on the new revision, showing a brief "updated on disk" notice for external sources).
- AC2.4 Two displays of the same model in different cells stay consistent: an edit in one appears in the other; the engine WASM is instantiated once per page and shared.
- AC2.5 A snapshot the widget sends against a stale revision (kernel advanced in between) is rejected: the widget is re-seeded from the kernel's snapshot with a visible notice, and the file is never written from the stale snapshot.
- AC2.6 Widget keyboard shortcuts (Delete/Backspace/Undo) act only on the focused widget's editor and never fire when typing in its equation editor, and never trigger JupyterLab notebook shortcuts (`data-lm-suppress-shortcuts` set on the widget root).
- AC2.7 `m.selection` reflects the variables currently selected in the widget (so an agent-driven cell can ask "what is the human looking at").
- AC2.8 Verified interactively in Colab and VS Code notebooks (manual checklist recorded in the PR; the automated journey runs on JupyterLab).

**AC3 -- read-only rendering (DoD 3)**
- AC3.1 `m.diagram()` returns an object whose `_repr_svg_` is the engine's SVG for the model; models with no view render via a transient auto layout.
- AC3.2 No result-overlay / chart features are present in the widget in v1.

**AC4 -- build, packaging, tests (DoD 4)**
- AC4.1 A new TypeScript package builds `widget.js` (single ESM, CSS inlined) with rsbuild; the pysimlin wheel contains `simlin/_widget/widget.js` and `simlin/_widget/libsimlin-browser.wasm` (wasm-opt'd), and `import simlin` fails loudly with an actionable message if the assets are missing from an installed wheel.
- AC4.2 An automated Playwright journey launches headless JupyterLab, executes a cell that opens a model, drives the Editor to add a variable, and asserts the file on disk changed and a following `m.run()` cell reports the new variable. It runs in CI (its own job, not the pre-commit hook).
- AC4.3 Fast unit tests cover the sync state machine (pure), the trait handling with a fake anywidget model (Python), and the widget module with a fake `model` shim (rstest), all inside the pre-commit budget.
- AC4.4 `scripts/release-pysimlin.sh` / `release.yml` build the JS assets once on the host before cibuildwheel and package identical assets into every platform wheel.

## Architecture

```
+----------------------- kernel (pysimlin) ------------------------+
|  simlin.open(path) -> Model ---- .project: Project               |
|     Project: path, format, revision, dirty, save(), reload()     |
|        |  edit() -> apply patch -> incremental layout            |
|        |            -> serialize(format) -> atomic write         |
|        |            -> remember hash -> revision++ -> notify     |
|        |  poll thread: stat/hash file -> replace_in_place(json)  |
|        |            -> revision++ -> notify                      |
|        v                                                          |
|  ModelWidget (anywidget.AnyWidget)                                |
|     traits: project_json, revision, selection, height, theme,    |
|             pending_base;  custom msgs: wasm bytes, notices       |
+-------------------^-----------------------------|----------------+
                    | comm (ipywidgets protocol)  |
+-------------------|-----------------------------v----------------+
|  widget.js (ESM)  |  anywidget render(el, model)                  |
|     page-global engine cache (wasm compiled once per page)        |
|     <Editor inputFormat="json" key=revision onSave=...>            |
|        onSave(whole project json, rev) -> model.set + save_changes |
|        change:revision from kernel -> remount with new snapshot    |
+-------------------------------------------------------------------+
                    file on disk (.stmx/.xmile/.mdl/.sd.json)
      <- Claude Code edits it / simlin MCP edit_model / git checkout
```

Three roles, one authority:
- **File on disk** is authoritative. Every writer produces a whole-project snapshot; readers reload.
- **Kernel** owns the file: it is the only process that writes on the widget's behalf, and it is the only place the browser gets its initial snapshot and updates.
- **Browser** owns interaction and undo history; it never writes disk and never talks to a server. It holds its own engine (WASM) so editing is local and instant.

This mirrors what `simlin-serve` already does (registry version counter, echo-hash suppression, watcher, "remount on external change") but with the kernel in the server's role and the ipywidgets comm in place of HTTP+WebSocket. Op-based peer sync was rejected: patch ops are not commutative (whole-variable upserts, positional view ops, per-side uid minting), so op sync would mean building the CRDT layer described in `docs/design/crdt-collaborative-editing.md`. Snapshot + revision is what all three existing hosts implement and test.

## Components and Contracts

### 1. libsimlin additions (Rust FFI)

- `simlin_project_serialize_mdl(project, out_buf, out_len, out_collected_errors, out_err)` -- exposes `simlin_engine::to_mdl_with_warnings`; export warnings are non-fatal and travel on the collected-errors channel (the `apply_patch` convention); hard failures (multiple ordinary models, module instances) go to `out_err`.
- `simlin_project_replace_contents(dst, src, out_err)` -- replaces `dst`'s `datamodel::Project` with a clone of `src`'s under `dst`'s locks (datamodel then db) and incrementally re-syncs the salsa db, so `SimlinModel` handles (which hold `*const SimlinProject` + name) stay valid across a reload. Every on-disk format is covered by composing with the existing `simlin_project_open_*` functions (open the new bytes into a temporary project, replace, unref the temporary), so there is one replace primitive rather than per-format variants. A handle whose model disappears returns a clear bad-model-name error and revives if the model reappears; a `SimlinSim` created before the replace is a stale snapshot for simulation queries (sim-bearing analysis queries mix its results with the current model, so reloading callers re-run).
- The already-present `simlin_project_diagram_sync(project, model, patch_json)` is used incrementally from Python (pass the applied patch, not NULL).

### 2. pysimlin: file-backed project

```python
class Project:
    path: Path | None            # None for in-memory projects
    format: FileFormat | None    # XMILE | MDL | NATIVE_JSON (from suffix; .json sniffed)
    revision: int                # monotonic per process; bumps on every accepted change
    dirty: bool
    autosave: bool
    def save(self) -> None                       # atomic tmp+rename in self.format
    def save_as(self, path, format=None) -> None
    def reload(self) -> bool                     # True if contents changed
    def watch(self, enabled: bool = True, interval: float = 0.5) -> None
    def on_change(self, callback: Callable[[ChangeEvent], None]) -> Unsubscribe
    def _replace_from_bytes(self, data: bytes, fmt: FileFormat) -> None   # in place

class Model:
    path / revision / dirty / save() / reload()  # proxies to .project
    selection: tuple[str, ...]                   # last selection reported by a widget
    def diagram(self) -> Diagram                 # _repr_svg_ object
    def widget(self, *, height: int = 600, theme: str = "auto") -> ModelWidget
    def _repr_mimebundle_(self, **kw)            # widget view + image/svg+xml fallback

def open(path, *, autosave=True, watch=True) -> Model
def load(path_or_bytes) -> Model                 # unchanged semantics; suffix table shared with open()
```

`ChangeEvent(source: "edit"|"widget"|"disk"|"reload", revision: int)`.

Sync state machine (functional core, `simlin/_sync.py`): pure functions over `(revision, last_written_hash, pending_widget_rev)` deciding, for each incoming event (widget snapshot with rev, disk hash observed, local edit), one of `accept -> write`, `reject-stale -> reseed widget`, `ignore-echo`, `reload -> push`. The imperative shell (`Project`, the poll thread, `ModelWidget`) only executes those decisions. Locking: the existing `Project._lock` guards the handle; the poll thread takes it briefly around `_replace_from_bytes`; callbacks fire outside the lock. Trait updates from the poll thread go through the kernel's IO loop when available (ipykernel >= 7.1 tolerates direct sets; older kernels get `add_callback`).

Format dispatch is a single table shared by `open()`, `load()`, `save()`, matching simlin-serve's `format_for_path` plus `.mdl` write and content sniffing for `.json`. XMILE and MDL are regenerated (not byte-preserved) on save, as everywhere else in the repo.

Watching is a stdlib polling thread (stat every 0.5s, hash on mtime/size change) rather than a `watchfiles` dependency: one file, cheap, works on every filesystem including network mounts and Colab's FUSE drives, no native wheel to worry about.

### 3. pysimlin: `ModelWidget` (anywidget)

```python
class ModelWidget(anywidget.AnyWidget):
    _esm = <bundled widget.js>          # read once at import; env SIMLIN_WIDGET_ASSET=url|inline|<http url>
    project_json = Unicode()            # kernel -> widget seed; widget -> kernel snapshot
    revision     = Int()                # kernel-owned; widget echoes the base it edited from
    selection    = List(Unicode())      # widget -> kernel
    height       = Int(600)
    theme        = Unicode("auto")      # auto|light|dark
    pending_base = Int()                # widget -> kernel: revision the snapshot was edited from
    read_only    = Bool(False)
```

Protocol:
- Seed: kernel sets `project_json`+`revision` on construction and on every accepted change.
- Widget edit: Editor `onSave(json, base_rev)` -> JS does `model.set("project_json", json); model.set("pending_base", base_rev); model.save_changes()` (one coalesced sync message). Kernel `observe(project_json)`: if `pending_base == revision` accept (apply via `_replace_from_bytes`, write file, `revision += 1`, echo suppressed by hash) else reject (re-push the kernel's authoritative `project_json` at the unchanged `revision`, plus a conflict notice -> widget remounts). Kernel obligations (MUST): after an accept, push the EXACT bytes the widget sent (no re-serialization) together with `revision + 1` in ONE sync message (`hold_sync()` plus an explicit `send_state` for `project_json`, because assigning an equal value to a traitlet is silent); bump `revision` by exactly one per accepted snapshot; on reject, re-push `project_json` explicitly even though its value is unchanged.
- Kernel change (edit/disk): kernel sets `project_json` and `revision` in one message; JS remounts `<Editor>` on any kernel-originated JSON that is not one of its own pending snapshots -- whether or not `revision` changed (so a reject re-seeds even at an unchanged revision). The widget keeps a bounded queue of sent-but-unacknowledged snapshots and pops the OLDEST entry on a content match, so its own accepted saves never remount (undo history preserved) even for edit/undo/redo bursts while the kernel is busy.
- Notices ("Updated on disk", conflict) travel as custom messages `{type:'notice', text, level}` from kernel to widget, never as a trait: a trait re-set to an equal value sends nothing.
- WASM: on first `render` per page, JS looks for `globalThis.__simlinWidgetWasmModule` (a promise of the compiled `WebAssembly.Module`; the engine backend itself is per module instance). If absent it sends `{type:"wasm"}`; the kernel replies with a custom message `{type:'wasm'}` carrying the wasm bytes as the first binary buffer (or `{type:'wasm', error}`); JS compiles once and caches the promise page-wide, dropping it on failure so a later widget retries. Fallback: `SIMLIN_WIDGET_ASSET=inline` base64-embeds the wasm in `_esm` (Colab-safe but bloats output); `SIMLIN_WIDGET_ASSET=<url>` loads `_esm` from a URL (dev server / CDN). Chosen at import time (anywidget reads `_esm` at class definition), same as rerun's `RERUN_NOTEBOOK_ASSET`.
- Selection: Editor `onSelectionChanged` -> `selection` trait (150 ms debounce as in simlin-serve).

### 4. `src/notebook-widget` (new TS package `@simlin/notebook-widget`)

- Entry exports anywidget AFM `{ initialize, render }` (default export). `render` mounts React 19 into `el` (light DOM; theme tokens applied via `data-theme` on the wrapper; `data-lm-suppress-shortcuts` set on the wrapper), imports `@simlin/diagram/theme.css` and component CSS but NOT `reset.css`.
- Sizing: wrapper `position:relative; height: <height>px; width:100%`; Editor chrome anchors to it.
- Engine injection: uses `@simlin/engine` with a loader that accepts wasm bytes (`initWasm(bytes)`), running the engine on the main thread (DirectBackend) unless the spike (Phase 0) shows an inline-blob worker fits comfortably in a single-file bundle. Simulation for the widget's sparklines is small; heavy simulation runs in the kernel.
- Build: rsbuild config modelled on `config/rsbuild/rsbuild.component.config.js` (single chunk, ESM `library.type: 'module'`, CSS injected into JS, KaTeX fonts inlined as data URIs, Roboto NOT bundled -- widget uses `Roboto, system-ui` so hosts that have it use it). Output copied to `src/pysimlin/simlin/_widget/`. Size budget: `widget.js` <= 2.5 MB pre-gzip; wasm shipped separately (~2 MB after wasm-opt, gz ~1.6 MB) and delivered over the comm.

### 5. `@simlin/diagram` embeddability fixes (Editor)

- Keyboard: document keydown handler checks `composedPath()` includes the editor root and skips editable targets; each Editor instance only reacts when it (or its descendants) has focus-within or is the most recently focused editor.
- Viewport assumptions: replace `100vw/100vh` clamps with container-relative units; nothing `position: fixed` inside the Editor tree (toast viewport becomes absolute within the editor root).
- Host props: `showHomeLink?: boolean` (default true; hides `<Link to="/">` when embedded in a non-router host), an explicit `height`/fill behaviour documented in `src/diagram/CLAUDE.md` Hosting Requirements.
- These changes are covered by existing Editor tests plus new ones and must not change simlin-serve/app behaviour (they set the same defaults).

### 6. Read-only display

`Model.diagram()` returns a small `Diagram` object with `_repr_svg_` (engine SVG). `_repr_mimebundle_` on `Model` returns the widget's mimebundle merged with `image/svg+xml` so static renderers fall back to the picture. `simlin.load()`-based (in-memory) models get the same.

## Data Flow (happy paths and edge cases)

1. Human edits in widget -> Editor autosave -> `onSave(json, base)` -> traits -> kernel `observe` -> `_sync.decide(widget_snapshot)` = accept -> `_replace_from_bytes(json)` -> incremental layout is unnecessary (UI already positioned) -> serialize in file format -> `atomic_write` -> `last_written_hash = xxh/sha of bytes` -> `revision += 1` -> `on_change(widget)` -> traits pushed (`project_json` identical to what widget sent, `revision+1`) -> JS sees own snapshot, no remount.
2. Python `edit()` -> patch applied -> `diagram_sync(patch)` incremental -> serialize -> write -> hash -> `revision += 1` -> traits pushed -> JS remounts Editor at new revision (undo history reset; documented).
3. Claude Code / MCP / `git checkout` writes the file -> poll thread sees mtime change -> read bytes -> hash != last_written -> open into a temporary project and `replace_contents` in place (`_replace_from_bytes`) -> if parse fails: warn, keep last-known-good, remember bad hash so we don't re-warn every poll -> else `revision += 1`, `on_change(disk)`, traits pushed plus an "Updated on disk" notice message.
4. Stale widget snapshot (kernel advanced between the widget's base and its send) -> reject -> conflict notice message + authoritative `project_json` re-pushed at the unchanged revision -> remount. Because the Editor autosaves after every discrete edit and the kernel applies synchronously, this window is tiny outside the "cell running for a long time" case.
5. Kernel busy (long cell): widget edits queue in the frontend (ipywidgets semantics); the Editor keeps working locally; when the cell finishes the queued snapshot arrives with its base rev and is accepted if nothing else advanced. Documented behaviour; not a correctness issue because the file is only written by the kernel.
6. Widget displayed twice (two cells): both `ModelWidget` instances subscribe to the same `Project.on_change`; each display gets seeded and pushed independently; wasm compiled once per page via the global cache.
7. Non-widget contexts (nbconvert, GitHub, Colab without custom widget manager -> anywidget enables it automatically): SVG fallback in the mimebundle.

Error handling: file write errors raise from `edit()`/`save()` (autosave failure surfaces as an exception in the cell, and the in-memory change is kept with `dirty=True`); widget-originated write failures send a notice and leave `dirty=True`; parse failures on disk are warnings, never exceptions from a background thread; the poll thread never dies silently (exceptions are logged via `warnings` and the thread continues).

## Existing Patterns Followed

- `simlin-serve` `ProjectRegistry` (version counter, echo hash, remount-on-change) and its `EditorHost` client logic -- reproduced kernel-side.
- `simlin-mcp-core` `edit_model` incremental `sync_diagram` after variable ops -- reproduced in `Model.edit()`.
- `discovery::format_for_path` -- the single format table, now with `.mdl` writable.
- `src/app` `sd-component` rsbuild single-chunk config -- template for the widget bundle.
- pysimlin lock discipline (`docs/dev/python.md`); functional core / imperative shell for the sync logic and the widget JS.
- Rerun's `RERUN_NOTEBOOK_ASSET` modes for asset delivery.

## Implementation Phases

Phases are sequenced so each lands independently useful and green. Phase 0 is a feasibility spike whose result decides two open technical choices; it is not shipped as-is.

**Phase 0 -- bundle spike (gate).** In `src/notebook-widget`, build a single ESM containing `Editor` + `@simlin/engine` with the wasm supplied as bytes at runtime; load it in a static Playwright page with a fake anywidget `model` shim; measure size; try (a) DirectBackend main-thread and (b) inline-blob worker. Output: decision + the working rsbuild config. Also confirm anywidget custom-message binary buffers reach `render` in JupyterLab (a two-line widget).

**Phase 1 -- engine + pysimlin foundations.** FFI: `serialize_mdl`, `replace_contents` (all formats via composition with `open_*`). pysimlin: format table, `open()`, `Project.path/format/revision/dirty/autosave/save/save_as/reload`, `_replace_from_bytes`, incremental `diagram_sync(patch)` in `edit()`, `updateStockFlows` op parity, `_sync.py` state machine, poll-thread watcher, `on_change`, `Model.diagram()` + `_repr_svg_`, README/CLAUDE.md docs. TDD throughout; round-trip tests per format (including `.mdl` sketch preservation).

**Phase 2 -- Editor embeddability + widget bundle.** `@simlin/diagram` fixes (keyboard scoping, container units, `homeLink`, no fixed positioning) with tests; `src/notebook-widget` package (AFM entry, React mount, engine-from-bytes, page-global cache, remount-on-revision, own-snapshot detection, selection debounce, notice toast, theme); rstest unit tests with the model shim; build script copying assets into `simlin/_widget/`.

**Phase 3 -- `ModelWidget` + display hooks.** anywidget class, traits, protocol per Section 3, wasm-over-comm + `SIMLIN_WIDGET_ASSET` modes, `Model.widget()`, `_repr_mimebundle_` with SVG fallback, `Model.selection`; Python unit tests with a fake comm; manual smoke in JupyterLab.

**Phase 4 -- journey test, packaging, release.** Playwright JupyterLab e2e (own CI job); wheel packaging (`package-data`, missing-asset check), `release.yml` builds assets on host before cibuildwheel, `scripts/build_wheels.py` parity, `scripts/release-pysimlin.sh`; example notebook under `src/pysimlin/examples/`; docs (`src/pysimlin/README.md`, `CLAUDE.md`s, `docs/architecture.md`, `docs/README.md` index).

**Phase 5 -- hardening.** Colab + VS Code manual checklists executed and recorded; large-model snapshot size check (C-LEARN) against the 10 MiB frontend->kernel cap with a clear error if exceeded; nbconvert static export check; conflict UX polish; perf of the poll thread with many open models.

## Additional Considerations

- Out of scope / follow-ups (explicit): simlin-serve and simlin-mcp-core still treat `.mdl` as read-only with an `.sd.json` sidecar; aligning them with in-place `.mdl` writing is a separate change in a different subsystem (estimate: small, one dispatcher + writer branch + tests) and should be sequenced right after Phase 1 lands the FFI. Result overlays/LTM in the widget; a CDN asset mode (`@simlin/notebook-widget` on npm) to keep `_esm` out of `.ipynb` files in Colab; multi-peer merge (CRDT design doc).
- Colab specifics: each output is a sandboxed iframe -- portals are clipped to the cell; wasm over the comm has to be verified there (Phase 5); widget state is saved into the `.ipynb` by default, so `_esm` (~2 MB) + `project_json` are persisted per displayed widget -- documented, with the CDN mode as the future mitigation.
- Frontend->kernel message cap: tornado's default 10 MiB websocket max applies to the snapshot; models whose native JSON exceeds it get a clear error rather than a silent kernel-side drop.
- Security: the kernel writes only to the path it opened; no arbitrary-path handler exists (unlike 2021).
- Threading: trait sets from the poll thread are marshalled to the kernel IO loop where present.
- Editor undo history resets on kernel-originated remounts; own edits do not remount. This matches simlin-serve; a future op-level API could improve it.

## Glossary

- **anywidget / AFM** -- the Anywidget Front-end Module standard: a Python widget class whose `_esm` string is a JS module exporting `render({model, el})`; hosted natively by JupyterLab, Notebook 7, VS Code, Colab, marimo.
- **Snapshot** -- the whole project serialized as engine-native JSON (`simlin_engine::json::Project`), the unit of sync everywhere in this design.
- **Revision** -- per-process monotonic integer on `Project`, bumped on every accepted change; the widget echoes the revision it edited from so stale snapshots can be rejected.
- **Echo suppression** -- remembering the hash of bytes we wrote so the poll thread does not treat our own write as an external change.
- **Incremental layout** -- `simlin_project_diagram_sync` with the applied patch: positions new elements without moving existing ones.
- **DirectBackend** -- `@simlin/engine`'s main-thread engine backend (vs the Web Worker backend).
