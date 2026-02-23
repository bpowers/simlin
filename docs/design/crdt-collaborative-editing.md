# CRDT-Based Collaborative Editing: Research & Analysis

## Summary

This document surveys the Rust CRDT library landscape and analyzes how each
option maps onto Simlin's data model, architecture, and goals. The objective is
collaborative editing of system dynamics models by multiple humans and/or AI
agents simultaneously, with the CRDT layer living in Rust (usable from WASM in
browsers, natively compiled in Python/CLI, and on a server).

## The Data We're Collaboratively Editing

The canonical data shape is `datamodel::Project` (Rust) /
`simlin-project.schema.json` (JSON). It's a tree:

```
Project
├── name: String
├── simSpecs: SimSpecs { start, stop, dt, method, timeUnits }
├── dimensions: Vec<Dimension>
├── units: Vec<Unit>
└── models: Vec<Model>
    ├── name: String
    ├── variables: Vec<Variable>  (Stock | Flow | Aux | Module)
    │   ├── ident/name: String
    │   ├── equation: String  (the SD equation text)
    │   ├── units: Option<String>
    │   ├── documentation: String
    │   └── ... (graphicalFunction, inflows/outflows, etc.)
    ├── views: Vec<View>
    │   └── StockFlow { elements: Vec<ViewElement>, viewBox, zoom }
    │       └── ViewElement: Aux | Stock | Flow | Link | Module | Alias | Cloud | Group
    │           └── { uid, name, x, y, labelSide, ... }
    ├── loopMetadata: Vec<LoopMetadata>
    └── groups: Vec<ModelGroup>
```

Key characteristics of this data for CRDT selection:

1. **Hierarchical/nested**: Project → Model → Variable, View → ViewElement
2. **Keyed collections**: Variables are identified by `ident`, view elements by
   `uid`. These are semantically maps, not positional lists.
3. **Equation strings**: The equation field is where humans and AI spend most
   editing time. Rich text is not needed — these are plain-text SD equations.
4. **Geometric data**: View elements have x/y coordinates, flow points, link
   arcs. These are LWW (last-writer-wins) values.
5. **Cross-references**: Stocks reference flows by name (`inflows`/`outflows`),
   Links reference other elements by `uid`, Modules reference models by name.
6. **Moderate document size**: Typical models have 10–200 variables. Large
   Vensim models can have 1000+. The data fits easily in memory.

## Rust CRDT Library Landscape

### Loro (Recommended)

- **Repository**: https://github.com/loro-dev/loro
- **Maturity**: 1.0 stable (released 2024, currently v1.10.x as of Feb 2026).
  Stable data format, active maintenance, ~5.2k GitHub stars.
- **Language bindings**: Rust (native), JavaScript (WASM), Swift, Python, React
  Native.
- **Data types**: Text (Fugue algorithm), List, MovableList, Map (LWW), Tree
  (movable tree CRDT), Counter.
- **WASM support**: First-class. The JS bindings are WASM-compiled from Rust.
- **Structured data**: Containers can be nested — a Map value can itself be a
  Map, List, or Text. Documents must be trees (not DAGs). This is a natural fit
  for our Project schema.
- **Key features**: Full history DAG, time travel, undo/redo, incremental sync
  (export/import updates), shallow snapshots, version forking/merging.
- **Performance**: Built on the Eg-walker algorithm (from diamond-types). Does
  not keep CRDT structures in memory for editing — reconstructs them on demand.
  Optimized columnar encoding (influenced by Automerge). Fast snapshot loading.
- **API style**: `LoroDoc` → `get_map("name")` / `get_text("name")` /
  `get_list("name")`. Imperative mutations with event subscriptions.

**Why Loro is the best fit for Simlin:**

1. **Map + Text composition**: Our model is fundamentally maps-of-maps with
   text leaf values (equations). Loro's composable containers (Map containing
   Maps containing Text) model this directly.
2. **Movable Tree**: View element ordering and model hierarchy can use the Tree
   CRDT, which handles concurrent moves correctly (no cycles, no lost nodes).
3. **Stable 1.0 with active development**: Not experimental. The data format is
   stable, meaning documents created today will be readable by future versions.
4. **Same language, same binary**: Rust core compiled to WASM or native. No
   language impedance mismatch. The CRDT layer can live alongside simlin-engine.
5. **Undo/redo per-peer**: Critical for multi-user editing — each user can undo
   their own changes without affecting others.
6. **No server required**: Peer-to-peer sync via update export/import. We can
   layer server-mediated sync on top, but the CRDT doesn't require it.

**Risks/concerns:**

- Loro is younger than Automerge/Yjs. The community is smaller, though growing.
- The Rust API requires manual mapping between our `datamodel::Project` structs
  and `LoroDoc` containers (no derive-macro equivalent to Autosurgeon).
- Performance with very large models (1000+ variables) needs benchmarking,
  though our data volume is small relative to text editing workloads.

### Automerge

- **Repository**: https://github.com/automerge/automerge
- **Maturity**: Mature (v2.0+, rewritten in Rust). Academic pedigree. The most
  well-known CRDT library. Active maintenance.
- **Language bindings**: Rust (native), JavaScript (WASM), Swift, C (FFI).
- **Data types**: JSON-like document model — nested maps and lists. Text type.
  No dedicated Tree CRDT.
- **WASM support**: First-class (`@automerge/automerge-wasm` npm package).
- **Structured data**: Natively JSON-like. `Autosurgeon` provides derive macros
  (`#[derive(Reconcile, Hydrate)]`) for Rust struct ↔ Automerge document
  binding.
- **Key features**: Sync protocol (automerge-repo), change history,
  undo (manual), save/load with columnar compression.
- **Performance**: Historically slow, dramatically improved in 2.0 but still
  slower than Loro/Yrs in benchmarks. Large documents can be expensive to load.

**Fit for Simlin:**

- JSON document model maps naturally to our data.
- `Autosurgeon` would let us derive CRDT bindings directly on
  `datamodel::Project` structs, reducing boilerplate significantly.
- No built-in Tree CRDT means view element hierarchy and model tree operations
  would need custom handling.
- The sync protocol (`automerge-repo`) is more mature than Loro's, with
  existing WebSocket/WebRTC transports.
- Slower performance is unlikely to matter at our document sizes, but the
  ecosystem momentum is shifting toward Loro.

### Yrs (Yjs in Rust)

- **Repository**: https://github.com/y-crdt/y-crdt
- **Maturity**: Stable, tracks Yjs behavior. Funded by NLnet/EU. Binary
  protocol compatible with Yjs (JS interop).
- **Language bindings**: Rust (native), WASM (ywasm), C (yrs-ffi), Python
  (via Quantstack).
- **Data types**: Text (YText), Array (YArray), Map (YMap). No Tree CRDT.
- **WASM support**: Yes (ywasm package).
- **Structured data**: Maps and arrays compose, similar to Automerge. Less
  JSON-native than Automerge — more oriented toward collaborative text editing.
- **Key features**: Awareness protocol (cursor positions), V1/V2 encoding,
  large Yjs ecosystem (ProseMirror, CodeMirror, Monaco, Tiptap bindings).
- **Performance**: Competitive with Yjs. Has shown incorrect merge results in
  some benchmark traces (3 of 5 in json-joy benchmarks), which is concerning.

**Fit for Simlin:**

- Strong text editing story (best editor integrations in the ecosystem).
- Less natural for structured JSON documents — designed for text-first use
  cases.
- Yjs ecosystem is huge, but it's JavaScript-centric. Yrs is a port, not the
  primary implementation.
- No derive-macro story for Rust struct binding.
- Correctness concerns in some benchmarks warrant caution.

### Diamond-Types

- **Repository**: https://github.com/josephg/diamond-types
- **Maturity**: 1.0 released. Single author (Seph Gentle). Research-oriented.
- **Data types**: **Text/list only**. The `more_types` branch has work toward
  JSON-style data types, but it's not shipped.
- **WASM support**: Yes.
- **Performance**: Extremely fast for text — the fastest text CRDT in
  benchmarks. Designed as a performance research vehicle.

**Fit for Simlin:** Not viable — no structured data support. Influential as a
research project (Loro's Eg-walker is derived from diamond-types), but not
usable as our CRDT layer.

### Cola

- **Repository**: https://github.com/nomad/cola
- **Maturity**: v0.5.1, relatively early. Small community.
- **Data types**: **Text only**.
- **WASM support**: Not first-class.
- **Performance**: Very fast for text (1.4-2x faster than diamond-types in some
  benchmarks).

**Fit for Simlin:** Not viable — text only, no structured data.

## Recommendation Summary

| Library | Structured Data | Text | Tree | WASM | Rust-native | Stability | Fit |
|---------|:-:|:-:|:-:|:-:|:-:|:-:|:-:|
| **Loro** | Map, List, MovableList | Fugue | MovableTree | Yes | Yes | 1.0 stable | **Best** |
| Automerge | JSON maps/lists | Yes | No | Yes | Yes | 2.0 mature | Good |
| Yrs | Map, Array | YText | No | Yes | Yes | Stable | Moderate |
| Diamond-Types | No | Yes | No | Yes | Yes | 1.0 | Not viable |
| Cola | No | Yes | No | Limited | Yes | 0.5.x | Not viable |

**Recommendation: Loro.** It has the best combination of structured data
support, WASM compatibility, Rust-native implementation, and modern
algorithms. The 1.0 stable data format and active development give confidence
in long-term viability.

Automerge is the strongest alternative, especially if the `Autosurgeon` derive
macros significantly reduce integration effort. Consider it as a fallback if
Loro's manual mapping proves too burdensome.

## Architecture: How Loro Fits Into Simlin

### Conceptual Model

The `LoroDoc` becomes the shared, CRDT-backed representation of a
`datamodel::Project`. Each peer (browser tab, Python notebook, AI agent, server)
holds a `LoroDoc` and synchronizes via update messages.

```
┌─────────────┐      updates       ┌─────────────┐
│  Browser A  │◄──────────────────►│   Server    │
│  LoroDoc    │                    │  LoroDoc    │
│  Engine(wasm)│                    │             │
└─────────────┘                    └──────┬──────┘
                                          │ updates
                                          │
┌─────────────┐      updates       ┌──────┴──────┐
│  Browser B  │◄──────────────────►│  AI Agent   │
│  LoroDoc    │                    │  LoroDoc    │
│  Engine(wasm)│                    │  Engine(native)│
└─────────────┘                    └─────────────┘
```

### Document Schema Mapping

```
LoroDoc (root)
├── "name" → LoroText
├── "simSpecs" → LoroMap { "start", "stop", "dt", "method", ... }
├── "dimensions" → LoroMap<dimension_name, LoroMap { "elements" → LoroList, ... }>
├── "units" → LoroMap<unit_name, LoroMap { "equation", "disabled", "aliases" }>
└── "models" → LoroMap<model_name, LoroMap>
    ├── "variables" → LoroMap<var_ident, LoroMap>
    │   ├── "type" → "stock" | "flow" | "aux" | "module"
    │   ├── "equation" → LoroText  ← collaborative equation editing
    │   ├── "units" → LoroText
    │   ├── "documentation" → LoroText
    │   ├── "inflows" → LoroList (stocks only)
    │   ├── "outflows" → LoroList (stocks only)
    │   ├── "gf" → LoroMap (graphical function)
    │   └── ...
    ├── "views" → LoroList<LoroMap>
    │   └── "elements" → LoroMap<uid_string, LoroMap>
    │       ├── "type" → "stock" | "flow" | "aux" | "link" | ...
    │       ├── "x" → f64 (LWW via LoroMap)
    │       ├── "y" → f64
    │       └── ...
    ├── "loopMetadata" → LoroMap<name, LoroMap>
    └── "groups" → LoroMap<name, LoroMap>
```

Key design decisions in this mapping:

1. **Variables as Map<ident, ...> not List**: Variables are keyed by identifier,
   not positionally ordered. This means concurrent "add variable" operations
   never conflict (different keys). Using a LoroList would create unnecessary
   ordering conflicts.

2. **Equations as LoroText**: This is the critical win. Two users editing
   different parts of the same equation get character-level merge, not
   last-writer-wins replacement. An AI agent adding a term while a human fixes
   a typo merge cleanly.

3. **View elements as Map<uid, ...>**: Same rationale as variables — keyed by
   uid, not positional. Concurrent "add element" operations don't conflict.

4. **Coordinates as plain LoroMap values**: x/y positions get LWW semantics
   (last write wins), which is correct for drag operations — if two users drag
   the same element, one wins, and the result is still a valid position.

### Sync Layer

Loro's sync is based on exporting/importing binary update blobs:

```rust
// Peer A makes changes
let updates = doc_a.export(ExportMode::updates(&peer_b_version)).unwrap();
// Send `updates` over WebSocket/HTTP/whatever
// Peer B applies them
doc_b.import(&updates).unwrap();
```

For Simlin, the natural transport is:
- **Browser ↔ Server**: WebSocket (already the direction the app is heading).
  The server maintains a `LoroDoc` per project and fans out updates.
- **AI Agent ↔ Server**: Same WebSocket or HTTP polling, depending on agent
  lifecycle.
- **Peer-to-peer** (future): WebRTC data channels, using Loro's version
  vectors for consistency.

### Integration with simlin-engine

This is where the incremental compilation design (doc/design/2026-02-21-incremental-compilation.md)
becomes highly synergistic:

1. **LoroDoc → datamodel::Project**: A sync function converts the current
   LoroDoc state to a `datamodel::Project`. This is the "read" path — the
   engine consumes `datamodel::Project` as its input.

2. **Patches → LoroDoc mutations**: When the UI or an agent makes a change,
   it mutates the `LoroDoc` directly. A derived `datamodel::Project` is then
   extracted and fed to the engine (or, with incremental compilation, the
   salsa inputs are updated directly from the LoroDoc delta).

3. **Event subscriptions**: Loro fires events when the document changes (local
   or remote). The engine subscribes to these events and triggers
   recompilation of affected portions — which is exactly what the salsa-based
   incremental compilation is designed to handle efficiently.

```
LoroDoc mutation (local or remote)
    │
    ├── subscribe_root callback fires
    │
    ├── Diff applied to salsa SourceVariable/SourceModel inputs
    │   (only the changed variable's input is updated)
    │
    └── salsa incrementally recompiles only affected pipeline stages
```

This is the ideal pairing: Loro tells us *what* changed (which variable, which
field), and salsa ensures we only recompile *what's affected*.

## Implications and Prerequisites

### Work That Should Happen Before or Alongside CRDT Integration

#### 1. Incremental Compilation (High Priority, Prerequisite)

The incremental compilation design is nearly a prerequisite for good CRDT
performance. Without it, every remote edit triggers a full recompilation. With
salsa, a remote change to one variable's equation triggers only that variable's
reparse/recompile. This is the difference between sub-millisecond response and
a noticeable pause on every keystroke from a collaborator.

**Status**: Design complete (doc/design/2026-02-21-incremental-compilation.md),
implementation not started.

#### 2. Variables-as-Map Refactoring (Medium Priority)

Currently, `Model.variables` is a `Vec<Variable>` — a positional list. For
CRDT purposes, this should be a map keyed by identifier. The Rust datamodel
already treats variables as "find by ident" (see `Model::get_variable`), so
the in-memory representation is already semantically a map even if stored as a
Vec.

Options:
- **Change nothing in datamodel**: Map the `Vec<Variable>` to a `LoroMap`
  keyed by ident in the CRDT layer. Keep `Vec` for serialization compatibility.
- **Refactor to `BTreeMap<String, Variable>`**: Better alignment between
  in-memory representation and CRDT schema. Breaking change for protobuf
  serialization (would need migration).

The first option (keep Vec, map to LoroMap) is lower risk and probably the
right initial approach.

#### 3. View Elements uid-Keyed Map (Medium Priority)

Same issue as variables: `StockFlow.elements` is a `Vec<ViewElement>`. For
CRDT purposes, it should be keyed by `uid`. The existing `get_variable_name`
method already searches by uid, confirming the semantic-map nature.

#### 4. Equation Conflict Semantics (Design Decision Needed)

When two users edit the same equation concurrently, Loro's text CRDT will merge
character-by-character. This is usually correct for prose, but SD equations have
structure. Consider:

- User A changes `a + b` to `a + b + c`
- User B changes `a + b` to `a * b`
- Merge: `a * b + c` — syntactically valid but semantically questionable.

This is an inherent limitation of text CRDTs. Possible mitigations:
- Post-merge validation (the engine already does this via compilation errors).
- UI indication that an equation was auto-merged and should be reviewed.
- Per-variable locking (coarse-grained, loses fine-grained collaboration).

Recommendation: Accept text CRDT merges and rely on compilation feedback. The
engine will immediately flag broken equations, and the existing error-reporting
pipeline surfaces these to the user. This is similar to how collaborative code
editors work — merges can create syntax errors, and the language server flags
them.

#### 5. UID Generation Strategy (Design Decision Needed)

View elements use `i32` UIDs. In a CRDT world, UIDs must be globally unique
across peers. Options:
- Use Loro's built-in ID generation for tree nodes.
- Generate UUIDs and map to i32 via a local table.
- Switch to string-based UIDs in the datamodel.

This needs design work before implementation.

#### 6. Server Architecture Changes (Future)

The current server stores complete protobuf snapshots in Firestore. With CRDTs:
- The server would hold a `LoroDoc` per project.
- Firestore would store Loro snapshots + update logs (or just snapshots at
  periodic intervals).
- The API would shift from "POST full project" to "WebSocket update stream."
- Authentication/authorization per-operation becomes important (who can edit
  which model? which variable?).

This is a significant architectural change but doesn't need to happen first —
CRDT collaboration can work peer-to-peer before server integration.

#### 7. libsimlin FFI Surface (Low Priority Initially)

The current FFI exposes `apply_patch` which takes a full JSON patch. With
CRDTs, the "patch" is a Loro update blob. Options:
- Add a new FFI function `simlin_project_apply_loro_update(update_bytes)`.
- Keep `apply_patch` for non-collaborative use cases, add CRDT path alongside.
- Long term: the LoroDoc lives inside the WASM module (Rust), and the
  TypeScript layer just sends Loro updates through.

The most natural architecture: the `LoroDoc` lives inside `SimlinProject`
alongside the salsa database. TypeScript sends Loro update bytes across the
WASM boundary. The Rust side applies them to the LoroDoc, derives
datamodel changes, and feeds those into salsa.

### Refactorings Worth Doing Regardless

These improve the codebase independent of CRDT choice:

1. **`@simlin/core` dependency inversion** (tech-debt item #6): Core depends on
   Engine, but should be a leaf package. Resolving this makes the CRDT layer
   easier to place in the dependency graph.

2. **TypeScript test coverage** (tech-debt item #14): Collaborative editing
   will need extensive testing of sync scenarios. Starting from near-zero test
   coverage in app/core makes this harder.

3. **`unwrap()` in libsimlin FFI** (tech-debt item #8): Panicking across FFI is
   UB. CRDT integration adds more code paths through the FFI that need proper
   error handling.

## Open Questions

1. **Persistence format**: Should we store Loro snapshots in Firestore, or
   continue storing protobuf and derive Loro state on load? Loro snapshots are
   self-contained but not human-readable. Protobuf snapshots are the status quo
   but lose CRDT history.

2. **Granularity of AI agent edits**: Should an AI agent editing a model make
   one large transaction (commit all variable changes at once) or many small
   ones (one per variable)? The answer affects undo granularity and merge
   behavior.

3. **Offline-first scope**: Is the goal full offline-first (work offline,
   merge on reconnect) or primarily online collaborative editing with
   occasional brief disconnections? This affects how much Loro history we
   retain and how we handle divergent branches.

4. **Automerge as alternative**: If Autosurgeon's derive macros significantly
   reduce integration effort vs. Loro's manual mapping, is the performance
   difference worth it? For our document sizes, probably not. A small
   spike/prototype comparing the integration effort would be informative.

## Suggested Phasing

1. **Phase 0** (now): Incremental compilation. This is the foundation that
   makes CRDT-driven updates efficient.

2. **Phase 1**: Loro integration in simlin-engine only. Define the LoroDoc
   schema, implement `LoroDoc → datamodel::Project` and
   `datamodel::Project → LoroDoc` conversion. Write property tests ensuring
   round-trip fidelity. No networking yet — just the data model mapping.

3. **Phase 2**: Wire LoroDoc into libsimlin. Add FFI for applying Loro updates.
   Connect LoroDoc events to salsa input updates (from Phase 0).

4. **Phase 3**: TypeScript/WASM integration. The browser holds a LoroDoc
   (Loro's WASM JS bindings), sends updates to the WASM engine module. Local
   editing works through LoroDoc instead of the current JSON patch path.

5. **Phase 4**: Server integration. WebSocket transport for Loro updates.
   Server holds authoritative LoroDoc per project. Persistence to Firestore.

6. **Phase 5**: AI agent integration. Agent holds a LoroDoc, connects to
   the same sync channel, makes edits as a peer.
