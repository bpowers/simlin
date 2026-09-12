# Diagram editing core: one geometry owner, one gesture planner, one engine executor

## Why

The interactive editor (`src/diagram`) had no single owner for flow geometry,
for what a gesture does, or for how an edit reaches the engine. A four-part
adversarial audit (seeded invariant fuzzing of the production routing
functions, a preview-vs-commit sweep of every Canvas gesture, a commit-path
audit against the real WASM engine, and a measurement of every flow in the
`test/` corpus) found about fifty distinct defects. They are symptoms of three
structural problems:

1. **Flow geometry has no owner.** Straight-to-bend conversion had seven
   implementations with four thresholds and three corner conventions; stock face
   selection had at least six rules; valve placement at least nine. Callers of
   `UpdateCloudAndFlow` fabricated an endpoint element parked at the old
   position plus an inverted delta, each differently. Consequences included
   endpoints committed at a stock's center, pipes running along a stock face,
   routes through stock bodies, valves teleporting between segments, and a
   detached cloud landing half a stock width from the pointer.
2. **Preview and commit were different code.** The Canvas previewed through one
   layered pipeline and pointer-up dispatched to per-gesture Editor handlers
   that recomputed through others. Every seam was a jump on release (#830 and its
   variants). Clicks with no movement committed: a click on a stock-attached flow
   end detached the flow from its stock, a click on a link arrowhead deleted the
   link. Invalid (red) drop targets still committed. A selection change mid-drag
   crashed the Canvas; a pointer-up exception left half a gesture live.
3. **The commit path could save a model that disagrees with its diagram.**
   Handlers awaited the model patch before applying the view, built full
   replacement payloads from state read before that await, committed the view
   even when the patch failed, sent optimistic element lists with viewport-only
   updates, and undo disposed the engine an in-flight patch was using. Attaching a
   flow onto a stock that already listed it duplicated the inflow, which the engine
   integrates twice with no error. Creating a variable under an existing name
   silently replaced that variable, whatever its kind.

## Facts the design rests on

- XMILE 1.0 section 6.1.2 (`docs/reference/xmile-v1.0.html`): a flow's `pts` "MUST
  form right angles". The spec says nothing about attachment faces, clouds, or
  valve position.
- Corpus (490 imported models, 572 flows): 98.8% of flows have two points, none
  has four or more; 578/579 segments are exactly axis-aligned; of stock endpoints
  on a face, 593/595 have the adjacent segment perpendicular and leaving outward;
  72% of face endpoints are off-center; 8% of stock endpoints are off the face
  (Vensim stocks larger than 45x35, up to 254px); 99% of valves are on the path;
  75% of cloud endpoints coincide exactly with the cloud center and 99.7% within
  `CloudRadius`. XMILE imports can carry one flow listed as an outflow of two stocks.
- The engine's layout (`src/simlin-engine/src/layout/orthogonal.rs`) rebuilds pipes
  "so each segment leaves its stock face perpendicular".
- Rendering is pinned byte-for-byte to the Rust renderer (`tests/svg-rendering.test.ts`):
  stored points are the render input, the cloud endpoint is stored at the cloud
  center (the renderer retracts it), the valve is drawn where stored, and the
  arrowhead is drawn 7.5px back along the final segment.
- libsimlin applies a patch atomically: it stages a clone and restores it on any
  error (`src/libsimlin/src/patch.rs`); a view-only patch takes a fast path with no
  model validation.
- An engine round trip of a view is not value-exact: floats can move by one ULP,
  `isStraight` is re-derived from `arc`, and `nextUid` is recomputed as max uid + 1. A
  second round trip is a fixed point.
- `RenameVariable` renames equations, module references and group members, not view
  elements; the editor pairs it with an `upsertView`.

## Units

Model coordinates (px at zoom 1): `CORNER_CLEARANCE` = 3, `MIN_SEGMENT` (shortest
routed stub/riser) = 10, `VALVE_CLAMP_MARGIN` = 10, `MIN_SINK_SEGMENT` =
`FlowArrowheadRadius` + 7.5, `PIPE_SPACING` (preferred distance between endpoints on
a face) = 10, `GEOMETRY_EPSILON` = 1e-6. Screen px: `ClickDragThresholdPx` = 5
(divided by zoom before comparing to model deltas).

## Invariants

Every committed edit produces a view that holds these for every flow the edit
routed. Input views may violate them (imported data, legacy saves); the editor must
accept such input without asserting, render it unmodified while idle, and heal a
flow only when an edit routes it.

- **G1 structure**: >= 2 points; first point attached to the source, last to the
  sink (a stock, or a cloud owned by this flow); interior points unattached; finite
  coordinates; no uid <= 0 in a committed view; source and sink are different
  elements (strict only: imports can carry a self-loop, #720).
- **G2 orthogonal**: every segment axis-aligned (cloud-to-cloud included; spec 6.1.2).
- **G3 normalized**: no zero-length segment; no two consecutive collinear segments;
  every routed stub/riser >= `MIN_SEGMENT` and the final segment >= `MIN_SINK_SEGMENT`
  whenever the terminals leave room (otherwise the longest achievable).
- **G4 face attachment**: a stock endpoint lies on a face at least `CORNER_CLEARANCE`
  from its corners.
- **G5 perpendicular exit**: the segment adjacent to a stock endpoint is perpendicular
  to that face and leaves outward.
- **G6 no body crossing**: no segment passes through the interior of either terminal
  stock, and no cloud center lies inside a stock -- whenever the two terminal
  bodies, each inflated by `MIN_SEGMENT`, do not overlap. When they overlap, G1-G5
  still hold and G6 is best effort.
- **G7 cloud coincidence**: a cloud endpoint equals its cloud's center.
- **G8 valve on path**: the valve lies on the path, at least `VALVE_CLAMP_MARGIN` from
  the path's ends when the path is long enough.
- **M1 kind agreement**: every stock/flow/aux/module element names an existing
  variable of the same kind; a created element's variable exists after the commit.
- **M2 stock/flow agreement**: after an edit changes a flow end's attachment, the flow
  is removed from the old stock's list and present exactly once in the new stock's;
  no other list entry changes.
- **M3 referential integrity**: links, aliases and clouds reference existing elements;
  a link's ends are distinct named elements or aliases (never a cloud); an alias refers
  to a named element; every cloud is an endpoint of its owning flow exactly once.

Readings the checkers (`tests/support/flow-invariants.ts`, `view-invariants.ts`) pin:
tolerant mode checks only the structural G1 arms (heal repairs every geometric arm, and
the Vensim importer emits unattached flows); in G3 the first segment is a stub, interior
segments are risers, and "room" means the source body inflated by `MIN_SEGMENT` and the
sink body inflated by `MIN_SINK_SEGMENT` do not overlap; G6's cloud clause means any stock
in the view; G8's "long enough" is a path at least 2 x `VALVE_CLAMP_MARGIN` long, the
margin measured by arc length from the path ends; G1's uid clause covers every element;
G5 is undefined for an off-face endpoint (G4 owns it); in M2 a created or deleted flow is
unattached on its missing side, renames are excluded, list order is not compared, and
stale imported entries on uninvolved stocks must stay untouched; M1 and M3 are
committed-view properties of the elements an edit routes or creates -- imported input
may violate them (an orphan cloud, lookup-only variables without a primary element)
and the editor must accept it.

Routing preference (not an invariant): a flow that newly lands on a face takes a
slot at least `PIPE_SPACING` from existing endpoints on that face if one exists,
else the slot maximizing the minimum distance. Slides along a face are exempt.

Gesture invariants:

- **E1 click is not a drag**: movement below the click threshold commits nothing and
  previews nothing -- except an armed creation tool, where a click creates the element.
- **E2 preview == commit**: pointer-up commits `planGesture` evaluated at the pointer-up
  coordinates, which is exactly the frame the preview shows at those coordinates.
- **E3 continuity**: within a gesture, geometry changes continuously with the pointer
  except at documented discrete transitions: crossing the click threshold (and the
  slide/offset latch), snapping onto or off a target, a route changing shape because
  feasibility changed, `offsetSegment`'s riser appearing at `MIN_SEGMENT` beyond a face
  extent, and healing an imported flow on its first routed frame.
- **E4 locality**: elements the gesture does not route are unchanged in the planner's
  next view (exactly), and within `GEOMETRY_EPSILON` after an engine round trip,
  including other flows' attachment offsets.
- **E5 abort**: the gesture is dropped without committing and without throwing on
  pointercancel, a second pointer, pinch, a change of the controller's state token
  (below), or a change to the geometry or attachment fields of the elements the gesture
  reads (the routed set and target candidates), compared with `GEOMETRY_EPSILON` and
  ignoring derived fields (`isStraight`, `nextUid`, `var`). Republishes that do not
  change those fields (sim results attached, error annotations, a round trip, the
  gesture's own press-time selection change) do not abort.
- **E6 invalid drops abort**: a drop over a target rendered invalid commits nothing.

## Scope

In scope: the first stock-flow view of each model (the one the editor renders);
stocks, flows, clouds, auxes, modules, aliases, links (arc form), labels.
Out of scope, unchanged: groups (move only when explicitly selected; never
rubber-band selected), additional views, link `multiPoint` geometry, array
dimension compatibility of attachments.

## Architecture

```
drawing/Canvas.tsx          shell: pointer capture, coordinates, viewport physics,
                            one `activeGesture` value, renders planGesture(...).elements
gesture-planner.ts          pure: classifyPress, planGesture
plan-delete.ts              pure: planDelete
flow-geometry/              pure: geometry (units, boxes), terminal (face attachment),
                            validity (G2-G6), path (normalize, valve, slideValve),
                            route (route/routeEnd), offset-segment, heal; index re-exports
view-model-sync.ts          pure: renames, per-flow-end stock ops, created/deleted variable ops
Editor.tsx                  one gesture commit handler + details/module/sim-spec edits,
                            all expressed as controller edits
project-controller.ts       committed + pending[] per model, state token, one executor
```

### flow-geometry/

Positions are absolute model coordinates, never inverted deltas. Every operation
reads the gesture's base flow, never a previous frame, and returns the base geometry
unchanged when a terminal or coordinate is not finite.

- **Terminal**: `{ kind: 'stock'; stock; face?; offset? }` or `{ kind: 'free'; point;
  cloud? }` (a cloud or the pointer). One module owns face attachment -- the valid face
  points within the face extent minus `CORNER_CLEARANCE`, the outward direction, and
  the stub tip (`plane + sign * minimum`) -- and `route`'s candidates, `routeEnd`'s
  pinned terminal, `offsetSegment`'s tail re-solve and `heal`'s re-pin all use it.
  `face`/`offset` carry the BASE flow's attachment (stickiness is defined against the
  gesture's base view, so it needs no previous frame). A corner endpoint belongs to
  the face its adjacent segment leaves perpendicular to. `occupied` (other endpoints on
  the terminal stocks, for the slot preference) is in the frame's coordinates: a
  planner moving a stock moves the endpoints on it too.
- **heal(flow, terminals, { stocks })**: idempotent, identity on a valid flow. Order:
  attach endpoints to their terminals (a cloud is moved onto the endpoint rather than
  the pipe onto the cloud, and out of any stock in `stocks` along its segment; an
  off-face stock endpoint is re-pinned to the nearest valid face point, a corner
  endpoint clamped along the face its adjacent segment is perpendicular to), then snap
  slightly-diagonal segments by dominant axis, then normalize, then re-route from the
  moved terminals only if the result still violates G2-G6.
- **route(source, sink, ctx)**: the minimal orthogonal polyline between two terminals.
  Candidate faces per stock terminal x shapes (straight, L, Z). Interior holds are
  candidates owned by the port pair: the base corners clamped into the pair's band
  between its stub tips, a minimum riser either side of a point port, the band's
  midpoint, then stub tips and body clearances. Ordering: validity (G3-G6) -> not
  crossing a terminal body (G6's best effort when the bodies overlap is a preference,
  not no constraint) -> stickiness (base face kept if it has a candidate within one
  bend of the best; stickiness is the only base-face preference, since a pure function
  has no previous frame for hysteresis) -> bends -> axis change -> length (within
  `GEOMETRY_EPSILON`, a tie). When nothing simpler is valid, a U detour and then 3-4
  bends are tried (clouds a pixel off each other's line, or a stock over its own cloud,
  have no valid straight, L or Z). Total: if no candidate is valid, the least severe
  fault wins (G6 before G3 before structure); a route is always returned.
- **routeEnd(flow, end, terminal, ctx)**: re-route one end with a preserved prefix.
  Try preserving k interior corners counted from the FIXED end, k = K..1 (K = all but
  the corner adjacent to the re-routed end), then k = 0 with the fixed terminal pinned
  to its face and offset (no detours), and only then release to `route`. A candidate is
  accepted only if it is valid, crosses no terminal body, and meets every G3 minimum
  even where G3 excuses them. A preserved tail may not give the path more bends or
  more U turns than the base had (preserving corners keeps a shape; a tail that grows
  it is a detour that releasing replaces), nor run back over a preserved segment
  within `MIN_SEGMENT`; tails may take up to 3 bends when those budgets allow.
- **offsetSegment(flow, segmentIndex, coordinate, terminals, { stocks })**: move one
  segment perpendicular to itself. The coordinate is first resolved against the
  adjacent segments jointly, as the nearest coordinate where every stock stub keeps its
  minimum and every adjacent riser or cloud segment is at least its minimum or
  collapsed to zero (so a short riser collapses within half a minimum and is pushed out
  otherwise). If the path would still cross a terminal body or put a cloud inside a
  stock, the nearest valid coordinate on either side of the obstacle is taken (the
  segment follows the pointer to the obstacle and jumps across once the far side is
  nearer). Adjacent corners follow. At a terminal end the tail is re-solved: a cloud
  moves with the segment; a stock endpoint sits at `clamp(coordinate, face extent minus
  CORNER_CLEARANCE)`. While the coordinate is within `MIN_SEGMENT` beyond that extent
  the segment stays at the extent; beyond that a stub plus a riser connects the
  endpoint to the segment, each at least `MIN_SEGMENT` (the final segment at the sink
  end at least `MIN_SINK_SEGMENT`) (a documented E3 transition, so G3 holds). A tail of
  the form endpoint -> stub (at most the end's minimum, perpendicular) -> riser is
  recognized as re-solvable when the dragged segment follows the riser, so dragging a
  bracket back collapses it to straight and stubs never accumulate. A straight flow
  between aligned stocks slides within the faces first and becomes a bracket beyond
  them.
- **translate(flow, delta)**: both terminals move by the same delta.
- **Valve policy**: the valve is an arc-length position measured from the fixed end
  (from the source for operations with no fixed end). Routing preserves that distance,
  clamped to the new path length; `slideValve` moves it by the pointer delta projected
  along the path as straight-line pointer travel from the press (keeping the grab
  offset, crossing corners, and lagging the pointer at a corner by design);
  `offsetSegment` keeps the valve's coordinate along its own segment while that
  segment survives (clamped into its new span), and moves it to the nearest point of
  the new path when its segment is removed. `VALVE_CLAMP_MARGIN` is applied once, at
  the end.
- **normalize**: remove zero-length and collinear interior points.
- **Documented transitions a drag shows**: dragging a cloud perpendicular off a
  straight stock flow slides the endpoint along its face while nothing pinned is valid,
  then returns the endpoint to its base offset when the pinned Z becomes valid (at
  `MIN_SEGMENT`), then becomes an L (at `MIN_SINK_SEGMENT`); an endpoint that started
  off-center steps at 5, 10 and 16px accordingly. The accepted pinned routes occupy
  regions of pointer positions whose edges are G3 minima, so a region two minima bound
  has a corner, and a pointer path clipping that corner flip-flops (A -> B -> A over a
  few px). A cloud circling its stock does so in a band of radii just past each corner's
  distance (for a top-face source, r in [44.2, 45.7) through the pinned Z's corner). A
  pure function with no previous frame cannot avoid this; the sweeps pin those bands.

### gesture-planner.ts

```ts
type Gesture =
  | { kind: 'moveSelection' }
  | { kind: 'slideValve'; flow: UID }
  | { kind: 'offsetSegment'; flow: UID; segmentIndex: number }
  | { kind: 'flowEndpoint'; flow: UID; end: 'source' | 'sink' }
  | { kind: 'linkEndpoint'; link: UID }
  | { kind: 'linkArc'; link: UID }
  | { kind: 'createFlow'; from: { stock: UID } | 'empty' }
  | { kind: 'createLink'; from: UID }
  | { kind: 'createElement'; type: 'aux' | 'stock' | 'module' }
  | { kind: 'label'; uid: UID }
  | { kind: 'rubberBand' }
  | { kind: 'pan' };

planGesture(input: {
  view; variables; selection; gesture; press: Point; current: Point;
  zoom; pointerType; readOnly; names: NameAllocator;
}): {
  elements; nextUid;                 // what the preview renders and the commit saves
  target?: { uid: UID; valid: boolean };
  commit: 'none' | 'edit' | 'select';
  selection; handoff?: { editName: UID };
};
```

`classifyPress` maps (hit element and part, modifiers, armed tool, selection, pointer
type) to a gesture, a press-time selection change, and whether the tool is cleared.
Its table is derived from the current `Canvas.handleSetSelection`/`handlePointerDown`
arms and covers every arm, including: modifier-press on a selected element (toggle out,
`commit: 'none'` for any following drag); an armed tool on an element it does not apply
to (tool cleared, normal press semantics); link tool on empty canvas; a cloud in a
multi-element selection (moveSelection) vs a sole or unselected cloud (flowEndpoint); an
alias as a link source; label double-click (name edit), module double-click (drill-in);
the name-editor overlay press and tool change while editing (commit the name); a mouse
move with buttons = 0 (lost release: the gesture is cancelled, not committed) and a
second touch (pinch, E5). A pipe or valve press latches `offsetSegment` (segment under
the pointer) iff the first move beyond the click threshold is perpendicular-dominant,
else `slideValve`; the latch holds for the rest of the gesture.

Semantics:

- **moveSelection**: selected positioned elements translate; a flow with both ends
  moving translates; a flow with one moving end is `routeEnd`-ed to its moved terminal;
  a selected flow with no moving end slides its valve; links whose endpoint elements'
  positions changed (valves moved by routing included) get their arc updated once, from
  the final elements.
- **flowEndpoint**: the dragged end follows the pointer, keeping the grab offset
  relative to the endpoint. Target hit-testing uses the pointer. Over a valid stock the
  end routes to that stock; over empty space it becomes (or stays) a cloud at the
  endpoint; over an invalid target nothing commits. Cloud-attached and stock-attached
  ends are one gesture.
- **createFlow**: `route` from a source terminal whose face is chosen by the route (a
  stock) or a cloud at the press point, to the pointer; then as flowEndpoint.
- **createElement**: a draft element staged by the Canvas; the drag positions it; a
  click places it at the press point. The draft lives in Canvas state through name
  editing (it survives republishes); cancel discards it; done plans the create against
  the view rendered at done time and enqueues it.
- **createLink / linkEndpoint / linkArc / label**: existing semantics behind the same
  interface; a click never deletes a link; a drop on its own source or on nothing
  aborts.
- A target is valid exactly when committing onto it yields a view holding the
  invariants and the semantic rules: a flow's source and sink are different stocks,
  and the target's stock variable exists.

Resolutions made while implementing (the module is `src/diagram/gesture-planner/`):

- The Canvas plans on the view it is rendering each frame. A live gesture aborts (E5)
  when the state token moves or the view's geometry changes by value from the view
  captured at press (`sameGeometry`); a pan reads no geometry and is exempt. The
  comparison covers the whole view rather than the gesture's read set: a real change
  to ANY element aborts, including one the gesture does not read. That is simpler
  than tracking read sets and costs little, since geometry changes mid-gesture only
  when another edit lands. Benign republishes keep the gesture: a pending edit
  landing (floats one ULP away, `isStraight`, `var` and `nextUid` re-derived), sim
  results attaching, error annotations updating. The commit carries its base view,
  and the Editor drops a commit whose base no longer matches the view it would edit.
- A selection change mid-gesture does not abort: the gesture plans with the selection
  captured at press.
- A flow end that is unattached (a Vensim fallback flow) gets a new cloud when the
  gesture routes the flow, so every committed flow holds G1.
- A move whose routed flows break an invariant the plan does not excuse commits
  nothing, like an invalid drop: a cloud dragged into another stock while the flow's
  terminals are apart. Because G6 and the G3 minima are best effort when the terminal
  bodies leave no room, a stock dragged onto or up to its flow's other terminal
  still commits, holding G1-G5.
- Only the aux, stock and module tools place an element on a click; a flow or link tool
  click creates nothing. A creation press clears the selection rather than selecting a
  sentinel uid.
- A label drag that ends on the side the label already has commits nothing; the label
  gesture's threshold belongs to `Label`, so the planner applies none to it.
- Module double-click (drill-in) is classified before `pressesDisabled`: navigation is
  not an edit.
- A pointercancel or a lost release on a pan settles the viewport it reached.
- Flows render identically while moving and at rest (no retracted-arrow or hidden-grip
  variants), so the last preview frame equals the committed frame.

`planDelete(view, selection)` (in its own module, `plan-delete.ts`, because the delete
path is keyboard- and panel-driven rather than a pointer gesture) removes the selected
elements, links touching them,
aliases of them, and clouds of deleted flows; flow endpoints on deleted stocks become
clouds at the endpoint; a selected cloud whose flow survives is ignored.

Names: default names for new elements are allocated against the committed model plus
pending creates. A typed name that collides keeps the name editor open with an error
and enqueues nothing. A collision discovered at dequeue (only reachable if the allocator
is wrong) fails the item like any other engine error.

### view-model-sync.ts

`buildEditOps(committedModel, baseView, nextView)`, evaluated at dequeue against the
committed model produced by the previous edit item of that model. Every ident it uses is
derived from an element's `name` (canonicalized), never from the element's `ident`
field, which a caller-built element need not keep in step; a view planned on a pending
rename then resolves against the committed model the rename produced.

- **Renames**: a named element whose uid survives with a different `name` emits
  `renameVariable` from the committed ident to the new name as typed. The rename edit's
  next view is the rendered view with that element relabeled; nothing else carries the
  rename, so a combined rename and reattach needs no special path. Stock list entries
  echoed by the stock/flow ops are carried through the rename.
- **Stock/flow delta**, per flow element whose source (sink) attachment differs between
  base and next: remove the flow from the old stock's outflows (inflows) and add it to
  the new stock's, deduped. Only existing flow variables and existing stock variables
  are touched, and stocks deleted by the same edit are excluded (deleting a stock needs
  no list cleanup); no other entry of any list changes. One `updateStockFlows` per
  touched stock, carrying both full lists from the committed model with the deltas
  applied. "Existing" means present in the committed model or created by this same edit
  (a drawn flow between two stocks is listed in both). The echoed lists omit entries
  naming variables this edit deletes, because the patch applies `deleteVariable` first
  and the engine strips a deleted flow from every list; echoing it would re-add it. A
  stock whose lists come out unchanged (attaching onto a stock that already lists the
  flow) gets no op.
- **Created variables**: upserts for named elements present in next and absent in base.
- **Deleted variables**: `deleteVariable` for named elements removed in next whose
  variable exists in the committed model and has no remaining element.
- Op order in one patch: rename, variable upserts/deletes, stock/flow ops, `upsertView`
  last.

### Controller: committed + pending[] per model, a state token, one executor

State:

- `committed`: the engine's last acknowledged project (model + views).
- `token`: an integer bumped by every truncation, undo/redo, and any other change to
  `committed` not produced by the pending chain (reopen, external reload).
- `pending[]`: queued edit items in order. Each carries its target `modelName`, the
  `token` it was planned under, the view it planned on, its next view (every edit that
  changes what a view shows has one -- including rename, whose next view is the rendered
  view with the element relabeled), and `buildOps(committedModel)`. Model-only edits
  (details panel equation/table edits, module wiring, sim specs) have no next view and
  derive their full payload from the committed variable at dequeue, so `stockToJson`'s
  echoed inflows/outflows are never stale.
- `viewport`: the live viewBox/zoom per model.

Rendered view for model M = the next view of the last pending item targeting M (or
committed M), with `viewport` for M overlaid. `applyOptimisticView`, `preserveLiveView`
and `adoptPatchedViews` are replaced by this rule. Connector errors are computed on the
rendered view (the engine's per-variable incoming links plus the rendered connectors).
Every rendered element's `ident` is its name's (a create or rename sets it, and a rename
matches elements by name, so renaming an element whose rename or create is pending finds
it). While a pending view renames a committed variable, the rendered model names that
variable by its new ident, with its committed content, errors and connector dependencies,
so the canvas, the details panel and name allocation see the model the queued edits will
produce. A model-only edit resolves its variable at dequeue through the element: the uid
it was enqueued for, on the committed view.

Executor: one serialized async loop; every engine call runs inside an item, and no
engine reference is held across an await outside one. Two item classes:

- **Edit items** (and viewport persist items), FIFO. An edit item with a next view whose
  `token` is stale is dropped as failed; model-only edits derive everything at dequeue and
  are exempt (one that targets a variable a truncated item would have created fails
  naturally). Otherwise it sends its ops plus `upsertView` (its next view with
  the committed viewport; for a model-only edit none) as one patch, serializes, rebuilds
  `committed`, and records one history entry. A viewport persist item sends
  `upsertView(committed view with viewport)` -- never an optimistic element list --
  records no history, and is never truncated.
- **Maintenance items** (save, sim run, error refresh, connector-error attach): at most
  one queued per kind, never truncated, and run when no edit item is queued -- or after 5
  consecutive edit items or 5 seconds of continuous edit work, whichever comes first, so a
  sustained stream of slow edits cannot starve saving. A burst of edits costs one sim run
  and one save.

Failure of view edit item k (engine error or stale token): truncate every later edit with a
next view and every later undo/redo (each was planned on k's optimistic view), bump
`token` (which aborts live gestures, E5; a stale-token drop does not bump again), render
`committed`, report one error naming how many later edits were discarded. Model-only edits
survive truncation: they derive their payload from committed state at dequeue, so an
unrelated failure does not invalidate them, and discarding one would silently lose the
user's typed text. One that targets a variable a discarded edit would have created fails
naturally at dequeue and reports its own error. A failed model-only edit only reports:
no edit was planned on it, so nothing is truncated and `token` does not move; an undo
queued behind it for the draft it carried is discarded with it. A patch that applied but
could not be read back resyncs `committed` before its fate is decided (its next view keeps
rendering meanwhile): a successful re-read means it landed; a reopen of the snapshot at
the history cursor means it failed, and it fails after the swap, so edits planned on it
during the reopen are truncated too; a failed reopen latches engine-unavailable -- the
queue settles, every later request is refused quietly, and the host shows one persistent
notice offering a reload.
Undo/redo are edit-class items; the UI and the keyboard shortcut are disabled while
`pending` is non-empty or a gesture is live, the Canvas ignores new presses while an
undo/redo item is queued, a view edit enqueued while one is queued is refused quietly, and
landing bumps `token`. An Undo press with a details-panel draft submits the draft and
queues the undo behind its edit, so the undo takes the draft back; a Redo press queues
the redo first and submits the draft behind it, so the draft lands on the redone project.
A create or rename refused while an undo is queued returns a message, keeping the name
editor open.
Navigation need not wait for the queue; its viewport restore is a viewport item for the
target model.

Details panel drafts: any canvas press first flushes an open panel draft (the panel
commits synchronously, enqueueing its edit ahead of the gesture), because a canvas press
does not blur the panel editor. Panels are keyed on the selected element and its variable's
committed content (plus the read-only flag and a counter that moves only when an undo/redo
lands), not on a global generation, so an unrelated edit item landing while the user types
does not remount the panel and discard the draft; and while the panel holds a draft its key
is held, so an edit landing on this variable (a rename rewriting its equation, the draft's
own flushed edit while more text is typed) does not either. A field holds a draft when its
text differs from its base -- what the panel last submitted for it, or its seeded text -- and
only drafts are submitted: an untouched field is never echoed, and a field changed back
while its edit is in flight holds the panel so that edit cannot land over it. A submission
that does not land stops being the base. While an undo/redo is queued the panels render
read-only.

Engine defense in depth: stock inflow/outflow lists are deduped after canonicalization
for every op that sets them (`updateStockFlows`, `upsertStock`), so a duplicate can never
be integrated twice. Upsert keeps full-replacement semantics (MCP and pysimlin rely on
kind-changing upserts); name collisions are the editor's to prevent.

## Testing

- Committed invariant checkers (`tests/support/flow-invariants.ts`,
  `tests/support/view-invariants.ts`) with strict and tolerant modes, and a seeded scene
  and gesture generator (no new dependencies). Fuzz every gesture kind over generated
  scenes and 8-step sequences; assert G*, M*, E1-E6; `route`/`routeEnd` return a route
  for every input, including overlapping stocks.
- Table tests with rows derived from enumerations: terminal kind (4 faces, free) x path
  shape x drag direction for `route`/`routeEnd`; segment position (first/interior/last) x
  terminal kind x path length (2/3/4/6) x direction x reversal across gestures for
  `offsetSegment`; every arm of `classifyPress` and `Gesture` (click, sub-threshold,
  read-only, invalid target, abort); `buildEditOps` over delete stock x flow attached as
  source/sink/both, and delete stock together with one of its flows.
- Old suites (`flow-routing`, `group-movement`, `flow-attach`, `canvas-gestures-*`,
  `canvas-interaction`, `selection-logic`) are mined: each scenario is ported as a row or
  dropped with a stated reason (including #53, #818, #819, #720, #832, touch-straight
  links, wobble-is-a-click). Tests pinning the #820 commit-anyway policy are rewritten
  to the rollback policy.
- Canvas harness: one DOM-diff test per gesture kind (last frame vs committed view);
  the harness does not bump `version` on selection changes and fixtures pin endpoints to
  faces as production does.
- Controller: tables against the fake engine for truncation, stale tokens (a gesture
  planned before a failure, an edit planned before an undo lands), maintenance
  coalescing, viewport items, per-model rendering during navigation, undo gating; Editor
  + real WASM engine: model/view invariants after every gesture, including in-flight
  races, undo during a pending patch, and typing in a details panel then dragging a stock.
- Corpus: every imported flow routes under each gesture without throwing, and routed
  flows hold the strict invariants (run after the engine import fixes).
- Browser journey (notebook-widget e2e harness): drag a stock, detach and reattach a
  flow, offset a flow into a bracket; screenshots for review.

## Engine import and layout fixes

Found by the corpus measurement, fixed in the same branch as separate commits:
XMILE `Stock::is_left`/`is_above` sign errors; XMILE takeoff fixup reading top-left
coordinates as centers; clouds created before 2-point straightening; MDL endpoints left
off-face for stocks larger than 45x35; MDL flows imported unattached (invisible) --
attachments follow the model's stock lists; layout clamping endpoints onto corners and
orthogonalizer Ls missing the valve; incremental layout rewriting untouched flows;
duplicate flow elements when two stocks claim one outflow; duplicate stock
inflow/outflow entries.

## Phases

Each phase: one implementer, then an adversarial reviewer (mutation testing of new
tests, hashed snapshot), iterate, commit.

1. Test support: invariant checkers, generator.
2. `flow-geometry.ts` with exhaustive and fuzz tests (not yet wired).
3. `gesture-planner.ts` with tests (not yet wired).
4. `view-model-sync.ts` and the controller executor: committed + pending[] per model,
   token, item classes, undo gating; every Editor handler (including details panel,
   modules, sim specs, rename) migrated to controller edits while the Canvas still uses
   the old geometry. Independent of phases 2-3, so it runs in a parallel worktree.
5. Canvas and Editor gestures rewired onto the planner; old routing,
   `group-movement.ts`, `flow-attach.ts` removed; harness fidelity; gesture and
   Editor+engine tests.
6. Engine import/layout fixes (parallel worktree), merged before the corpus test.
7. Docs (`LAYOUT.md`, `src/diagram/CLAUDE.md` invariants), browser journey, removal of
   audit scratch files and logs, PR.
