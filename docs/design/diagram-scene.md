# Diagram scene contract (version 1)

The scene is a resolution-independent display list of one model's stock-and-flow view. The engine produces it; native renderers draw it. It exists so that exactly one implementation decides what a diagram looks like: the geometry behind every number here is computed by the same functions `simlin_engine::diagram::render_svg` uses, and that SVG output is byte-identical to the TypeScript editor's static renderer. A renderer that draws this scene faithfully therefore matches the web editor without re-deriving any geometry.

This document is normative for both producers and consumers.

## Producing a scene

- Engine: `simlin_engine::diagram::build_scene(project: &datamodel::Project, model_name: &str) -> Result<Scene, String>`, beside `render_svg`.
- libsimlin: `simlin_project_render_scene(project, model_name, out_buffer, out_len, out_error)` returns the scene as UTF-8 JSON in a buffer the caller frees with `simlin_free`. Like `simlin_project_render_svg`, a model with no stock-and-flow view is laid out automatically (transiently, never persisted) before the scene is built.

```c
void simlin_project_render_scene(SimlinProject *project,
                                 const char *model_name,
                                 uint8_t **out_buffer,
                                 uintptr_t *out_len,
                                 SimlinError **out_error);
```

## Coordinates

All coordinates are canvas (model) units, the same units the datamodel's view elements use: origin top-left, x right, y down. One canvas unit is one point at zoom 1.

## Top level

```json
{
  "version": 1,
  "modelName": "main",
  "contentBounds": { "left": 31.0, "top": 12.5, "right": 612.0, "bottom": 480.0 },
  "elements": [ ... ]
}
```

- `version`: incremented on any change a version-1 consumer would misread. Consumers ignore unknown fields.
- `contentBounds`: the union of the element bounds `render_svg` folds into its viewBox (every drawn element but connectors, labels included), before the SVG renderer's 10-unit padding. `null` for a view with nothing to draw. Use it for "fit to content".
- `elements`: in draw order. The order is the SVG renderer's: ascending layer, and within a layer the order elements appear in the view.

## Elements

```json
{
  "uid": 7,
  "kind": "stock",
  "layer": 4,
  "ident": "population",
  "isArrayed": false,
  "bounds": { "left": 177.5, "top": 82.5, "right": 222.5, "bottom": 131.0 },
  "shapes": [ ... ],
  "sparkline": { "x": 178.5, "y": 83.5, "width": 43.0, "height": 33.0 },
  "label": { ... }
}
```

| `kind`   | `layer` | `ident`                                  | `sparkline` slot | `label`      |
|----------|---------|------------------------------------------|------------------|--------------|
| `group`  | 0       | `null`                                   | never            | `groupLabel` |
| `link`   | 2       | `null`                                   | never            | never        |
| `flow`   | 3       | canonical name                           | yes              | yes          |
| `stock`  | 4       | canonical name                           | yes              | yes          |
| `cloud`  | 4       | `null`                                   | never            | never        |
| `module` | 4       | canonical name                           | never            | yes          |
| `aux`    | 5       | canonical name                           | yes              | yes          |
| `alias`  | 5       | canonical name of the aliased variable, `null` when the target is missing | yes when `ident` is non-null | yes |

- `ident` is the canonical identifier (`simlin_engine::common::canonicalize`) of the variable whose simulation series the element displays. An alias displays its target's series, exactly as the web canvas does.
- `isArrayed` is true when the variable has an apply-to-all or arrayed equation; the shapes already include the stacked copies. An alias is never arrayed and draws one circle whatever its target's equation, as the web canvas's `Alias.tsx` draws it.
- `bounds` is a conservative visual box (shapes including stroke width, arrowheads, and the label's estimated box from the same `label_bounds` the SVG renderer uses). It is for culling and hit testing, not for fitting; use `contentBounds` to fit. It does not cover the label halo's spread, so a consumer culling by `bounds` pads its query rect by at least 8 units (the halo's 4-unit dilation plus its blur).
- An element the SVG renderer skips (a flow or link whose endpoints are missing from the view, a flow with fewer than two points) does not appear.
- Every element carries at least one shape. An element the SVG draws nothing for does not appear: a link with no drawable geometry (a multi-point link, or an arc whose circle cannot be constructed), and any element whose geometry is not finite (a view element holding a NaN coordinate, which the SVG prints as `NaN`).

### Draw order within an element

Draw `shapes` in array order, then the sparkline (when a slot exists and series data is available), then the label. This reproduces the SVG group structure: a flow's outer pipe, arrowhead, inner pipe and valve precede its sparkline and label; a stock's rectangles precede its sparkline and label. Because labels belong to their element, a later element can cover an earlier element's label, as in the web editor.

## Shapes

Every shape carries a `paint`, a semantic style key resolved by the renderer's theme (below).

```json
{ "type": "rect",   "x": 177.5, "y": 82.5, "width": 45.0, "height": 35.0, "cornerRadius": 0.0, "paint": "stock" }
{ "type": "circle", "cx": 100.0, "cy": 200.0, "r": 9.0, "paint": "aux" }
{ "type": "path",   "d": [0, 100.0, 100.0, 1, 190.0, 100.0], "paint": "flowPipeOuter" }
```

- `rect` is axis-aligned; `cornerRadius` applies to both axes (SVG `rx` = `ry`).
- `path.d` is a flat number array of opcodes followed by their operands:
  - `0` move to `x, y`
  - `1` line to `x, y`
  - `2` cubic Bézier to `c1x, c1y, c2x, c2y, x, y`
  - `3` close subpath (no operands)
- Every SVG elliptical arc (connector arcs, arrowhead backs) is converted to cubic Béziers of at most 30 degrees each (so every cubic stays within 1e-6 of its circle's radius), and every SVG `transform` (the cloud's matrix, arrowhead rotation) is already applied, so a consumer never implements arcs or transforms.
- SVG hit-area helpers (`simlin-connector-bg`, `simlin-arrowhead-bg`) are invisible and are not emitted.

## Paints

The values are `src/diagram/drawing/*.module.css` over the tokens in `src/diagram/theme.css`. Stroke widths are canvas units; they scale with zoom.

| paint             | fill                         | stroke                  | width | notes |
|-------------------|------------------------------|-------------------------|-------|-------|
| `stock`           | `white`                      | `black`                 | 1     | |
| `aux`             | `white`                      | `black`                 | 1     | |
| `module`          | `white`                      | `black`                 | 1     | |
| `alias`           | `white`                      | `black`                 | 1     | dash `[2, 2]` |
| `valve`           | `white`                      | `black`                 | 1     | the flow's valve circle |
| `flowPipeOuter`   | none                         | `black`                 | 4     | |
| `flowPipeInner`   | none                         | `white`                 | 2     | |
| `arrowheadFlow`   | `white`                      | `black`                 | 1     | round join |
| `cloud`           | `white`                      | `cloudStroke`           | 2     | round join, miter limit 4 |
| `connector`       | none                         | `connector`             | 0.5   | |
| `connectorDashed` | none                         | `connector`             | 0.5   | dash `[2, 2]`; a link whose target is a stock |
| `arrowheadLink`   | `connector`                  | `connector`             | 1     | round join |
| `group`           | `fieldBackground` at 50% alpha | `textLight`           | 1     | |

Theme tokens:

| token             | light                 | dark                        |
|-------------------|-----------------------|-----------------------------|
| `black`           | `#000000`             | `#bbbbbb`                   |
| `white`           | `#ffffff`             | `#222222`                   |
| `cloudStroke`     | `#6388dc`             | `#2d498a`                   |
| `connector`       | `#808080`             | `#777777`                   |
| `fieldBackground` | `#f5f5f5`             | `#2a2a2a`                   |
| `textLight`       | `rgba(0,0,0,0.4)`     | `rgba(255,255,255,0.4)`     |
| `textMuted`       | `rgba(0,0,0,0.6)`     | `rgba(255,255,255,0.6)`     |
| `canvasBackground`| `#f2f2f2`             | `#121212`                   |
| `selected`        | `#4444dd`             | `#4444dd`                   |
| `sparkline`       | `#2299dd`             | `#2299dd`                   |

## Labels

```json
{
  "paint": "label",
  "anchor": "middle",
  "baseline": "alphabetic",
  "fontSize": 12.0,
  "fontWeight": 300,
  "halo": true,
  "lines": [
    { "text": "birth", "x": 200.0, "y": 131.0 },
    { "text": "rate",  "x": 200.0, "y": 145.0 }
  ]
}
```

- `lines` are display text: the stored `\n` escape already split into lines and `_` shown as a space (`display_name`).
- Each line's `x, y` is its text anchor point. `anchor` is SVG `text-anchor` (`start`, `middle`, `end`) around `x`. With `baseline: "alphabetic"`, `y` is the alphabetic baseline; with `baseline: "hanging"` (group labels), `y` is the top of the line box, so the line's alphabetic baseline is `y` plus the font's ascent at `fontSize`.
- The SVG's `dy` arithmetic (`1em` first lines, 14-unit line spacing, the reversed stacking of top labels) is resolved into per-line positions, so consumers never re-implement label layout.
- `paint` is `label` (fill `black`, Roboto Light 300, 12) or `groupLabel` (fill `textMuted`, weight 500, 12). A group label is always one line: the SVG prints a group name as a single text run under `white-space: nowrap`, so a stored line break shows as a space.
- `halo: true` asks for the background halo the SVG `labelBackground` filter draws: the glyphs dilated by 4 units, blurred with standard deviation 2, filled with the `white` token at 85% alpha, under the text. Renderers may approximate it (a wide round-joined `white` stroke under the fill is a good approximation).

## Sparklines

A `sparkline` slot is the rectangle `{x, y, width, height}` the web canvas's `Sparkline` component draws into. Given the saved simulation times `t` and values `v` for the element's `ident`:

- `xMin = t[0]`, `xSpan = t[last] - t[0]`
- `yMin = min(0, min(v))`, `yMax = max(v)`, `ySpan = (yMax - yMin)`, or `1` when that is zero
- a point is `(x + width * (t[i] - xMin) / xSpan, y + height - height * (v[i] - yMin) / ySpan)`; NaN values are skipped
- the line strokes `sparkline` at width 0.5 with round caps; the zero axis is a horizontal line at `v = 0` stroking `textLight` at width 0.75 with round caps, drawn before the line
- an arrayed variable displays one line per element series (`name[element]` in the simulation results, ordered by name), all sharing one value range computed across every series, stroked in order with the ColorBrewer Dark2 palette the web editor uses (`#1b9e77`, `#d95f02`, `#7570b3`, `#e7298a`, `#66a61e`, `#e6ab02`, `#a6761d`, `#666666`, repeating); a single series uses `sparkline`
- non-finite values are left out of the range and the line; fewer than two saved times, a zero time span, or no finite value at all draws no sparkline (where the web component's arithmetic produces NaN coordinates and so draws nothing)

## Stability rules for producers

- Never emit geometry the SVG renderer does not draw, and never compute a number a second way: add a shared geometry function and have `render_svg` and `build_scene` both read it.
- Changing what the SVG draws changes this scene in the same commit.
