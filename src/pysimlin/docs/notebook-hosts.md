# The notebook editor on each host

pysimlin's in-cell diagram editor (`simlin.open(path)` displayed in a cell,
or `model.widget()`) is an [anywidget](https://anywidget.dev), so it runs
wherever anywidget does: JupyterLab 4, Notebook 7, VS Code (local and
Remote-SSH), Google Colab, marimo. This document is the checklist for
trying it on each host and the record of what is verified and what is not.

**Read the status line of each host before relying on it.** VERIFIED means
the listed steps have been executed against the listed version and the
observations held. UNVERIFIED means nobody has done that yet: the host is
expected to work because anywidget supports it, but expectation is not
compatibility. When you run a checklist on an unverified host, record the
host version, the pysimlin version, and every deviation, and change the
status line in this file in the same PR.

Design and acceptance criteria: [docs/design-plans/2026-08-17-pysimlin-widget.md](/docs/design-plans/2026-08-17-pysimlin-widget.md) (AC2.1-AC2.8).

## What is common to every host

Install: `pip install pysimlin` (a wheel exists for Linux x86_64/aarch64
and macOS arm64, Python 3.11+; from a checkout, the wheel
`scripts/build_wheels.py` produces). No extension, no sidecar process.

The cells, run in order (this is
[`examples/notebook_editor.ipynb`](../examples/notebook_editor.ipynb) in
short; the Colab variant is
[`examples/colab_quickstart.ipynb`](../examples/colab_quickstart.ipynb)):

```python
# 1. open a file-backed model (any .stmx/.xmile/.mdl/.sd.json; build one if you have none)
import simlin
m = simlin.open("logistic-growth.stmx")

# 2. display: the editor
m

# 3. after an edit in the editor: the file changed, revision advanced, a run sees it
print(m.revision, m.dirty)
run = m.run()

# 4. an edit from Python reaches the editor
from dataclasses import replace
with m.edit() as (current, patch):
    patch.upsert(replace(current["carrying_capacity"], equation="12000"))

# 5. an external write reaches the editor (another process / Claude Code / git checkout)
# e.g. in a terminal: python -c 'import simlin; m=simlin.open("logistic-growth.stmx", watch=False); ...edit...'

# 6. selection flows back
print(m.selection)

# 7. a second display of the same model in another cell
m
```

What to observe, per acceptance criterion:

| AC    | Observe                                                                                                                                                                                                                                                                                                                                                                                                                                      |
| ----- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| AC2.1 | Cell 2 shows the Simlin editor (diagram, tool dial bottom-left, search bar top-right) at 600 px height; `m.widget(height=400, theme="dark")` honours both. Nothing outside the cell changes style (the widget's CSS is scoped to `.simlin-notebook-widget`).                                                                                                                                                                                 |
| AC2.2 | Add a variable from the tool dial, click it, give it an equation, Save. Before running cell 3 the model file on disk already contains it; cell 3 prints a higher `revision`, `dirty == False`, and `run.results` has the new column.                                                                                                                                                                                                                                  |
| AC2.3 | Cell 4: the editor shows the new equation with a short "Updated from Python" toast (its undo history resets). Step 5: within about a second, "Updated on disk" and the change is drawn; `m.revision` advanced.                                                                                                                                                                                                                                                       |
| AC2.4 | Cell 7: a second editor of the same model. An edit in either appears in the other (the other shows "Updated in another view"). The browser asked the kernel for the engine wasm once for the page (network/devtools: one `{type:'wasm'}` reply carrying ~5 MB, not two).                                                                                                                                                                      |
| AC2.5 | Make an edit in the editor while a long cell (`import time; time.sleep(20)`) is running, then during that time run nothing else; the edit is saved when the cell finishes. To provoke a stale snapshot: edit in one of two views, then quickly edit in the other before the first save's reply arrives -- the second view shows a warn toast ("Your edit was based on an older version...") and reloads; the file holds the first edit only. |
| AC2.6 | With the editor focused, Delete/Backspace remove the selected element and Ctrl/Cmd-Z undoes; typing Backspace inside the equation editor edits text and deletes nothing; none of these run the notebook's own shortcuts (in JupyterLab, `d d` while a variable is selected must not delete the cell).                                                                                                                                              |
| AC2.7 | Select a variable, run cell 6: its name (a tuple of names). Deselect, run again: `()`.                                                                                                                                                                                                                                                                                                                                                       |
| size  | Display a model whose native JSON exceeds `max_snapshot_bytes` (8 MiB; use `m.widget(max_snapshot_bytes=1024)` to test with a small model): a `RuntimeWarning` appears with the display; an editor edit shows the warn toast "Edit not saved: the model is too large for the notebook connection ..." and the file does not change; `m.edit()` still works.                                                                                        |
| theme | `theme="auto"` follows the host's light/dark setting live; `"light"`/`"dark"` force it.                                                                                                                                                                                                                                                                                                                                                       |

Things every host shares:

- **Asset delivery** is chosen once per process from `SIMLIN_WIDGET_ASSET`
  before `import simlin`: unset/`bundled` (module text in the widget state,
  wasm over the comm as a binary buffer), `inline` (wasm base64-embedded in
  the module; for hosts whose comm cannot carry binary buffers; largest
  saved state), or an `http(s)://` URL of the module (a dev server or CDN;
  wasm still over the comm).
- **Snapshot size**: an edit travels browser-to-kernel as the whole project
  in JSON. The editor refuses to send one above `max_snapshot_bytes`
  (default 8 MiB) because JupyterLab/Notebook 7's server drops websocket
  messages above tornado's 10 MiB `websocket_max_message_size` by closing
  the connection. The cap applies on every host; whether a given host has
  its own limit above or below is a fact to check per host, not to assume.
- **Static renderers**: the display's output also carries the SVG diagram.
  nbconvert shows it when the notebook has no saved widget state
  (`--ExecutePreprocessor.store_widget_state=False` when executing);
  with saved state nbconvert exports the widget itself, loading ipywidgets
  from a CDN when opened (verified by `make export-check`).
- **Poll thread**: an opened model polls its file every 0.5 s on a daemon
  thread (about 0.1% of a core for twenty open models); `watch=False`
  disables it. Change notifications are marshalled onto the kernel's IO
  loop when running under ipykernel; on hosts without ipykernel they are
  delivered directly from the poll thread (see marimo).

## JupyterLab 4 -- VERIFIED (4.6.3, headless, by the automated journey)

Status: `make -C src/pysimlin e2e` (CI job `pysimlin-e2e`) drives a real
JupyterLab 4.6.3 with Playwright and verifies: display renders the editor
(AC2.1); adding a variable and saving an equation in the editor rewrites the
file and the next `m.run()` sees it, with exactly one accepted snapshot per
edit (AC2.2); `m.selection` (AC2.7); a Python `edit()` -> "Updated from
Python" and the element drawn (AC2.3); a write from a second process ->
"Updated on disk" and `m.revision` advanced (AC2.3); no page errors, no
stderr, no error outputs. `make export-check` verifies the static exports.
NOT covered by the journey (unit-tested, not observed in a browser): two
views of one model (AC2.4), stale-snapshot rejection (AC2.5), keyboard
scoping (AC2.6), theme following, the oversize toast, the "model not found"
reopen case. Run those rows of the table by hand when touching them.

Install and run:

```bash
pip install pysimlin jupyterlab
jupyter lab
```

Host notes we know:

- Reopening a notebook without a running kernel (or after "Restart Kernel")
  shows JupyterLab's "Error displaying widget: model not found" in place of
  every widget, because JupyterLab does not save widget state into the
  `.ipynb` by default (Settings Editor > Jupyter Widgets > "Save Jupyter
  widget state in notebooks" turns it on and stores the ~1.5 MB module per
  displayed widget). Re-run the display cell.
- The 10 MiB tornado limit above applies here. Raising it:
  `jupyter lab --ServerApp.tornado_settings='{"websocket_max_message_size": 104857600}'`
  together with `m.widget(max_snapshot_bytes=...)` at or below ~80% of it.
- `theme="auto"` reads `body[data-jp-theme-light]`, so it follows the Lab
  theme switcher live.
- The widget root sets `data-lm-suppress-shortcuts` so Lab's notebook
  shortcuts stay out of the editor (AC2.6).

## Notebook 7 -- UNVERIFIED

Same server (jupyter_server + tornado), same ipywidgets/anywidget
labextensions, same JupyterLab theming attributes; expected to behave as
JupyterLab 4. Not run.

```bash
pip install pysimlin notebook
jupyter notebook
```

Run the whole table. Points to watch: the tornado limit and its override
apply unchanged; "model not found" on reopen applies unchanged.

## VS Code (local kernel) -- UNVERIFIED

```bash
pip install pysimlin ipykernel
# open a .ipynb in VS Code with the Jupyter extension, pick this interpreter as the kernel
```

Run the whole table. Points to watch:

- VS Code renders notebook outputs lazily, when they scroll into view.
  Confirm the editor appears when its cell is scrolled to; check whether
  scrolling away and back re-renders it (a re-render mounts a fresh Editor,
  so its undo history would restart) and that an edit made before scrolling
  away was saved (file changed).
- VS Code's outputs live in a webview without JupyterLab's
  `body[data-jp-theme-light]`, so `theme="auto"` falls back to
  `prefers-color-scheme`; check which VS Code theme that reflects.
- The Jupyter extension relays comm messages between the kernel and the
  webview through its own channel, not tornado's websocket; whether it has
  a message-size limit of its own is unknown -- the widget's cap still
  applies, so the oversize row should behave identically.
- Keyboard: confirm Delete/Backspace/undo do not reach VS Code's notebook
  commands (VS Code does not honour `data-lm-suppress-shortcuts`; the
  Editor's own scoping is what carries this here).

## VS Code Remote-SSH -- UNVERIFIED

Kernel and files on the remote host, editor in the local window. Same
checklist as local VS Code, plus: step 5 (external write) must be done ON
THE REMOTE (the poll thread runs there); confirm the wasm (~5 MB) arrives
over the remote channel and the editor renders within a few seconds on a
slow link, and consider `SIMLIN_WIDGET_ASSET=<url>` served from the remote
if it does not.

## Google Colab -- UNVERIFIED

Open [`examples/colab_quickstart.ipynb`](../examples/colab_quickstart.ipynb)
in Colab (`File > Open notebook > GitHub`) and run it top to bottom; then
run the table's rows 3-7 with the `m` it creates.

Host notes we know (from anywidget's Colab support and the design
investigation, not from a run):

- Colab needs its custom widget manager for third-party widgets;
  anywidget enables it itself (`google.colab.output.enable_custom_widget_manager()`
  on widget creation, plus the `custom_widget_manager` URL in the display
  metadata, which `Model._repr_mimebundle_` passes through). Nothing to do.
- Colab saves widget state into the `.ipynb` by default, so every displayed
  widget stores its `_esm` (~1.5 MB) and `project_json` in the notebook
  file. Expect the saved notebook to grow by that much per display; use
  `SIMLIN_WIDGET_ASSET=<url>` to keep the module out of the file.
- Every Colab output is a sandboxed iframe: anything the editor renders in a
  portal (menus, the toast) is clipped to the cell's output area. Check the
  tool dial's menu and the details panel stay usable at `height=600`.
- Whether Colab's comm delivers the wasm as a binary buffer is the open
  question from the design. If the editor shows "engine unavailable" or a
  60 s timeout, set `SIMLIN_WIDGET_ASSET=inline` in the first cell
  (`import os; os.environ["SIMLIN_WIDGET_ASSET"] = "inline"` BEFORE
  `import simlin`) and record which one worked.
- Colab is not JupyterLab: no `data-jp-theme-light`, so `theme="auto"`
  follows `prefers-color-scheme` inside the output iframe; check whether
  that matches Colab's own dark mode.
- Colab's kernel runs the notebook's Python; `Path.cwd()` is `/content`
  (local disk, fine for polling). Models on a mounted Drive are polled too
  (that is why the watcher is stdlib polling, not inotify), but Drive's
  FUSE latency may make step 5 slower than a second.
- Whether Colab has a websocket message-size limit of its own is unknown.

## marimo -- UNVERIFIED

marimo runs cells as reactive Python and hosts anywidgets natively.

```bash
pip install pysimlin marimo
marimo edit
```

```python
import marimo as mo
import simlin
m = simlin.open("logistic-growth.stmx")
w = mo.ui.anywidget(m.widget())
w
```

Points to watch:

- marimo wraps anywidgets with `mo.ui.anywidget(...)`; whether a bare `m`
  (the ipywidgets mimebundle) also renders is unknown -- try both.
- marimo is not ipykernel: `Model`'s change notifications are delivered to
  the widget directly from the poll thread (there is no `shell.kernel.io_loop`
  to marshal onto). Traits are then written from a non-main thread; watch
  for missed or duplicated "Updated on disk" pushes and report them.
- Reactivity: marimo re-runs dependent cells when `w` changes; the widget's
  `selection` trait changes on every selection, which may re-run cells that
  read `w`. Prefer reading `m.selection` in a cell that is not downstream of
  the display.
- marimo serves the frontend from its own server (starlette/uvicorn), not
  tornado; its message-size limit, if any, is unknown.
