# simlin/_widget

Package data for the notebook editor widget (`simlin.widget.ModelWidget`).
Two build outputs of `src/notebook-widget` land here and ship inside every
pysimlin wheel; neither is committed to git:

- `widget.js` -- the anywidget front-end module: one self-contained ES module
  (React, `@simlin/diagram`'s Editor, `@simlin/engine`, CSS and fonts inlined).
  Its text is the widget's `_esm`.
- `libsimlin-browser.wasm` -- the engine, `wasm-opt`'d. Delivered to the
  browser as a binary comm buffer when the module asks (`{type:'wasm'}`), or
  base64-embedded into `_esm` when `SIMLIN_WIDGET_ASSET=inline`.

`simlin/widget.py` resolves both from this directory (via
`importlib.resources`) once at import; a missing file does not break
`import simlin` -- creating or displaying a widget raises `SimlinAssetError`
naming the file and how to produce it. To populate this directory from a
source checkout run the widget package's build (`pnpm --filter
@simlin/notebook-widget build`), which copies both files here; the release
workflow does the same once on the host before building wheels
(`scripts/release-pysimlin.sh`).

`pyproject.toml` (`[tool.setuptools.package-data]`) and `MANIFEST.in` include
this directory's `*.js`, `*.wasm`, and this README.
