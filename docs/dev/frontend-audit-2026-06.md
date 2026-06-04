# Frontend audit: src/app and src/diagram (June 2026)

A multi-agent audit of the frontend TypeScript/React/CSS code in `src/app` and
`src/diagram`, focused on real bugs (not function-component conversion). Eight
area/dimension reviewers swept the code; every finding was then adversarially
verified by an independent agent before being accepted. 51 raw findings were
reduced to the confirmed list below (10 were refuted in verification).

Status legend: **fixed** (in the PR introducing this doc), **deferred**
(tracked but intentionally not fixed here).

## High severity

| # | Finding | Location | Status |
|---|---------|----------|--------|
| H1 | Undo/redo does not discard the redo branch when a new edit follows an undo: `updateProject` prepends to the full history instead of `history.slice(projectOffset)`, so undo after edit-after-undo restores sibling states from the abandoned branch | `Editor.tsx` `updateProject` | fixed |
| H2 | Pan/zoom/momentum/panel-resize flood the 5-entry undo history: `queueViewUpdate` calls `updateProject`, which always prepends a snapshot; viewBox/zoom are serialized into the protobuf so every momentum frame is a distinct snapshot, evicting real edits | `Editor.tsx` `queueViewUpdate` | fixed |
| H3 | `flowStillBeingCreated` is set on flow creation and never reset (the cancel branch re-sets it to `true`, a typo for `false`); a later Escape-cancel of an unrelated rename calls `onDeleteSelection()` and deletes that variable | `drawing/Canvas.tsx` `handleEditingNameDone` | fixed |
| H4 | Details panels keyed by `projectVersion`/`projectOffset` remount on every view bump (+0.001 per pan frame, +0.01 per engine round-trip); a mid-typing autosave completion or undo/redo discards in-progress Slate edits and refires the async LaTeX load | `Editor.tsx` `getDetails` | fixed |
| H5 | Long KaTeX equations overflow the details card: KaTeX `.base` spans are atomic inline-blocks with `white-space: nowrap`, which `overflow-wrap: anywhere` on the ancestor cannot break; `.eqnPreview` is a flex container with `min-width: auto` and no `overflow-x` | `VariableDetails.module.css` | fixed |
| H6 | `reset.css` (global `box-sizing: border-box`) is absent from the app's compiled CSS bundle, so `.searchBar` (content-box width + `padding: 0 8px`) renders 16px wider than the details `.card`, misaligning their left edges | `Editor.module.css` + bundling | fixed (defensive `box-sizing`); bundling gap tracked |
| H7 | `HostedWebEditor` initial load rejection is swallowed (network error, non-JSON body, missing `pb`/`version`), leaving a permanently blank editor with no message | `HostedWebEditor.tsx` | fixed |
| H8 | The account menu's Logout item only closes the menu; no signOut/navigation/fetch is ever invoked, so users cannot sign out. (Server-side `DELETE /session` is itself a stub.) | `app/Home.tsx` | fixed (client side); server stub tracked |

## Medium severity

| # | Finding | Location | Status |
|---|---------|----------|--------|
| M1 | Equation preview feeds raw equation text to `katex.renderToString` during the async LaTeX load window and when the engine can't produce LaTeX, mangling `_` into subscripts etc. | `VariableDetails.tsx` | fixed |
| M2 | A variable with only a non-fatal unit warning is forced out of the LaTeX preview into the raw editor (`showPreview` gates on `unitErrors` presence, contradicting `variableDetailsView` semantics) | `VariableDetails.tsx` | fixed |
| M3 | Error highlighting only applies to line 0 and treats engine byte offsets as UTF-16 indices; multi-line equations (including the `{apply-to-all:}\n` prefix) and non-ASCII content mis-highlight. Caret-from-preview-click has the same byte/UTF-16 conflation. Stray `console.log(err)` ships to production | `VariableDetails.tsx`, `equation-caret.ts` | fixed |
| M4 | `Dialog` exposes `disableEscapeKeyDown` but backdrop clicks always dismiss; in `NewUser`, a backdrop click with a non-empty username submits the PATCH while bypassing the terms-agreement guard on the Submit button | `components/Dialog.tsx`, `app/NewUser.tsx` | fixed |
| M5 | `Autocomplete` portaled listbox position computed once at open; goes stale when the scrollable details panel scrolls or the window resizes | `components/Autocomplete.tsx` | fixed |
| M6 | `Login`: `fetchSignInMethodsForEmail` and `sendPasswordResetEmail` lack the try/catch that all sibling auth handlers have; rejections are unhandled and the UI gives no feedback | `app/Login.tsx` | fixed |
| M7 | Multi-line label `<tspan>`s keyed by line text produce duplicate React keys for repeated lines (severity lowered to low in verification; flat stateless list) | `drawing/Label.tsx` | fixed |

## Low severity

| # | Finding | Location | Status |
|---|---------|----------|--------|
| L1 | `LookupEditor`: datapoint count of 1 divides by `size - 1 == 0`, producing NaN x and y that can be saved into the table | `LookupEditor.tsx` | fixed |
| L2 | Login error states never clear while the user edits the field | `app/Login.tsx` | fixed |
| L3 | `Home.getProjects`: fired from the constructor, no error catch, no unmount guard (StrictMode double-construction is the reliable trigger) | `app/Home.tsx` | fixed |
| L4 | `ErrorDetails` list items rendered without React keys | `ErrorDetails.tsx` | fixed |
| L5 | Editor passes freshly-allocated no-op handlers to PureComponent Canvas in readOnly/embedded mode, defeating its memoization | `Editor.tsx` | fixed |
| L6 | Untracked `setTimeout`s in module-navigation handlers can run against an unmounted Editor (every other deferred callback is tracked/guarded) | `Editor.tsx` | fixed |
| L7 | Flow cloud-end retraction applies x and y shifts independently, double-shifting diagonal final segments by sqrt(2)*CloudRadius | `drawing/Flow.tsx` | fixed |
| L8 | Flow arrowhead angle defaults to 0 (pointing right) when the final segment is zero-length (`atan2(0,0)`) | `drawing/Flow.tsx` | fixed |
| L9 | Connector aux arc-intersection uses `tan(r/circ.r)`, the same asymptote hazard the stock branch was already fixed to avoid (`atan`) | `drawing/Connector.tsx` | fixed |
| L10 | `centerVariable` hardcodes the small panel width and `handleZoomChange` the large one, ignoring the 420px medium breakpoint | `Editor.tsx` | fixed |
| L11 | `Checkbox` onChange type drops Radix `'indeterminate'`; normalize to boolean | `components/Checkbox.tsx` | fixed |
| L12 | `TextField` spreads downshift inputProps after `id`, breaking label association for a labeled Autocomplete | `components/TextField.tsx` | fixed |
| L13 | `Menu` anchors Radix to a `position: fixed` proxy span at a memoized rect; the span goes stale if the true anchor moves while open | `components/Menu.tsx` | deferred |
| L14 | Component library CSS modules hardcode light colors with no dark-mode overrides (latent: editor chrome is not dark-themed today) | `components/*.module.css` | deferred |
| L15 | Autocomplete dropdown options don't clip/ellipsize long variable names | `components/Autocomplete.module.css` | fixed |

## Deferred / tracked separately

- **reset.css missing from the app CSS bundle**: `src/diagram/index.ts` has a
  side-effect `import './reset.css'`, but the universal `box-sizing` rule does
  not survive into `src/app/build/static/css/*`. The panels now declare
  `box-sizing: border-box` explicitly (defense in depth), but the bundling gap
  means *all* of reset.css's body/typography defaults are absent app-wide.
- **Server `DELETE /session` is a stub**: client-side logout (Firebase
  `signOut`) is wired up, but the session cookie is not invalidated
  server-side.
- **Engine round-trip per view-change event**: every wheel tick, momentum
  animation frame, and pinch update runs `applyPatch` + `serializeProtobuf` +
  full project JSON re-parse through the WASM engine. Excluding these from
  undo history (H2) removes the worst symptom; debouncing the persistence is
  follow-up work.
- **Dark-mode coverage for the component library** (L14): needs a pass over
  ~10 CSS modules plus a decision about theming editor chrome.
- **`Menu` anchor-tracking** (L13): consumer today is short-lived; fix
  alongside a positioning-library adoption rather than ad hoc.

## Verified-and-rejected findings (for the record)

Ten findings were refuted in adversarial verification, including: `loadSim`'s
non-functional setState appends (benign in practice), `StaticDiagram` WASM
handle leak (unreachable path), `HostedWebEditor` not reloading on prop change
(hosts always remount via key/full navigation), and the snapshot-card object
URL leak (the Snapshotter UI is commented out; dead code).
