# Diagram editing core in Rust

## Why

The editing core of `docs/design-plans/2026-09-10-diagram-editing-core.md` -- one owner for flow geometry, one planner whose frame at pointer-up is the commit, and model ops derived from the view an edit produces -- lives in TypeScript under `src/diagram`. Two consumers need the same core and cannot use TypeScript:

- **Native hosts.** A native host draws the engine's scene display list (`docs/design/diagram-scene.md`). A drag that re-routes a flow must be drawn with the engine's pipe, arrowhead and valve geometry every frame, so the frame has to come from the engine anyway.
- **Tool edits.** AI tools (`simlin-mcp-core`, a host's own assistant tools) edit through libsimlin, and created elements are placed by the engine's incremental layout. Touch edits routed by a second implementation would draw different pipes for the same change.

The invariants and their constants already have a Rust owner for imported and laid-out views (`diagram::flow_geometry`). The interactive half -- routing, offsetting, healing, hit testing, planning taps and drags, and deriving model ops -- joins it in the engine.

## What moves, and what stays with the host

Moves into `simlin_engine::editing`:

- Flow geometry: face attachment and terminals, the G2-G6 classification, the valve policy, `route`/`route_end`, `offset_segment`, `heal`, `slide_valve`.
- Hit testing: which element, and which part of it, a point lands on.
- Planning: what a tap means, what a press on an element starts dragging, every gesture kind's frame, links following their moved endpoints, default names, and the scene elements a frame draws differently.
- Delete planning and rename planning.
- The model ops a view edit implies, as a patch op (`EditView`) evaluated when the patch applies.

Stays with each host:

- **The edit queue.** Committed state, pending edits and rollback depend on the host's concurrency model (a Web Worker engine behind promises, or libsimlin called from a native host's own threads), so each host owns how an edit lands.
- **Input policy.** Which recognizer a touch goes to, the screen-space slop (divided by the zoom before it reaches the engine), double taps, lost releases, the name editor, and refusing presses while an edit lands.
- **Rendering.** The engine returns scene elements for what a frame draws differently; the host decides how to show them.

The web editor keeps its TypeScript core until it chooses to plan through WASM (it runs the engine in a Web Worker, so per-frame planning would need a main-thread instance).

## Deliberate differences from the TypeScript planner

The engine's planner is tuned for touch. Where the TypeScript planner's behavior follows from pointer-and-keyboard input, the engine's does not follow it, and no test holds the two together: the tests pin the intended behavior.

1. **Taps and drags are separate entry points.** A host's recognizers already tell a tap from a drag, so a drag carries no click threshold. A press on a pipe latches slide or offset on its first movement and keeps it.
2. **Creating an element commits at once**, under an allocated default name, and hands off to the name editor; typing a name then renames it. On a touch device the element should exist the moment it appears.
3. **A finger on the empty canvas starts no drag**, so the host pans, as every touch canvas does. A pencil or a pointer rubber-bands a selection there.
4. **A link a finger draws or reattaches is straight**: a finger has no stable point to curve a link through. A pencil or a pointer curves it through where it lets go, and dragging a link's arc snaps it straight within `STRAIGHT_LINE_MAX` of the direct bearing.
5. **Drop targets reach beyond their bodies** by the press's `target_slop`, the host's slop divided by the zoom.
6. **Hit testing decides in tiers**, so an end stays grabbable where it touches what it attaches to: a body firmly holding the point (at least `FIRM_INSET` in from its edge) lands on that element unless something drawn above it takes the point first; otherwise an end handle within reach (a flow's source or arrowhead, a link's arrowhead) wins; otherwise a label firmly holding the point; otherwise the nearest drawing within the tolerance. A handle outranks a label because an end is small and bound to an edge while a label is large.
7. **Model ops are derived at apply time.** `EditView` diffs the edited view against the model's current view inside the patch, so a payload is never built from state read before an earlier edit landed.

Stocks and aliases are not link targets, as in the web editor.

## Engine API

`simlin_engine::editing` exposes what hosts call; the geometry core is crate-private and reached through the planner.

- Units and invariants: `CORNER_CLEARANCE`, `MIN_SEGMENT`, `MIN_SINK_SEGMENT`, `VALVE_CLAMP_MARGIN`, `PIPE_SPACING`, `GEOMETRY_EPSILON`.
- `hit_test(project, model_name, point, tolerance) -> Result<Option<Hit>, String>`: the element and `HitPart` (`Body`, `Arrowhead`, `Source`, `Label`), judged from the geometry and draw order the scene is built from.
- `BaseView::new(model, view)`: the index a plan reads -- elements by uid, stock attachments, the links touching each element, variable kinds, arrayedness, used names -- built once per tap or drag.
- `Press { point, hit, tool, selection, toggle, pointer, target_slop }`: a press, resolved by the host.
- `plan_tap(&BaseView, &Press) -> Plan`: a creation tool creates at the tap and hands off; a tap on an element selects it (a toggle adds or removes it) and opens its details when it lands on the body or label; a flow's end or a cloud selects the flow; the empty canvas clears the selection.
- `begin_drag(BaseView, Press) -> Option<GestureSession>`: classifies the press into a `GestureKind` or starts nothing. `GestureSession::frame(pointer) -> Plan` plans one frame against the captured base view. A `Plan` carries what landing it does (`CommitKind::None`, `Select`, `Edit`), the selection, the drop target and its validity, the handoff, whether it opens details, the undo label, and for an invalid link drop the dangling link to draw; `Plan::edit()` is the `ViewEdit` it commits; a plan whose changes equal the base plans nothing, so an `Edit` always carries one. The release is the frame at the release point: exactly what the preview showed there.
- `plan_move(&BaseView, &[i32], Point) -> Plan`: the move-selection frame at an offset, which `GestureSession::frame` plans for a move-selection drag and a host plans without a session to nudge the selection from the keyboard.
- `preview(&BaseView, &Plan) -> Preview { hidden, elements }`: the base elements the frame does not draw as they were (what it changes or removes, and the links touching them) and the scene elements it draws in their place, in draw order.
- `plan_delete(&StockFlow, &[i32]) -> ViewEdit` and `plan_rename(&StockFlow, old, new) -> ViewEdit`.
- `ModelOperation::EditView { index, upsert, remove }` and `patch::is_view_only_patch`.

## The `EditView` patch op

`EditView` upserts elements into view `index` (replacing the element of the same uid, or appending a new one) and removes elements by uid, then makes the model agree with the edited view. Against the model and view as they are when the op applies:

- a named element whose uid survives under a different name renames its variable (`from` its current ident, `to` the new name as typed); a rename onto an existing variable fails the patch;
- a removed named element deletes its variable when no remaining element names it;
- a named element with a new uid creates its variable with an empty equation; a variable of that name already existing fails the patch, since an upsert would silently replace it whatever its kind;
- a flow end whose attached stock changed moves the flow between the stocks' inflow or outflow lists, touching only flows and stocks that exist after the edit and never duplicating an entry.

Op order inside the op is renames, deletes, creates, stock list updates, then the view. libsimlin applies a patch to a staged clone, so the model and the view land together or not at all. `is_view_only_patch` derives the ops against the project and calls an `EditView` view-only when it implies none, so a geometry-only commit skips recompilation.

## libsimlin surface

The one mutation stays `simlin_project_apply_patch`, which reads the `editView` op; every planning entry point returns a patch the host applies.

- `simlin_model_hit_test(model, x, y, tolerance, out_hit, out_uid, out_part, out_error)`.
- `SimlinPress { x, y, has_hit, hit_uid, hit_part, tool, selection, selection_len, toggle, pointer, target_slop }`.
- `simlin_model_plan_tap(model, press, out_buf, out_len, out_error)`: `{kind, commit, selection, handoff, details, label, patch}`, `patch` null unless the tap edits.
- `simlin_gesture_begin(model, press, out_error) -> *mut SimlinGesture`: NULL with no error when the press starts no drag.
- `simlin_gesture_frame(gesture, x, y, out_buf, out_len, out_error)`: `{kind, commit, selection, target, handoff, details, label, hidden, elements}` in a buffer the gesture owns and reuses, valid until its next call, so a drag allocates no output buffer per frame.
- `simlin_gesture_commit(gesture, x, y, out_buf, out_len, out_error)`: the tap's shape for the frame at the release point, `commit` `none` and `patch` null when landing changes nothing.
- `simlin_gesture_ref` and `simlin_gesture_unref`.
- `simlin_model_plan_move(model, uids, count, dx, dy, out_buf, out_len, out_error)`: the tap's shape, with `kind` `moveSelection`, for the selection moved by `(dx, dy)` model units: what a keyboard nudge lands.
- `simlin_model_plan_delete(model, uids, count, out_buf, out_len, out_error)` and `simlin_model_plan_rename(model, old_name, new_name, out_buf, out_len, out_error)`: patch JSON. A variable the diagram does not draw is renamed with a direct `renameVariable`.

Every entry point refuses a model with no stock-and-flow view with `DoesNotExist`: the scene draws such a model through a transient layout, which no edit could change. A session holds its own base view, so it stays valid whatever the project does meanwhile; a host applies an edit only once its gesture has ended.

## Driving the planner from a host

- **Which touches plan.** A host asks `simlin_gesture_begin` at the moment its pan would begin, with the point the touch went down at, and lets the canvas pan when no drag starts. A press that grabs nothing then costs panning no delay.
- **Drawing a frame.** A host hides the base scene elements a frame lists in `hidden` and draws its `elements` among the scene at their layers, complete, whatever the camera is doing: a frame is a handful of elements replaced at display rate, and a label that appeared a frame late would flicker on every frame. The layers are load-bearing: a link's arc runs between the centers of the elements it connects, and only those elements, drawn above it, hide its ends, so a frame drawn on top of the scene shows links crossing what they connect.
- **Landing an edit.** A host applies the release's patch off its UI thread, with errors allowed (a new variable has no equation yet), rebuilds the scene, and shows it in place of the last frame without moving the camera; then it adopts the plan's selection, opens the name editor on a handoff, and simulates again. Presses start nothing while an edit lands, since they would plan against a view the project has left.

## Testing

- **One oracle.** `editing::invariants` (test-only) states G1-G8 as a checker with every arm enumerated (`FlowArm::ALL`); tests assert arms, not message text. `test_support::agreement_report` states the model/view agreement a committed edit holds.
- **Tables for decisions.** Rows come from the enum: each `HitPart` has a point landing on it and each tier of the ranking a row; each `GestureKind` has a press derived through `hit_test` whose frame commits what it previews, whose drag back to the press commits nothing (E1), and whose refused drops commit nothing and mark their target (E6); every arm of `begin_drag` that starts nothing has a row; each kind of view element has a nudge, and a nudge of a selection some drag moves plans exactly that drag's frame; each derived model op has a test through the production patch path.
- **Generated scenes.** `scene_gen` builds strict and imported-shape scenes with its own geometry (mulberry32 seeds), never the checker's or the core's. `scene_sweep_tests` drags every gesture over the seeds, requiring the strict invariants and locality (E4) on every frame and bounding route-shape jumps by a measured budget; `gesture_tests` moves stocks and drags flow ends on generated scenes and applies each commit through the patch path, requiring the invariants and model/view agreement.
- **FFI.** `tests/integration/editing.rs` drives each entry point from a press to an applied patch and checks the refusals and NULL safety.

## Follow-ups

- `diagram::flow_geometry::flow_invariant_violations`, the importer and layout tests' oracle, restates the same invariants with message text those tests match on. It should become a formatter over `editing::invariants` once those tests assert arms; its cloud clause (any cloud inside any stock) reads the checker's G6 more broadly, which the migration has to settle.
- The web editor could plan through the engine once it has a main-thread WASM instance.
- Scenes are rebuilt whole after a commit; per-element scene updates would make large models cheaper to edit.
- Plans carry an undo label; no host keeps a history yet.

## Phases

1. The geometry core with the invariant checker and its generated-scene tests.
2. The planner: hit testing, taps, every gesture kind, previews, delete and rename planning.
3. The `EditView` op.
4. The libsimlin surface and header.
