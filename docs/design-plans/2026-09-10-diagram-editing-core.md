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
gesture-planner.ts          pure: classifyPress, planGesture, planDelete
flow-geometry.ts            pure: terminals, route/routeEnd, offsetSegment, valve, heal
view-model-sync.ts          pure: per-flow-end stock ops, created/deleted variable ops
Editor.tsx                  one gesture commit handler + details/module/sim-spec edits,
                            all expressed as controller edits
project-controller.ts       committed + pending[] per model, state token, one executor
```

### flow-geometry.ts

Positions are absolute model coordinates, never inverted deltas.

- **Terminal**: `{ kind: 'stock'; stock; face?; offset? }` or `{ kind: 'free'; point }`
  (a cloud or the pointer). One function owns face attachment -- the valid face points
  within the face extent minus `CORNER_CLEARANCE`, the outward direction, and the
  stub/riser shapes that reach a face -- and both `route`'s candidates and
  `offsetSegment`'s tail re-solve use it. `face`/`offset` carry the BASE flow's
  attachment (stickiness is defined against the gesture's base view, so it needs no
  previous frame).
- **heal(flow, terminals)**: idempotent, identity on a valid flow. Order: attach
  endpoints to their terminals (a cloud is moved onto the endpoint rather than the
  pipe onto the cloud; an off-face stock endpoint is re-pinned to the nearest valid
  face point), then snap slightly-diagonal segments by dominant axis, then normalize.
- **route(source, sink, ctx)**: the minimal orthogonal polyline between two terminals.
  Candidate faces per stock terminal x shapes (straight, L, Z, and 3-bend tails when a
  preserved segment forces them). Ordering: validity (G3-G6) -> stickiness (base face
  kept if it has a valid candidate within one bend of the best) -> bends -> axis change
  -> length. A face other than the base face must pass validity with an extra
  `MIN_SEGMENT` margin (pure-function hysteresis against jitter at a validity boundary).
  Total: if no candidate is valid, relax G6 only (the precondition in G6); a route is
  always returned.
- **routeEnd(flow, end, terminal, ctx)**: re-route one end with a preserved prefix.
  Try preserving k interior corners counted from the FIXED end, k = K..0 (K = all but
  the corner adjacent to the re-routed end); the first k with a valid tail wins. At
  k = 0 the fixed terminal stays pinned to its face and offset; only if that is still
  invalid is it released to `route`.
- **offsetSegment(flow, segmentIndex, coordinate, terminals)**: move one segment
  perpendicular to itself. The coordinate is first clamped to the feasible interval
  (no body crossing, stubs >= `MIN_SEGMENT`). Adjacent corners follow. At a terminal
  end the tail is re-solved: a cloud moves with the segment; a stock endpoint sits at
  `clamp(coordinate, face extent minus CORNER_CLEARANCE)`. While the coordinate is
  within `MIN_SEGMENT` beyond that extent the segment stays at the extent; beyond that a
  stub plus a riser connects the endpoint to the segment, each at least `MIN_SEGMENT`
(the final segment at the sink end at least `MIN_SINK_SEGMENT`)
  (a documented E3 transition, so G3 holds). A tail of the form endpoint -> stub
  (<= `MIN_SEGMENT` + eps, perpendicular) -> riser is recognized as re-solvable when the
  dragged segment follows the riser, so dragging a bracket back collapses it to
  straight and stubs never accumulate. A straight flow between aligned stocks slides
  within the faces first and becomes a bracket beyond them.
- **translate(flow, delta)**: both terminals move by the same delta.
- **Valve policy**: the valve is an arc-length position measured from the fixed end
  (from the source for operations with no fixed end). Routing preserves that distance,
  clamped to the new path length; `slideValve` moves it by the pointer delta projected
  along the path (keeping the grab offset, crossing corners); `offsetSegment` preserves
  the valve's arc-length distance from the terminal on the valve's side of the dragged
  segment (from the dragged segment's start when the valve is on it).
  `VALVE_CLAMP_MARGIN` is applied once, at the end.
- **normalize**: remove zero-length and collinear interior points.

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

`planDelete(view, selection)` removes the selected elements, links touching them,
aliases of them, and clouds of deleted flows; flow endpoints on deleted stocks become
clouds at the endpoint; a selected cloud whose flow survives is ignored.

Names: default names for new elements are allocated against the committed model plus
pending creates. A typed name that collides keeps the name editor open with an error
and enqueues nothing. A collision discovered at dequeue (only reachable if the allocator
is wrong) fails the item like any other engine error.

### view-model-sync.ts

`buildEditOps(committedModel, baseView, nextView)`, evaluated at dequeue against the
committed model produced by the previous edit item of that model:

- **Stock/flow delta**, per flow element whose source (sink) attachment differs between
  base and next: remove the flow from the old stock's outflows (inflows) and add it to
  the new stock's, deduped. Only existing flow variables and existing stock variables
  are touched, and stocks deleted by the same edit are excluded (deleting a stock needs
  no list cleanup); no other entry of any list changes. One `updateStockFlows` per
  touched stock, carrying both full lists from the committed model with the deltas
  applied.
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

Failure of edit item k (engine error or stale token): truncate `pending` from k (every
later edit was planned on k's optimistic view), bump `token` (which aborts live gestures,
E5), render `committed`, report one error naming how many later edits were discarded.
Undo/redo are edit-class items; the UI and the keyboard shortcut are disabled while
`pending` is non-empty or a gesture is live, the Canvas ignores new presses while an
undo/redo item is queued, and landing bumps `token`.
Navigation need not wait for the queue; its viewport restore is a viewport item for the
target model.

Details panel drafts: any canvas press first flushes an open panel draft (the panel
commits synchronously, enqueueing its edit ahead of the gesture), because a canvas press
does not blur the panel editor. Panels are keyed on the selected variable's committed
content (plus the read-only flag), not on a global generation, so an unrelated edit item
landing while the user types does not remount the panel and discard the draft.

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
3. `gesture-planner.ts`, `view-model-sync.ts` with tests (not yet wired).
4. Controller executor: committed + pending[] per model, token, item classes, undo
   gating; every Editor handler (including details panel, modules, sim specs, rename)
   migrated to controller edits while the Canvas still uses the old geometry.
5. Canvas and Editor gestures rewired onto the planner; old routing,
   `group-movement.ts`, `flow-attach.ts` removed; harness fidelity; gesture and
   Editor+engine tests.
6. Engine import/layout fixes (parallel worktree), merged before the corpus test.
7. Docs (`LAYOUT.md`, `src/diagram/CLAUDE.md` invariants), browser journey, removal of
   audit scratch files and logs, PR.
