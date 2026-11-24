# simlin-diagram

A model's stock-and-flow diagram drawn: as SVG, as PNG and as the scene display list a host draws, with the exact geometry of each element.
It uses [simlin-base](/src/simlin-base/CLAUDE.md) alone.
The root `CLAUDE.md` holds the development rules; this file is a map of where functionality lives (paths are relative to this crate's `src/`) and of the standing invariants code in this crate keeps.

- The crate renders a model's first stock-and-flow view as SVG (`render_svg`, byte-identical to the TypeScript static renderer; `src/diagram/tests/svg-rendering.test.ts` is that parity test), as PNG (`render_png`, behind `png_render`), and as a scene display list (`build_scene`, whose contract is [the diagram scene](/docs/design/diagram-scene.md)), and exposes the exact geometry the layout metric scores.
  Every drawing decision has one owner, and both serializers read it:
  - `resolve::resolve_view` decides which elements are drawn (a link or flow whose endpoints are missing from the view is not), their layer and draw order, whether each is arrayed, an alias's target, and the fit-to-content bounds.
  - Each element's geometry is one function: `aux_geometry`, `stock_geometry`, `module_geometry`, `alias_geometry`, `group_geometry`, `cloud_transform` (over the shared `CLOUD_PATH`), `flow_geometry`, `connector_geometry`, `arrowhead_geometry`, and `label::label_lines` for per-line label anchors.
    `render_*` prints from it and `scene.rs` reads it.
    Never compute a drawn number a second time in either serializer: a hand-maintained twin drifts exactly where the geometry is non-trivial; extend the geometry function instead.
  - The scene changes representation only: `path::PathBuilder::svg_arc_to` converts every SVG arc to cubic Béziers (SVG 1.1 F.6.5 and F.6.6, at most 30 degrees per cubic, so a cubic stays within 1e-6 of its radius), and transforms are applied to the points.
    `scene_tests.rs` checks each scene number against the number the SVG prints for the same project.

Features: `png_render` (the resvg rasterizer, which the browser bundle leaves out) and `test-support` (the test helpers other crates' tests use, for `[dev-dependencies]` edges alone).
A test that opens a model file is in simlin-tests (`src/simlin-tests/tests/integration/diagram.rs`), since the readers are built on this crate.
