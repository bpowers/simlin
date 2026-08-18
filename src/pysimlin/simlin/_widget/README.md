# simlin/_widget

Package data for the notebook editor widget (`simlin.widget.ModelWidget`).
Three files land here and ship inside every pysimlin wheel; none is committed
to git (`.gitignore`):

- `widget.js` -- the anywidget front-end module: one self-contained ES module
  (React, `@simlin/diagram`'s Editor, `@simlin/engine`, CSS and fonts inlined),
  built by `src/notebook-widget` (`dist/widget.js`). Its text is the widget's
  `_esm`.
- `libsimlin-browser.wasm` -- the engine from `src/engine/core/`, `wasm-opt`'d
  in release builds. Delivered to the browser as a binary comm buffer when the
  module asks (`{type:'wasm'}`), or base64-embedded into `_esm` when
  `SIMLIN_WIDGET_ASSET=inline`.
- `ASSETS.json` -- the staging manifest: source commit, wasm-opt mode, and each
  asset's size and sha256. `setup.py` refuses to build a wheel or sdist unless
  the two assets are present, non-empty and match it; the release workflow's
  wheel check re-verifies it inside every wheel.

`scripts/stage_widget_assets.py` is the ONE thing that writes this directory
(`make assets` from `src/pysimlin`, or the notebook-widget package's
`pnpm build`, which runs it after `rsbuild build`; `--no-build` restages from
the existing build outputs, `--check` verifies). Do not copy files here by hand
or add a second copy step. The release workflow (`.github/workflows/release.yml`)
runs the same script once on the host and every platform wheel ships that one
set of assets byte for byte.

`simlin/widget.py` resolves the two assets from this directory (via
`importlib.resources`) once at import; a missing file does not break
`import simlin` -- creating or displaying a widget raises `SimlinAssetError`
naming the file and how to produce it.

`pyproject.toml` (`[tool.setuptools.package-data]`) and `MANIFEST.in` include
this directory's `*.js`, `*.wasm`, `ASSETS.json` and this README.
