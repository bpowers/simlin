// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::borrow::Cow;
use std::collections::HashMap;
use std::fmt;
use std::marker::PhantomData;
use std::{error, result};

use crate::ast::Loc;

// Legacy type aliases - to be deprecated
pub type DimensionName = String;
pub type ElementName = String;

// ===== Canonical-identifier string interner =====
//
// A global, thread-safe, de-duplicating, *non-leaking* string interner. It
// backs all three canonical identifier newtypes so that constructing one for a
// string that has already been seen is a hashmap probe plus an `Arc` clone --
// no fresh `String` allocation -- and `Clone` is an atomic refcount bump.
//
// Why a hand-rolled interner rather than the `internment` crate: the obvious
// choice (`internment::ArcIntern<str>`) is backed by `dashmap`, which pulls in
// `ahash` -> `getrandom`. `getrandom` does not build for the
// `wasm32-unknown-unknown` target without a `wasm_js` cfg, and the engine ships
// in the libsimlin wasm bundle (`src/engine/build.sh`). The previous engine
// dependency graph had no `getrandom` edge, so adopting `internment` would
// break the wasm build (which is part of the pre-commit hook and CI). This
// std-only interner avoids that entirely while preserving the same semantics
// (dedup, O(1) clone, refcount-reclaim on last drop, thread-safe).

/// The heap-allocated, refcounted payload of one interned canonical string.
/// Held behind an `Arc`; when the last `Arc` drops, the `Drop` impl evicts the
/// now-dead entry from the interner so the table does not grow without bound
/// across the lifetime of a long-running process.
struct Interned {
    /// The shard this entry lives in, cached so `Drop` can re-lock the right
    /// shard in O(1) without recomputing it from the hash.
    shard: usize,
    s: Box<str>,
}

/// Number of shards. A power of two so the shard index is a cheap mask of the
/// hash. The interner is a process-global (`GLOBAL` below) reachable from any
/// thread, so sharding bounds lock contention without pulling in a
/// concurrent-map dependency.
///
/// The concurrency it bounds is NOT compilation, which runs on one thread today
/// (measured at 0.9996 CPUs utilized). It is `layout::generate_best_layout`'s
/// best-of-k seed fan-out -- the engine's only rayon call site -- plus any host
/// driving several `SimlinDb`s at once.
const INTERNER_SHARDS: usize = 64;

/// One shard: a content-keyed map from string -> weak handle. A `Weak`
/// (not `Arc`) is stored so the entry does not itself keep the payload alive;
/// the payload is reclaimed when the last *external* `Arc` drops.
///
/// Keyed with `FxHashMap` (rustc's fixed-seed FxHash) rather than the std
/// default SipHash: identifier strings are short and the interner is on the
/// hottest compile path (`canonicalize`/`Ident::new` -> `intern`), so the
/// per-shard get/insert hashing is a measurable share of compile self-time.
/// The hasher is purely a performance detail here -- the map still de-dups by
/// string CONTENT, so which strings share a payload is unaffected.
type Shard = rustc_hash::FxHashMap<Box<str>, std::sync::Weak<Interned>>;

/// The global interner: a fixed array of independently-locked shards.
///
/// Shard selection hashes the key with `FxBuildHasher` (the per-shard map
/// rehashes the key with its own FxHash hasher). `FxBuildHasher` is a
/// zero-size, fixed-seed unit type, so there is nothing to store: the shard
/// chosen for a given string at insert is the same shard `hash_of` recomputes
/// for `contains` and that `Drop` recomputes for eviction. The hasher being
/// fixed-seed (vs the old `RandomState`'s per-process random seed) only makes
/// shard selection deterministic across runs; dedup-by-content is unchanged.
struct Interner {
    shards: [std::sync::Mutex<Shard>; INTERNER_SHARDS],
}

impl Interner {
    fn global() -> &'static Interner {
        // `std::sync::OnceLock` (std-only) lazily initializes the global.
        static GLOBAL: std::sync::OnceLock<Interner> = std::sync::OnceLock::new();
        GLOBAL.get_or_init(|| Interner {
            shards: std::array::from_fn(|_| std::sync::Mutex::new(Shard::default())),
        })
    }

    fn hash_of(&self, s: &str) -> u64 {
        use std::hash::BuildHasher;
        // `FxBuildHasher` is a fixed-seed zero-size unit type, so a fresh
        // `default()` is the same hasher every call -- shard selection stays
        // self-consistent between insert, `contains`, and eviction.
        rustc_hash::FxBuildHasher.hash_one(s)
    }

    /// Total number of entries currently held across all shards. Test-only:
    /// used to assert that dropping the last handle reclaims the entry (the
    /// non-leaking invariant). Not exact under concurrency, but the unit tests
    /// that call it use process-unique strings on a single thread.
    #[cfg(test)]
    fn live_entry_count(&self) -> usize {
        self.shards.iter().map(|m| m.lock().unwrap().len()).sum()
    }

    /// Whether a specific string currently has a live interned entry.
    /// Test-only reclaim probe.
    #[cfg(test)]
    fn contains(&self, s: &str) -> bool {
        let hash = self.hash_of(s);
        let shard_idx = (hash as usize) & (INTERNER_SHARDS - 1);
        let shard = self.shards[shard_idx].lock().unwrap();
        shard.get(s).map(|w| w.upgrade().is_some()).unwrap_or(false)
    }

    /// Intern `s`: return an `Arc<Interned>` shared with any live handle for the
    /// same content, allocating a new payload only on the first sighting.
    fn intern(&self, s: &str) -> std::sync::Arc<Interned> {
        let hash = self.hash_of(s);
        let shard_idx = (hash as usize) & (INTERNER_SHARDS - 1);
        // `.unwrap()` on the shard lock: poisoning is unreachable here and in
        // `Drop` -- the only work done while holding a shard lock is hashmap
        // operations and the `Box`/`Arc` allocations below, none of which can
        // unwind (allocation failure aborts), so the `Mutex` can never be
        // poisoned.
        let mut shard = self.shards[shard_idx].lock().unwrap();

        if let Some(weak) = shard.get(s)
            && let Some(arc) = weak.upgrade()
        {
            // Live entry: share it. (If `upgrade` fails the payload is mid-drop
            // -- its `Drop` will remove the dead weak; we fall through and
            // replace it below, which is safe because the guard in `Drop`
            // only removes an entry whose weak still points at the dead payload.)
            return arc;
        }

        let arc = std::sync::Arc::new(Interned {
            shard: shard_idx,
            s: Box::from(s),
        });
        // Insert (or overwrite a dead weak) keyed by an owned copy of the
        // string. Overwriting a dead weak here is what makes the `Drop` guard
        // (`std::ptr::eq` on the weak target) necessary for correctness.
        //
        // This allocates a second `Box<str>` for the map key (the payload owns
        // the first). It is deliberately left simple: this is the cold
        // first-sighting path (one distinct canonical string ever reaches it
        // once), not the hot reuse path that the `upgrade()` fast path above
        // serves, so the extra allocation does not show up in the compile
        // profile. Sharing one allocation would mean keying the map on an
        // `Arc<str>` cloned from the payload, trading the allocation for a
        // second `Arc` indirection on every access -- not worth it here.
        shard.insert(Box::from(s), std::sync::Arc::downgrade(&arc));
        arc
    }
}

impl Drop for Interned {
    fn drop(&mut self) {
        // Reclaim the table entry for this now-dead payload. Re-lock the same
        // shard and remove the entry *only if* its weak still refers to this
        // payload: a concurrent `intern` of the same string after our strong
        // count hit zero may have installed a fresh `Arc` (and overwritten the
        // weak); we must not evict that live replacement.
        let interner = Interner::global();
        let mut shard = interner.shards[self.shard].lock().unwrap();
        if let Some(weak) = shard.get(self.s.as_ref()) {
            // `Weak::as_ptr` is stable across upgrade/downgrade and identifies
            // the payload. If it points at `self`, this entry is ours to evict.
            if std::ptr::eq(weak.as_ptr(), self as *const Interned) {
                shard.remove(self.s.as_ref());
            }
        }
    }
}

/// Interned, de-duplicated storage for a canonical identifier string.
///
/// This is the single backing store for all three canonical identifier
/// newtypes (`Ident<Canonical>`, `CanonicalElementName`,
/// `CanonicalDimensionName`). Constructing one for a string that has already
/// been interned is a hashmap hit plus an atomic refcount bump -- no new
/// `String` allocation -- and `Clone` is likewise a refcount bump, so cloning
/// identifiers (which the compiler does constantly) is O(1). Entries are
/// reclaimed when the last handle drops (see `Drop for Interned`), so a
/// long-lived process that compiles many distinct models does not leak.
///
/// The string stored here is assumed to already be in canonical form; callers
/// canonicalize before constructing (see `Ident::new` / `from_raw`). The
/// `_unchecked` constructors trust the caller, matching the previous
/// `String`-backed contract.
///
/// ## Trait impls (and why they are manual)
///
/// We implement the comparison/hash traits deliberately and let the three
/// public newtypes simply `#[derive(...)]` (which delegates to the impls here):
/// - `Hash` is **value based** (`self.as_str().hash()`). This is required so
///   `HashMap<Ident, _>` lookups via the `Borrow<str>` path stay sound: the map
///   hashes the `&str` key with `str`'s hasher and must find the entry whose
///   key hashes identically.
/// - `PartialEq`/`Eq` use `Arc` pointer equality, which is value-correct
///   precisely because the interner de-duplicates (one payload per distinct
///   string), and is consistent with the value-based `Hash`.
/// - `Ord`/`PartialOrd` are **lexicographic by string content**. Many
///   `BTreeSet`/`BTreeMap` orderings and the deterministic byte-stable runlists
///   depend on this; a pointer-address ordering would be non-deterministic
///   across runs.
/// - salsa: a re-executed query's memo is backdated purely by `PartialEq`
///   (`values_equal`), so the pointer-equality `PartialEq` above is what
///   decides whether a downstream query sees a change.
#[derive(Clone)]
pub(crate) struct CanonicalStorage(std::sync::Arc<Interned>);

// `Debug` is unconditional (not gated on the `debug-derive` feature) because
// `Ident<State>` derives `Debug` unconditionally (it predates the feature),
// and a field type must be `Debug` for that derive to hold with the feature
// off. Printing the canonical string is the useful representation anyway.
impl fmt::Debug for CanonicalStorage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self.as_str(), f)
    }
}

impl CanonicalStorage {
    /// Intern a string that is already in canonical form. A hashmap hit plus an
    /// `Arc` refcount bump when the string has been seen before; otherwise
    /// allocates the backing payload once.
    fn intern(canonical: &str) -> Self {
        CanonicalStorage(Interner::global().intern(canonical))
    }

    /// Borrow the canonical string.
    fn as_str(&self) -> &str {
        &self.0.s
    }
}

impl PartialEq for CanonicalStorage {
    fn eq(&self, other: &Self) -> bool {
        // De-duplication guarantees one payload per distinct string, so O(1)
        // `Arc` pointer equality is exactly value equality. (Fast path; falls
        // back to nothing -- distinct pointers always mean distinct strings.)
        std::sync::Arc::ptr_eq(&self.0, &other.0)
    }
}

impl Eq for CanonicalStorage {}

impl std::hash::Hash for CanonicalStorage {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        // Value-based, consistent with `str`'s Hash so `Borrow<str>` HashMap
        // lookups work (see the type-level docs). Must NOT hash the pointer.
        self.as_str().hash(state);
    }
}

impl PartialOrd for CanonicalStorage {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for CanonicalStorage {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        // Lexicographic by string content: runlist determinism and BTree
        // ordering depend on this (NOT pointer address).
        self.as_str().cmp(other.as_str())
    }
}

/// A canonicalized identifier - guaranteed to be in canonical form (OLD - being replaced)
///
/// Canonical form means:
/// - Lowercase
/// - Spaces/newlines replaced with underscores
/// - Dots outside quotes replaced with middle dot (·)
/// - Properly handles quoted sections
///
/// A raw, non-canonicalized identifier as it appears in source.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct RawIdent(String);

/// A canonicalized dimension name
///
/// Backed by interned, de-duplicated storage (see [`CanonicalStorage`]): the
/// derived `PartialEq`/`Eq`/`Hash`/`Ord`/`PartialOrd` all delegate to that
/// handle's manual impls (value equality + value hash + lexicographic order),
/// so the observable behavior is identical to the old `String` backing while
/// construction and clone avoid allocation.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CanonicalDimensionName(CanonicalStorage);

/// A raw dimension name as it appears in source
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct RawDimensionName(String);

/// A canonicalized element name (dimension element)
///
/// Backed by interned, de-duplicated storage (see [`CanonicalStorage`]); the
/// derived trait impls delegate to that handle, matching the old `String`
/// backing's behavior without per-construction allocation.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CanonicalElementName(CanonicalStorage);

/// A raw element name as it appears in source
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct RawElementName(String);

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub enum ErrorCode {
    NoError,      // will never be produced
    DoesNotExist, // the named entity doesn't exist
    XmlDeserialization,
    VensimConversion,
    ProtobufDecode,
    InvalidToken,
    UnrecognizedEof,
    UnrecognizedToken,
    ExtraToken,
    UnclosedComment,
    UnclosedQuotedIdent,
    ExpectedNumber,
    UnknownBuiltin,
    BadBuiltinArgs,
    EmptyEquation,
    BadModuleInputDst,
    BadModuleInputSrc,
    NotSimulatable,
    BadTable,
    BadSimSpecs,
    /// No producer since GH #568 deleted `model.rs`'s second dependency walk,
    /// which was the only site that raised it (on a `\·`-prefixed dependency).
    /// The salsa dependency extraction never had an equivalent check, so this
    /// was already unreachable from every production compile; deleting the walk
    /// made it unreachable from every path. Kept because `ErrorCode` is mapped
    /// to a numbered FFI enum (`src/core/errors.ts`, `src/engine/src/errors.ts`)
    /// and removing a discriminant renumbers its successors. Reinstating the
    /// check belongs in `db::var_fragment`'s dependency walk, not here.
    NoAbsoluteReferences,
    CircularDependency,
    ArraysNotImplemented,
    MultiDimensionalArraysNotImplemented,
    BadDimensionName,
    BadModelName,
    MismatchedDimensions,
    ArrayReferenceNeedsExplicitSubscripts,
    DuplicateVariable,
    UnknownDependency,
    VariablesHaveErrors,
    UnitDefinitionErrors,
    Generic,
    NoAppInUnits,
    NoSubscriptInUnits,
    NoIfInUnits,
    NoUnaryOpInUnits,
    BadBinaryOpInUnits,
    NoConstInUnits,
    ExpectedInteger,
    ExpectedIntegerOne,
    DuplicateUnit,
    /// No producer, for the same reason and with the same caveat as
    /// [`ErrorCode::NoAbsoluteReferences`]: the deleted walk raised it when a
    /// dotted `submodel·output` dependency named a non-module variable.
    ExpectedModule,
    ExpectedIdent,
    UnitMismatch,
    TodoWildcard,
    TodoStarRange,
    TodoRange,
    TodoArrayBuiltin,
    CantSubscriptScalar,
    DimensionInScalarContext,
    BadOverride,
    UnsupportedForSerialization,
    // NOTE: `ErrorCode` is a pure-Rust runtime type. It is NOT part of
    // `project_io.proto` / `project_io.gen.rs` (verified by grep: the proto
    // has no `ErrorCode`), so it is never serialized and new variants may be
    // appended freely. Keep additions at the END of the enum anyway, to keep
    // the existing discriminants stable for any in-memory consumers.
    DuplicateMacroName,
    /// A standalone lookup-only table (a graphical function with no driving
    /// input) was referenced bare -- without applying it to an argument. A
    /// table has no scalar value of its own; it must be called, e.g.
    /// `LOOKUP(my_table, x)` or `my_table(x)` (issue #606).
    LookupReferencedWithoutArgument,
    /// A conveyor stock has no non-leakage outflow (no outflows at all, or every
    /// outflow is `<leak/>`-marked). A conveyor needs one primary outflow that
    /// the belt drives (docs/design/conveyors.md §3.3).
    ConveyorWithoutOutflow,
    /// A conveyor is present under RK2/RK4 integration. The slat model is
    /// defined per-DT and has no meaning under Runge-Kutta substeps, so
    /// conveyors require Euler (docs/design/conveyors.md §9.4).
    ConveyorNonEulerMethod,
    /// A queue is directly upstream of a non-discrete conveyor
    /// (docs/design/conveyors.md §11 / §6.4).
    ConveyorQueueUpstreamNotDiscrete,
    /// A conveyor's transit time (`<len>`) is not positive at compile time
    /// (docs/design/conveyors.md §4.1).
    ConveyorTransitNotPositive,
    /// A conveyor's latched transit time implies more belt slats
    /// (`round(transit/dt)`) than the engine will allocate. The slat count sizes
    /// the belt `Vec`; an enormous `transit/dt` (a hostile or typo'd `<len>`)
    /// would otherwise request an unbounded allocation -- a `usize`-saturating
    /// count panics `vec![0.0; usize::MAX]` -> host abort under `panic = "abort"`,
    /// and a merely-huge finite one OOMs. Rejected loudly at belt init / latch
    /// time rather than silently saturating the belt geometry (see
    /// `conveyor::MAX_SLATS_PER_BELT`, docs/design/conveyors.md §4.1).
    ConveyorTransitTooLong,
    /// A conveyor's transit time is not an integer multiple of DT; the belt is
    /// DT-quantized to the nearest whole slat count. Warning-level
    /// (docs/design/conveyors.md §4.1).
    ConveyorTransitNotDtMultiple,
    /// A conveyor's constant linear leak fractions sum above 1. Warning-level;
    /// the primary outflow will be starved (docs/design/conveyors.md §5.1).
    ConveyorLeakFractionsExceedOne,
    /// LTM analysis was requested on a model containing conveyors; the belt's
    /// internal dynamics are not scored as INTEG. Warning-level
    /// (docs/design/conveyors.md §9.6).
    ConveyorLtmDegraded,
    /// Another equation references a conveyor-driven flow (the primary outflow
    /// or a leak flow) by name. The conveyor pass runs after the flows phase, so
    /// such a reader would read the pre-pass placeholder 0 rather than the
    /// belt-driven rate. Rejected loudly rather than silently mis-computed
    /// (docs/design/conveyors.md §4.3 "Visibility to other equations").
    ConveyorDrivenFlowRead,
    /// A conveyor stock reached the ordinary compile path without being expanded
    /// by the special-stock build path. An INTERNAL invariant guard, not a
    /// backend or model limitation: every backend (the bytecode VM and wasmgen
    /// alike) routes a conveyor model through `queue_compile::compile_sim`, which
    /// expands it. Any other path would integrate the belt as a plain stock and
    /// silently mis-simulate, so it is rejected (docs/design/conveyors.md §9.3).
    ConveyorNotExpanded,
    /// A conveyor inflow requests an `isee:spreadflow` placement whose runtime
    /// wiring is not yet available (`dist` needs the distribution graphical
    /// function evaluated per slat; `source` needs upstream-leak coupling).
    /// Rejected loudly rather than silently placed at the entry
    /// (docs/design/conveyors.md §8).
    ConveyorSpreadflowUnsupported,
    /// An arrayed conveyor stock (or one of its driven flows) is declared over a
    /// dimension the project does not define, so the per-element belt layout
    /// cannot be resolved. An internal-consistency guard on the arrayed-conveyor
    /// expansion (docs/design/conveyors.md §10).
    ConveyorArrayedDimensionUnresolved,
    /// An equation uses a conveyor stock as a **container** in a form that cannot
    /// be lowered. The supported forms -- `SUM`/`MIN`/`MAX`/`MEAN`/`STDDEV`/`SIZE`
    /// over a single belt, and `conv[j]` for a compile-time-constant slat index --
    /// are rewritten to synthesized hidden stocks the passes publish at step
    /// start, on both backends. What stays rejected is a reducer over an
    /// EXPRESSION involving the belt, a dynamic slat index, a range/wildcard over
    /// slats, and a bare arrayed-conveyor reducer other than `SUM`: the belt lives
    /// in a side table with a runtime-dynamic length, not in the fixed-dimension
    /// data buffer. Rejected loudly rather than silently mis-resolving (`SIZE` ->
    /// 1, `MEAN` -> the belt total) or erroring opaquely
    /// (docs/design/conveyors.md §10).
    ConveyorContainerAccessUnsupported,
    /// A queue stock reached the ordinary compile path without being expanded by
    /// the special-stock build path. An INTERNAL invariant guard on every backend,
    /// which all route a queue model through `queue_compile::compile_sim`; any
    /// other path would integrate the FIFO as a plain stock and silently
    /// mis-simulate, so it is rejected (mirrors [`ConveyorNotExpanded`],
    /// docs/design/queues.md §10.3).
    QueueNotExpanded,
    /// A queue is present under RK2/RK4 integration. The per-DT admit-then-serve
    /// batch model is defined per-DT and has no meaning under Runge-Kutta
    /// substeps, so queues require Euler (mirrors [`ConveyorNonEulerMethod`],
    /// docs/design/queues.md §10.3).
    QueueNonEulerMethod,
    /// Another equation references a queue-driven outflow by name. The queue pass
    /// runs after the flows phase, so such a reader would read the pre-pass
    /// placeholder 0 rather than the served rate. Rejected loudly rather than
    /// silently mis-computed (mirrors [`ConveyorDrivenFlowRead`],
    /// docs/design/queues.md §2 "Driven outflow"). The structural
    /// `<inflow>`/`<outflow>` stock linkage is NOT a reference and is not caught
    /// here: a stock fed by the driven outflow via INTEG is correct (the Stocks
    /// phase runs after the pass).
    QueueDrivenFlowRead,
    /// An `<overflow/>` marker appears on a flow that is NOT a queue outflow, or on
    /// a queue's FIRST (highest-priority) outflow. XMILE (§4.3) allows `<overflow/>`
    /// only on a queue outflow, and never on the first one: an overflow is by
    /// definition a lower-priority sibling that activates when a higher-priority
    /// outflow is blocked (docs/design/queues.md §3.3, §10.7). Rejected loudly at
    /// queue-expansion time.
    QueueOverflowNotOnQueue,
    /// LTM (Loops That Matter) analysis was requested on a model containing a
    /// queue. A queue is a stock with non-INTEG dynamics (a FIFO of batches),
    /// so the flow-to-stock link-score numerator assumes plain INTEG under
    /// Euler and any score touching the queue may be wrong. Emitted as a
    /// Warning naming the queue, mirroring `ConveyorLtmDegraded`
    /// (docs/design/queues.md §10.5).
    QueueLtmDegraded,
    /// A conveyor stock is defined in a model that is NOT the main model -- a
    /// module-referenced sub-model, or a model defined but never instantiated.
    /// Conveyor expansion (`conveyor_compile::expand_conveyors`) rewrites only the
    /// main model, so a conveyor anywhere else can never be expanded and would
    /// otherwise trip the internal [`ConveyorNotExpanded`] guard with an
    /// engine-internal message. Support for conveyors inside sub-models is a
    /// deferred feature, not an engine bug, so it is rejected up front with the
    /// offending stock's name and model rather than the internal invariant error.
    /// The spec does not yet state the limitation anywhere; GH #940 tracks writing
    /// it down, and GH #941 is the real-world fixture it blocks.
    ConveyorInSubmodelUnsupported,
    /// A queue stock is defined in a model that is NOT the main model. Queue
    /// simulation is currently supported only in the main model (mirrors
    /// [`ConveyorInSubmodelUnsupported`]; undocumented in the spec, GH #940).
    QueueInSubmodelUnsupported,
    /// A queue outflow OTHER THAN the primary (first, highest-priority) feeds a
    /// conveyor -- an `<overflow/>` sibling or a second ordinary outflow whose
    /// destination is a conveyor stock. Only a queue's first outflow may feed a
    /// conveyor (docs/design/queues.md §4.4/§9): the combined queue-conveyor pass
    /// couples exactly the primary, so a secondary conveyor destination is neither
    /// discipline-guarded nor served under the batch rules. The spec sketches an
    /// overflow-to-conveyor (§4.5) but does not define how a secondary's
    /// redirectable budget interleaves with a (possibly distinct) second belt's
    /// admission budget, so it is rejected loudly at coupling-detection time rather
    /// than silently mis-accounted (which desyncs the queue FIFO / belt stock from
    /// its side table). Fires whether the destination conveyor is discrete or
    /// continuous, and whether the secondary is an overflow or an ordinary outflow.
    QueueSecondaryOutflowToConveyor,
    /// A conveyor stock has more than one NON-leak outflow. The slat model has
    /// exactly one primary (belt-end) outflow that the belt drives, plus any
    /// number of `<leak/>`-marked leakage flows; a second plain outflow has no
    /// place in the model (docs/design/conveyors.md §3.3). Left unhandled it
    /// would stay an ordinary equation-driven outflow of the expanded INTEG
    /// stock: the Stocks phase drains the stock by that rate while the belt side
    /// table never sheds the material, so the reported stock diverges below the
    /// belt total permanently. Rejected loudly at conveyor-expansion time,
    /// naming the conveyor, its primary outflow, and every extra non-leak
    /// outflow (mark the extras with `<leak/>` if leakage was intended).
    ConveyorMultipleNonLeakOutflows,
    /// One stock carries BOTH a `<conveyor>` block and a `<queue/>` marker. XMILE
    /// defines conveyors and queues as distinct stock TYPES; a stock has exactly
    /// one type (docs/design/queues.md §10.7). The two markers are independent
    /// optional fields the reader/proto carry side by side, and the two expansion
    /// passes each clear only their OWN marker
    /// ([`crate::conveyor_compile::expand_conveyors`] clears the conveyor block,
    /// [`crate::queue_compile::expand_queues`] clears the queue marker), so a
    /// both-marked stock would be expanded TWICE -- given both a `ConveyorPlan`
    /// AND a `QueuePlan` over the same stock and shared outflow slot -- and the two
    /// runtime passes would each drive the shared flow (the last writer winning
    /// while belt and FIFO advance under different rates): silent garbage with no
    /// diagnostic. Rejected loudly BEFORE either expansion, naming the stock.
    StockBothConveyorAndQueue,
    /// A conveyor stock's initial `<eqn>` is a §7.2 explicit comma-separated
    /// init list that cannot be used as written: an entry is not a numeric
    /// constant (the list is evaluated once at belt-init time, so only
    /// compile-time constants are supported), or the list-initialized
    /// conveyor's `<len>` is not a compile-time constant (the list-length
    /// interpretation and the initial total depend on the slat count).
    /// Rejected loudly at conveyor-expansion time rather than surfacing as
    /// an opaque equation parse error on the stock
    /// (docs/design/conveyors.md §7.2).
    ConveyorInitListUnsupported,
    /// A non-apply-to-all arrayed variable has an `<element>` entry whose
    /// subscript names no declared element combination of the variable's
    /// dimensions. Every consumer of the per-element list (the compiler's
    /// arrayed expansion, per-element graphical-function table layout, and
    /// conveyor per-element init lists) matches entries by exact canonical
    /// key and silently DROPS an unmatched one, so a one-character typo
    /// simulates plausibly-but-wrong with no signal. Warning-level (GH #905).
    UnknownElementSubscript,
    /// A macro-marked model's body instantiates a module (`Variable::Module`).
    /// A cycle-safety rule, not a taste rule. `db::project_module_graph` -- the
    /// gate every compile, diagnostic, and analysis entry point consults so a
    /// module cycle surfaces as [`ErrorCode::CircularDependency`] rather than as
    /// salsa's dependency-graph cycle panic -- records only EXPLICIT module
    /// edges, because it must stay parse-free. A macro CALL is an implicit edge
    /// it cannot see, so a macro whose body holds an explicit module targeting a
    /// model that calls that macro closes a cycle the gate reports as absent, and
    /// the recursive queries abort on it (fatal under `panic = "abort"`).
    /// Rejected at `MacroRegistry::build`, which restores the invariant that
    /// every module cycle lies in explicit edges.
    ///
    /// The cost is real and accepted, not zero. The shape is reachable from an
    /// ordinary XMILE file -- the `<macro>` content model is shared with
    /// `<model>` and the reader passes a `<module>` through unfiltered -- our own
    /// XMILE writer round-trips it, and a project containing an ACYCLIC one
    /// compiles and simulates correctly today. It is rejected anyway because
    /// narrowing to only-when-cyclic would need a second reachability analysis
    /// whose back edge (the macro call) is only discoverable by parsing every
    /// model's equations, and because a macro is a template that has no business
    /// instantiating a sub-model. The MDL importer cannot produce it, though not
    /// because "Vensim macros have no modules" -- its multi-output materializer
    /// does mint modules; the scoped macro-body context just never runs it.
    ///
    /// Distinct from `CircularDependency` on purpose -- the rejection covers the
    /// acyclic case too, which is not a cycle and must not claim to be one. A
    /// module *targeting* a macro model is unaffected; only a module *inside* one
    /// is rejected. See `MacroRegistry::build`'s Pass 4 for the full argument.
    MacroContainsModule,
    /// A variable's equation is nothing but the NaN literal, so the variable has
    /// no usable equation and every value it produces is NaN.
    ///
    /// This is where Vensim's `A FUNCTION OF(...)` sketch placeholder lands: the
    /// modeller drew the variable and its inputs but has not written the formula
    /// yet, and our MDL importer stores that as the equation text `NAN`. Vensim's
    /// own documentation says the construct "precludes simulation" -- Vensim
    /// refuses to run such a model -- while we compile it, simulate it, and hand
    /// back NaN. A hand-authored XMILE `<eqn>NAN</eqn>` reaches the same place and
    /// means the same thing.
    ///
    /// Warning-level, not Error: the rest of the model is worth simulating, and
    /// `FormattedErrors::push` counts `Error` severity only, so this must not flip
    /// the failure-shaped flags. Its value is ATTRIBUTION -- see `crate::float`'s
    /// module docs for why. A NaN spreads through arithmetic to whatever reads it,
    /// so the modeller's next task is a backward hunt through the dependency graph
    /// for the origin. Naming the one variable the engine knows STRUCTURALLY must
    /// be NaN replaces that entire hunt.
    ///
    /// The spread is through arithmetic only, which is why neither this doc nor
    /// the emitted message claims every downstream variable is NaN: IEEE
    /// comparisons against a NaN are false, so `IF x > 0 THEN 1 ELSE 0` reading a
    /// NaN `x` returns a finite `0` and everything below it is finite too. A
    /// diagnostic that asserted otherwise would send the modeller looking in the
    /// wrong place.
    UnfilledEquation,
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        use ErrorCode::*;
        let name = match self {
            NoError => "no_error",
            DoesNotExist => "does_not_exist",
            XmlDeserialization => "xml_deserialization",
            VensimConversion => "vensim_conversion",
            ProtobufDecode => "protobuf_decode",
            InvalidToken => "invalid_token",
            UnrecognizedEof => "unrecognized_eof",
            UnrecognizedToken => "unrecognized_token",
            ExtraToken => "extra_token",
            UnclosedComment => "unclosed_comment",
            UnclosedQuotedIdent => "unclosed_quoted_ident",
            ExpectedNumber => "expected_number",
            UnknownBuiltin => "unknown_builtin",
            BadBuiltinArgs => "bad_builtin_args",
            EmptyEquation => "empty_equation",
            BadModuleInputSrc => "bad_module_input_src",
            BadModuleInputDst => "bad_module_input_dst",
            NotSimulatable => "not_simulatable",
            BadTable => "bad_table",
            BadSimSpecs => "bad_sim_specs",
            NoAbsoluteReferences => "no_absolute_references",
            CircularDependency => "circular_dependency",
            ArraysNotImplemented => "arrays_not_implemented",
            MultiDimensionalArraysNotImplemented => "multi_dimensional_arrays_not_implemented",
            BadDimensionName => "bad_dimension_name",
            BadModelName => "bad_model_name",
            MismatchedDimensions => "mismatched_dimensions",
            ArrayReferenceNeedsExplicitSubscripts => "array_reference_needs_explicit_subscripts",
            DuplicateVariable => "duplicate_variable",
            UnknownDependency => "unknown_dependency",
            VariablesHaveErrors => "variables_have_errors",
            UnitDefinitionErrors => "unit_definition_errors",
            Generic => "generic",
            NoAppInUnits => "no_app_in_units",
            NoSubscriptInUnits => "no_subscript_in_units",
            NoIfInUnits => "no_if_in_units",
            NoUnaryOpInUnits => "no_unary_op_in_units",
            BadBinaryOpInUnits => "bad_binary_op_in_units",
            NoConstInUnits => "no_const_in_units",
            ExpectedInteger => "expected_integer",
            ExpectedIntegerOne => "expected_integer_one",
            DuplicateUnit => "duplicate_unit",
            ExpectedModule => "expected_module",
            ExpectedIdent => "expected_ident",
            UnitMismatch => "unit_mismatch",
            TodoWildcard => "todo_wildcard",
            TodoStarRange => "todo_star_range",
            TodoRange => "todo_range",
            TodoArrayBuiltin => "todo_array_builtin",
            CantSubscriptScalar => "cant_subscript_scalar",
            DimensionInScalarContext => "dimension_in_scalar_context",
            BadOverride => "bad_override",
            UnsupportedForSerialization => "unsupported_for_serialization",
            DuplicateMacroName => "duplicate_macro_name",
            LookupReferencedWithoutArgument => "lookup_referenced_without_argument",
            ConveyorWithoutOutflow => "conveyor_without_outflow",
            ConveyorNonEulerMethod => "conveyor_non_euler_method",
            ConveyorQueueUpstreamNotDiscrete => "conveyor_queue_upstream_not_discrete",
            ConveyorTransitNotPositive => "conveyor_transit_not_positive",
            ConveyorTransitTooLong => "conveyor_transit_too_long",
            ConveyorTransitNotDtMultiple => "conveyor_transit_not_dt_multiple",
            ConveyorLeakFractionsExceedOne => "conveyor_leak_fractions_exceed_one",
            ConveyorLtmDegraded => "conveyor_ltm_degraded",
            ConveyorDrivenFlowRead => "conveyor_driven_flow_read",
            ConveyorNotExpanded => "conveyor_not_expanded",
            ConveyorSpreadflowUnsupported => "conveyor_spreadflow_unsupported",
            ConveyorArrayedDimensionUnresolved => "conveyor_arrayed_dimension_unresolved",
            ConveyorContainerAccessUnsupported => "conveyor_container_access_unsupported",
            QueueNotExpanded => "queue_not_expanded",
            QueueNonEulerMethod => "queue_non_euler_method",
            QueueDrivenFlowRead => "queue_driven_flow_read",
            QueueOverflowNotOnQueue => "queue_overflow_not_on_queue",
            QueueLtmDegraded => "queue_ltm_degraded",
            ConveyorInSubmodelUnsupported => "conveyor_in_submodel_unsupported",
            QueueInSubmodelUnsupported => "queue_in_submodel_unsupported",
            QueueSecondaryOutflowToConveyor => "queue_secondary_outflow_to_conveyor",
            ConveyorMultipleNonLeakOutflows => "conveyor_multiple_non_leak_outflows",
            StockBothConveyorAndQueue => "stock_both_conveyor_and_queue",
            ConveyorInitListUnsupported => "conveyor_init_list_unsupported",
            UnknownElementSubscript => "unknown_element_subscript",
            MacroContainsModule => "macro_contains_module",
            UnfilledEquation => "unfilled_equation",
        };

        write!(f, "{name}")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct EquationError {
    pub start: u16,
    pub end: u16,
    pub code: ErrorCode,
    /// The human-readable reason, when the code and the span do not already
    /// carry it.
    ///
    /// `ErrorCode` names the CLASS of failure and `start..end` points at the
    /// offending text, which `errors::format_diagnostic_with_datamodel` renders as a
    /// source snippet -- so a parse error needs no reason: the snippet IS the
    /// reason. A site writes `details` when the reason is NOT visible in the
    /// span: the name that did not resolve, the arity a call missed, the
    /// identifier a lowering could not shape. Whatever is written here reaches
    /// the user unchanged, through `Diagnostic` -> `FormattedError::details`
    /// -> the FFI's `SimlinErrorDetail::details`.
    pub details: Option<String>,
}

impl EquationError {
    /// An error whose code and span say everything there is to say.
    pub fn new(code: ErrorCode, start: u16, end: u16) -> Self {
        EquationError {
            start,
            end,
            code,
            details: None,
        }
    }

    /// An error carrying the reason its raising site had in hand.
    pub fn detailed(code: ErrorCode, start: u16, end: u16, details: impl Into<String>) -> Self {
        EquationError {
            start,
            end,
            code,
            details: Some(details.into()),
        }
    }

    /// Add `context` to whatever reason this error already carries.
    ///
    /// Composing rather than replacing is what lets an ANNOTATING layer -- the
    /// unit-string parse, which tags every error out of one `<units>` string
    /// with that string -- run over a producer that may or may not have written
    /// its own reason, without either one silently winning.
    pub fn in_context(mut self, context: impl fmt::Display) -> Self {
        self.details = Some(match self.details {
            Some(reason) => format!("{reason} ({context})"),
            None => context.to_string(),
        });
        self
    }
}

impl fmt::Display for EquationError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self.details {
            Some(ref details) => {
                write!(f, "{}:{}:{} -- {details}", self.start, self.end, self.code)
            }
            None => write!(f, "{}:{}:{}", self.start, self.end, self.code),
        }
    }
}

impl From<Error> for EquationError {
    /// An `Error` has no span, so the result is span-less; its `details` is the
    /// whole reason `Error` carries one and rides across unchanged.
    fn from(err: Error) -> Self {
        EquationError {
            code: err.code,
            start: 0,
            end: 0,
            details: err.details,
        }
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub enum ErrorKind {
    Import,
    Model,
    Simulation,
    Variable,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Error {
    pub kind: ErrorKind,
    pub code: ErrorCode,
    pub details: Option<String>,
}

impl From<Box<dyn std::error::Error>> for Error {
    fn from(err: Box<dyn std::error::Error>) -> Self {
        Error {
            kind: ErrorKind::Simulation,
            code: ErrorCode::Generic,
            details: Some(err.to_string()),
        }
    }
}

impl Error {
    pub fn new(kind: ErrorKind, code: ErrorCode, details: Option<String>) -> Self {
        Error {
            kind,
            code,
            details,
        }
    }

    pub fn get_details(&self) -> Option<String> {
        self.details.clone()
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let kind = match self.kind {
            ErrorKind::Import => "ImportError",
            ErrorKind::Model => "ModelError",
            ErrorKind::Simulation => "SimulationError",
            ErrorKind::Variable => "VariableError",
        };
        match self.details {
            Some(ref details) => write!(f, "{}{{{}: {}}}", kind, self.code, details),
            None => write!(f, "{}{{{}}}", kind, self.code),
        }
    }
}

impl error::Error for Error {}

pub type Result<T> = result::Result<T, Error>;
pub type EquationResult<T> = result::Result<T, EquationError>;

/// Reserved sentinel standing in for a *literal period* inside a quoted
/// identifier in canonical form (U+2024 ONE DOT LEADER).
///
/// `canonicalize` must distinguish two uses of `.` in a raw identifier: the
/// module-hierarchy separator (`model.variable`), which maps to the middle
/// dot `·` (U+00B7), and a literal period that is part of a quoted name
/// (`"Goal 1.5 for Temperature"`). A literal period must NOT remain a raw
/// ASCII `.` in the canonical form: `is_canonical` rejects any `.`, so a
/// re-canonicalization pass would treat the now-unquoted period as a module
/// separator and corrupt the identity (issue #559 -- the corrupted
/// `goal_1·5_…` then splits into a phantom submodule and fails to
/// resolve with `DoesNotExist`). Mapping it to a dedicated canonical-stable
/// sentinel that is distinct from `·` makes `canonicalize` idempotent while
/// preserving the literal-vs-separator distinction. `to_source_repr` (via
/// `canonical_to_source`) maps it back to `.` so all user-facing/serialized
/// output is byte-identical to before.
const LITERAL_PERIOD_SENTINEL: char = '\u{2024}';
const LITERAL_PERIOD_SENTINEL_STR: &str = "\u{2024}";

/// Inverse of the period handling in [`canonicalize`]: map both the module
/// separator (`·`) and the literal-period sentinel back to `.` for source /
/// user-facing output. Borrows when neither is present (the common case).
fn canonical_to_source(s: &str) -> Cow<'_, str> {
    if s.contains('·') || s.contains(LITERAL_PERIOD_SENTINEL) {
        Cow::Owned(s.replace(['·', LITERAL_PERIOD_SENTINEL], "."))
    } else {
        Cow::Borrowed(s)
    }
}

/// Returns true if the string is already in canonical form, meaning no
/// transformations (trimming, lowercasing, quote stripping, period-to-middle-dot
/// conversion, whitespace-to-underscore, or backslash unescaping) would change it.
fn is_canonical(name: &str) -> bool {
    // Must not have leading/trailing whitespace
    let bytes = name.as_bytes();
    if !bytes.is_empty()
        && (bytes[0].is_ascii_whitespace() || bytes[bytes.len() - 1].is_ascii_whitespace())
    {
        return false;
    }

    // ASCII fast path: avoid char iteration and Unicode to_lowercase() entirely.
    // The vast majority of identifiers are pure ASCII, so this is the common case.
    if name.is_ascii() {
        let mut i = 0;
        while i < bytes.len() {
            let b = bytes[i];
            match b {
                b'"' | b'.' | b' ' | b'\n' | b'\r' | b'\t' => return false,
                b'\\' if i + 1 < bytes.len() => {
                    let next = bytes[i + 1];
                    if next == b'\\' || next == b'n' || next == b'r' {
                        return false;
                    }
                }
                b if b.is_ascii_uppercase() => return false,
                _ => {}
            }
            i += 1;
        }
        return true;
    }

    // Unicode slow path: handles non-ASCII characters like middle dots,
    // titlecase letters, etc.
    let mut chars = name.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' => return false,
            '.' => return false,
            ' ' | '\n' | '\r' | '\t' | '\u{00A0}' => return false,
            '\\' => {
                if let Some(&next) = chars.peek()
                    && (next == '\\' || next == 'n' || next == 'r')
                {
                    return false;
                }
            }
            // Reject any character that to_lowercase() would change.
            // This covers both uppercase (Lu) and titlecase (Lt) Unicode
            // categories -- titlecase letters like ǅ are NOT uppercase but
            // to_lowercase() still maps them to a different character (ǆ).
            c if changes_when_lowercased(c) => return false,
            _ => {}
        }
    }

    true
}

/// Canonicalize a variable/model name into a normalized form.
///
/// Returns `Cow::Borrowed` when the input is already canonical (avoiding
/// allocation), or `Cow::Owned` when transformations were needed.
///
/// Note: the borrowed slice may be a sub-slice of the input when there is
/// leading/trailing whitespace but the trimmed content is already canonical.
/// The returned `Cow` borrows from the input `&str` in all borrowed cases.
/// The engine's hash map for identifier-keyed lookups.
///
/// `FxHashMap`, not `std::collections::HashMap`. Identifier lookups are the
/// compiler's densest operation -- a name resolution per AST node, per element,
/// per fragment -- and SipHash over a short string was measured at 4-6% of a
/// large model's compile cycles. FxHash's fixed seed additionally makes
/// iteration order reproducible across processes, which is the direction this
/// crate already wants (GH #595): a salsa-cached value built by iterating a map
/// must not differ run to run.
///
/// The cost, stated because it is the reason this is not the default
/// everywhere: a fixed seed means an adversary who chooses the KEYS can force
/// collisions, and the keys here are variable names out of a model file. Every
/// engine entry point today compiles a model on behalf of the person who
/// supplied it -- the CLI, the local MCP and viewer servers, pysimlin, and the
/// browser's wasm bundle -- so a collision attack costs the attacker their own
/// compile. Do not extend this alias to a map keyed by input from a party other
/// than the one paying for the work.
pub(crate) type IdentMap<K, V> = std::collections::HashMap<K, V, rustc_hash::FxBuildHasher>;

/// Whether `to_lowercase` would change `c`, i.e. Unicode's
/// `Changes_When_Lowercased`.
///
/// Spelled as two `next()` calls rather than `c.to_lowercase().ne(once(c))`:
/// the iterator-comparison form goes through the generic `Iterator::eq_by`,
/// which does not collapse, and this predicate is on the identifier fast path.
///
/// The separators the ENGINE ITSELF mints are answered without consulting the
/// Unicode case tables. They are the reason this predicate is hot at all:
/// [`is_canonical_needing_no_trim`] decodes every non-ASCII byte and asks here,
/// and the module separator `·` appears in every `submodel·var` ident, so on
/// C-LEARN this was 342,138 `unicode_data::conversions::lookup` calls per
/// compile (~1.9% of it) to re-derive that a middle dot is not an uppercase
/// letter. Each listed character is verified against the table by
/// `engine_separators_are_lowercase_invariant`, so the shortcut is checked
/// rather than asserted, and anything not listed still takes the general path.
#[inline]
fn changes_when_lowercased(c: char) -> bool {
    if is_engine_separator(c) {
        return false;
    }
    let mut lower = c.to_lowercase();
    lower.next() != Some(c) || lower.next().is_some()
}

/// The non-ASCII characters the engine writes into identifiers itself: the
/// module-hierarchy separator, and the two LTM synthetic-name separators.
///
/// Listed here only as a fast path for [`changes_when_lowercased`]; membership
/// carries no meaning beyond "the case tables say this character is unchanged
/// by lowercasing, and it is common enough in our identifiers to be worth not
/// asking them".
#[inline]
fn is_engine_separator(c: char) -> bool {
    matches!(c, '\u{00B7}' | '\u{205A}' | '\u{2192}')
}

/// Per-byte "this byte alone cannot make a name non-canonical" table, the
/// fast path's whole decision.
///
/// `false` for every non-ASCII byte (the Unicode rules need `char`s), for the
/// characters [`is_canonical`] rejects outright, for ASCII uppercase, and for
/// the backslash -- which is excluded not because it is always wrong but
/// because deciding needs a lookahead, and a backslash in an identifier is
/// rare enough that paying the slower check for it is free.
///
/// A byte table rather than a 128-bit mask, which was measured and is worse on
/// both counts: the mask needs a range test and a variable shift per byte,
/// where the table is one load the scan can keep in flight.
static CANONICAL_BYTE: [bool; 256] = {
    let mut table = [false; 256];
    let mut b = 0usize;
    while b < 128 {
        table[b] = !matches!(
            b as u8,
            b'"' | b'.' | b' ' | b'\n' | b'\r' | b'\t' | b'\\' | b'A'..=b'Z'
        );
        b += 1;
    }
    table
};

/// Whether `name` is already canonical AND `str::trim` would not change it --
/// [`is_canonical`] composed with "needs no trimming", decided in one pass.
///
/// The point of handling non-ASCII here rather than bailing to
/// [`is_canonical`] is that a non-ASCII character costs one decode instead of
/// demoting the WHOLE string to that function's Unicode arm: LTM's synthetic
/// variable names are mostly ASCII with a handful of U+205A / U+2192
/// separators, and re-canonicalizing one used to case-check every character.
///
/// Conservative in one direction and never the other: a character this rejects
/// may still be canonical (any non-ASCII whitespace, say), in which case the
/// caller's `trim` + [`is_canonical`] pair answers exactly as it always did --
/// only slower. Nothing it ACCEPTS may be non-canonical.
fn is_canonical_needing_no_trim(name: &str) -> bool {
    let bytes = name.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if CANONICAL_BYTE[b as usize] {
            i += 1;
            continue;
        }
        if b < 0x80 {
            // An ASCII byte the table rejects: uppercase, a character
            // `is_canonical` rejects outright, or a backslash (whose verdict
            // needs a lookahead this scan deliberately does not take).
            return false;
        }
        // Non-ASCII. Only two of `is_canonical`'s Unicode rules can apply --
        // every character it rejects by value is ASCII apart from U+00A0, and
        // `char::is_whitespace` covers that one. Rejecting EVERY Unicode
        // whitespace (not just U+00A0) is what makes "needs no trimming" sound:
        // `str::trim` strips the whole White_Space property.
        let Some(c) = name[i..].chars().next() else {
            return false;
        };
        if c.is_whitespace() || changes_when_lowercased(c) {
            return false;
        }
        i += c.len_utf8();
    }
    true
}

pub fn canonicalize(name: &str) -> Cow<'_, str> {
    // Fastest path: one scan proving the name is already canonical AND needs
    // no trimming. This is the overwhelmingly common case -- every already-canonical
    // identifier the compiler re-canonicalizes on its way through a map lookup
    // or an AST lowering lands here -- and it replaces a Unicode `trim`, an
    // `is_ascii` scan, and `is_canonical`'s own scan with a single pass.
    if is_canonical_needing_no_trim(name) {
        return Cow::Borrowed(name);
    }

    // Fast path: if the name is already trimmed and canonical, avoid allocation.
    let trimmed = name.trim();
    if is_canonical(trimmed) {
        // Return the trimmed slice (which may equal the original if there was
        // no leading/trailing whitespace).
        return Cow::Borrowed(trimmed);
    }

    // Slow path: four rewrites per identifier part -- period mapping,
    // doubled-backslash unescaping, whitespace collapse, lowercasing. Each is
    // guarded by a cheap "is there anything to do" test and skipped by
    // borrowing when there is not, and the last two are fused for an ASCII part
    // so they write straight into the output. Spelling them as four
    // unconditional `String`-returning steps cost four allocations and a
    // two-way substring searcher per part even for a name whose only defect was
    // its capitalization -- and this is the single hottest function in a large
    // model's compile, reached once per identifier occurrence.
    let mut canonicalized_name = String::with_capacity(trimmed.len());

    for part in IdentifierPartIterator::new(trimmed) {
        let bytes = part.as_bytes();
        let quoted: bool =
            { bytes.len() >= 2 && bytes[0] == b'"' && bytes[bytes.len() - 1] == b'"' };

        let part = if quoted {
            let inner = &part[1..bytes.len() - 1];
            if inner.contains('.') {
                // Literal period inside a quoted identifier. Map it to the
                // canonical-stable sentinel rather than leaving a raw `.`:
                // a raw `.` is rejected by `is_canonical`, so a re-canonical
                // pass would treat the now-unquoted period as the `·`
                // module separator and corrupt the identity (#559).
                // `canonical_to_source` reverses this back to `.`.
                Cow::Owned(inner.replace('.', LITERAL_PERIOD_SENTINEL_STR))
            } else {
                Cow::Borrowed(inner)
            }
        } else if part.contains('.') {
            // Replace periods with middle dots (·) for module hierarchy separators.
            // This allows us to distinguish between:
            // - Module separators: model.variable -> model·variable
            // - Literal periods in quoted names: "a.b" -> a<U+2024>b
            Cow::Owned(part.replace('.', "·"))
        } else {
            Cow::Borrowed(part)
        };

        // Unescape doubled backslashes. Guarded on a single-character search
        // (which is memchr) rather than on the two-character pattern, whose
        // searcher setup costs more than the scan: no backslash at all implies
        // no doubled one, and a part holding a lone backslash is rare enough
        // that letting it fall through to a no-op `replace` is free.
        let part = if part.contains('\\') {
            Cow::Owned(part.replace("\\\\", "\\"))
        } else {
            part
        };

        push_whitespace_folded_lowercase(&mut canonicalized_name, &part);
    }

    Cow::Owned(canonicalized_name)
}

/// Append `part` to `out` with the last two canonicalization steps applied:
/// [`replace_whitespace_with_underscore`], then `to_lowercase`.
///
/// The ASCII arm fuses the two into one pass that writes directly into `out`.
/// That is sound for exactly two reasons, both of which fail outside ASCII:
/// ASCII lowercasing is per-byte and context-free (`str::to_lowercase` is
/// context-sensitive in general -- Greek capital sigma lowercases differently
/// at the end of a word), and the only non-ASCII character the whitespace pass
/// recognizes, U+00A0, cannot occur. The non-ASCII arm therefore keeps the two
/// steps separate and unchanged, and skips `to_lowercase` when no character
/// would change -- the same predicate `is_canonical` uses.
fn push_whitespace_folded_lowercase(out: &mut String, part: &str) {
    if part.is_ascii() {
        let bytes = part.as_bytes();
        let mut in_whitespace = false;
        let mut i = 0;
        while i < bytes.len() {
            let b = bytes[i];
            // A literal `\n` / `\r` escape (two characters) counts as
            // whitespace; any other backslash passes through.
            if b == b'\\' && matches!(bytes.get(i + 1), Some(b'n' | b'r')) {
                i += 2;
                if !in_whitespace {
                    out.push('_');
                    in_whitespace = true;
                }
            } else if matches!(b, b'\n' | b'\r' | b'\t' | b' ') {
                i += 1;
                if !in_whitespace {
                    out.push('_');
                    in_whitespace = true;
                }
            } else {
                i += 1;
                in_whitespace = false;
                out.push(char::from(b.to_ascii_lowercase()));
            }
        }
        return;
    }

    let replaced = replace_whitespace_with_underscore(part);
    if replaced
        .chars()
        .any(|c| c.to_lowercase().ne(std::iter::once(c)))
    {
        out.push_str(&replaced.to_lowercase());
    } else {
        out.push_str(&replaced);
    }
}

/// Group a variable-ident list by canonical form and return the colliding
/// groups: for each canonical ident declared more than once, the canonical
/// form plus every as-written spelling, in declaration order (GH #885).
///
/// [`canonicalize`] collapses case, whitespace, and underscores, so
/// `Attrition`/`attrition` or `net flow`/`net_flow` are the SAME variable
/// identifier -- every canonical-keyed map downstream (salsa sync and parsed
/// lowering scopes) silently keeps only one such twin. Callers use the groups to
/// reject the model loudly instead. Group order follows the first occurrence
/// of each colliding canonical so diagnostics are deterministic for a given
/// declaration order.
pub(crate) fn duplicate_variable_groups<'a, I>(idents: I) -> Vec<(String, Vec<String>)>
where
    I: IntoIterator<Item = &'a str>,
{
    let mut order: Vec<String> = Vec::new();
    let mut groups: HashMap<String, Vec<String>> = HashMap::new();
    for ident in idents {
        let canonical = canonicalize(ident).into_owned();
        let entry = groups.entry(canonical.clone()).or_default();
        if entry.is_empty() {
            order.push(canonical);
        }
        entry.push(ident.to_string());
    }
    order
        .into_iter()
        .filter_map(|canonical| {
            let spellings = groups.remove(&canonical)?;
            (spellings.len() > 1).then_some((canonical, spellings))
        })
        .collect()
}

/// The user-facing message for one duplicate-canonical-ident group, shared by
/// the hard compile error (`compile_project_incremental`,
/// `queue_compile::build_compiled`) and the accumulated diagnostic
/// (`model_all_diagnostics`) so every surface reports identical text.
pub(crate) fn duplicate_variable_message(
    model_name: &str,
    canonical: &str,
    spellings: &[String],
) -> String {
    let list = spellings
        .iter()
        .map(|s| format!("'{s}'"))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "variables {list} in model '{model_name}' all canonicalize to the same identifier \
         '{canonical}' (variable names are case-, whitespace-, and underscore-insensitive); \
         simulating would silently keep only one of them, so rename them to be distinct"
    )
}

#[test]
fn test_duplicate_variable_groups() {
    // No collisions: distinct canonicals yield no groups.
    assert!(duplicate_variable_groups(["a", "b", "c"]).is_empty());
    assert!(duplicate_variable_groups([]).is_empty());

    // Case, whitespace, and underscore variants collide; spellings are
    // reported in declaration order.
    let groups = duplicate_variable_groups(["Attrition", "x", "attrition"]);
    assert_eq!(
        groups,
        vec![(
            "attrition".to_string(),
            vec!["Attrition".to_string(), "attrition".to_string()]
        )]
    );
    let groups = duplicate_variable_groups(["net flow", "net_flow"]);
    assert_eq!(
        groups,
        vec![(
            "net_flow".to_string(),
            vec!["net flow".to_string(), "net_flow".to_string()]
        )]
    );

    // Byte-identical twins (the shape an MDL space/underscore pair imports
    // as) are also a collision.
    let groups = duplicate_variable_groups(["net_flow", "net_flow"]);
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].1.len(), 2);

    // Multiple groups keep first-occurrence order; a three-way collision is
    // one group with all three spellings.
    let groups = duplicate_variable_groups(["B b", "a", "A", "b_B", "B_B"]);
    assert_eq!(
        groups,
        vec![
            (
                "b_b".to_string(),
                vec!["B b".to_string(), "b_B".to_string(), "B_B".to_string()]
            ),
            ("a".to_string(), vec!["a".to_string(), "A".to_string()]),
        ]
    );
}

#[test]
fn test_canonicalize() {
    // A literal period inside a quoted identifier canonicalizes to the
    // reserved sentinel (U+2024), NOT a raw `.` -- so the result is itself
    // canonical and re-canonicalization is a no-op (#559). The module
    // separator (unquoted `.`) still maps to `·` (line below). Every other
    // assertion in this test is byte-unchanged by the sentinel fix.
    assert_eq!("a\u{2024}b", &*canonicalize("\"a.b\""));
    assert_eq!("a/d·b_\\\"c\\\"", &*canonicalize("\"a/d\".\"b \\\"c\\\"\""));
    assert_eq!("a/d·b_c", &*canonicalize("\"a/d\".\"b c\""));
    assert_eq!("a·b_c", &*canonicalize("a.\"b c\""));
    assert_eq!("a/d·b", &*canonicalize("\"a/d\".b"));
    assert_eq!("quoted", &*canonicalize("\"quoted\""));
    assert_eq!("a_b", &*canonicalize("   a b"));
    assert_eq!("å_b", &*canonicalize("Å\nb"));
    assert_eq!("a_b", &*canonicalize("a \n b"));
    assert_eq!("a·b", &*canonicalize("a.b"));
}

/// Regression for issue #559: a Vensim quoted identifier containing a
/// literal period (e.g. C-LEARN's `"Goal 1.5 for Temperature"`) must
/// canonicalize *idempotently*.
///
/// Before the fix the first pass strips the quotes and keeps the raw `.`
/// (`"a.b"` -> `a.b`), but `is_canonical("a.b")` returns false (it rejects
/// any `.`), so a downstream "ensure canonical" re-pass treats the now
/// unquoted `a.b` as a module path and mis-converts the literal period into
/// the U+00B7 module-hierarchy separator (`a·b`). The variable's own
/// identity then splits at `·` into a phantom submodule and resolution
/// fails with `DoesNotExist`. The invariant
/// `canonicalize(canonicalize(x)) == canonicalize(x)` must hold for ALL
/// inputs, quoted-period names included.
#[test]
fn test_canonicalize_idempotent_quoted_period() {
    for raw in [
        "\"a.b\"",
        "\"a.b c\"",
        "\"Goal 1.5 for Temperature\"",
        "\"goal_1.5_for_temperature\"",
        "\"Fig. 3\"",
        "\"v1.2 target\"",
    ] {
        let once = canonicalize(raw).into_owned();
        let twice = canonicalize(&once).into_owned();
        assert_eq!(
            twice, once,
            "canonicalize not idempotent for {raw:?}: once={once:?}, twice={twice:?}"
        );
        // The corrupting outcome specifically: no raw `.` survives, and the
        // literal period did NOT become the `·` module separator.
        assert!(
            !once.contains('.'),
            "canonical form of {raw:?} still has a raw `.`: {once:?}"
        );
        assert!(
            !once.contains('·'),
            "literal period in {raw:?} was mis-mapped to the `·` module \
             separator: {once:?}"
        );
        // And it round-trips back to a literal `.` for source/display
        // output, so user-facing output is unchanged by the fix.
        let source = canonical_to_source(&once);
        assert!(
            source.contains('.') && !source.contains('·'),
            "source repr of {raw:?} should restore the literal `.`: {source:?}"
        );
    }
}

/// The canonicalize change must ONLY affect identifiers with a literal
/// period inside quotes (#559). Every other input class -- plain idents,
/// the `·` module separator, the `⁚` synthetic separator, unicode,
/// quoted-without-period -- must be byte-for-byte identical to the
/// pre-fix behavior, and the sentinel must never appear in their
/// canonical form. The expected values here are exactly the pre-fix
/// `test_canonicalize` expectations.
#[test]
fn test_canonicalize_non_period_idents_byte_unchanged() {
    let cases: &[(&str, &str)] = &[
        ("hello_world", "hello_world"),
        ("Population", "population"),
        ("a b c", "a_b_c"),
        // Unquoted period = module-hierarchy separator -> `·` (unchanged).
        ("a.b", "a·b"),
        ("model.variable", "model·variable"),
        // `.` between two quoted parts is still a module separator.
        ("\"a/d\".\"b c\"", "a/d·b_c"),
        ("\"a/d\".b", "a/d·b"),
        ("a.\"b c\"", "a·b_c"),
        // Quoted, but NO literal period -> just quote-stripped.
        ("\"quoted\"", "quoted"),
        ("\"b c\"", "b_c"),
        // Synthetic separators and unicode are untouched.
        ("stdlib⁚smth1", "stdlib⁚smth1"),
        ("model·variable", "model·variable"),
        ("café", "café"),
        ("Å\nb", "å_b"),
    ];
    for (raw, expected) in cases {
        let got = canonicalize(raw).into_owned();
        assert_eq!(
            &got, expected,
            "canonicalize({raw:?}) changed: got {got:?}, expected {expected:?}"
        );
        assert!(
            !got.contains(LITERAL_PERIOD_SENTINEL),
            "sentinel leaked into a non-literal-period ident {raw:?}: {got:?}"
        );
        // Idempotent for these too (the invariant is universal).
        assert_eq!(canonicalize(&got).into_owned(), got);
    }
}

#[test]
fn test_canonicalize_returns_borrowed_when_already_canonical() {
    // Already-canonical strings should return Cow::Borrowed
    assert!(matches!(canonicalize("hello_world"), Cow::Borrowed(_)));
    assert!(matches!(canonicalize("population"), Cow::Borrowed(_)));
    assert!(matches!(canonicalize("a_b_c"), Cow::Borrowed(_)));
    assert!(matches!(canonicalize("stdlib⁚smth1"), Cow::Borrowed(_)));
    assert!(matches!(canonicalize("model·variable"), Cow::Borrowed(_)));
    assert!(matches!(canonicalize(""), Cow::Borrowed(_)));

    // Strings with only leading/trailing whitespace still borrow the
    // trimmed slice when the trimmed content is canonical.
    assert!(matches!(canonicalize("  trimmed  "), Cow::Borrowed(_)));

    // The literal-period sentinel form is itself canonical -> Borrowed.
    // This is the idempotency fast path the sentinel mapping relies on.
    assert!(matches!(canonicalize("a\u{2024}b"), Cow::Borrowed(_)));
    assert!(matches!(
        canonicalize("goal_1\u{2024}5_for_temperature"),
        Cow::Borrowed(_)
    ));

    // Non-canonical strings should return Cow::Owned
    assert!(matches!(canonicalize("Hello"), Cow::Owned(_)));
    assert!(matches!(canonicalize("a.b"), Cow::Owned(_)));
    assert!(matches!(canonicalize("a b"), Cow::Owned(_)));
    assert!(matches!(canonicalize("\"quoted\""), Cow::Owned(_)));
    // A quoted-period ident takes the slow path -> Owned (it then maps to
    // the sentinel form asserted Borrowed above).
    assert!(matches!(canonicalize("\"a.b\""), Cow::Owned(_)));
}

#[test]
fn test_is_canonical() {
    assert!(is_canonical("hello_world"));
    assert!(is_canonical("population"));
    assert!(is_canonical("model·variable"));
    assert!(is_canonical("stdlib⁚smth1"));
    assert!(is_canonical(""));
    assert!(is_canonical("a_b_c_123"));
    // The literal-period sentinel (U+2024) is a canonical character: this
    // is precisely why canonicalize is idempotent for quoted-period idents
    // (#559). Contrast with the raw `.` rejection asserted below.
    assert!(is_canonical("a\u{2024}b"));
    assert!(is_canonical("goal_1\u{2024}5_for_temperature"));

    assert!(!is_canonical("Hello"));
    assert!(!is_canonical("a.b"));
    assert!(!is_canonical("a b"));
    assert!(!is_canonical("\"quoted\""));
    assert!(!is_canonical("has\\\\escape"));
    assert!(!is_canonical(" leading"));
    assert!(!is_canonical("trailing "));
    assert!(!is_canonical("a\tb"));
    assert!(!is_canonical("\ttab"));
}

#[test]
fn test_is_canonical_ascii_fast_path() {
    // Pure ASCII canonical names -- hit the byte-level fast path
    assert!(is_canonical("x"));
    assert!(is_canonical("abc_def_123"));
    assert!(is_canonical("rate"));
    assert!(is_canonical("a\\b")); // single backslash not followed by \, n, or r

    // Pure ASCII non-canonical -- fast path must still reject
    assert!(!is_canonical("ABC"));
    assert!(!is_canonical("camelCase"));
    assert!(!is_canonical("a.b"));
    assert!(!is_canonical("\"q\""));
    assert!(!is_canonical("a\\\\b"));
    assert!(!is_canonical("a\\nb"));
    assert!(!is_canonical("a\\rb"));
    assert!(!is_canonical("a b"));
    assert!(!is_canonical("a\tb"));
    assert!(!is_canonical("a\nb"));
    assert!(!is_canonical("a\rb"));
}

#[test]
fn test_is_canonical_unicode_slow_path() {
    // Non-ASCII canonical names -- must fall through to the Unicode path
    assert!(is_canonical("café"));
    assert!(is_canonical("naïve"));
    assert!(is_canonical("model·variable"));

    // Non-ASCII with uppercase Unicode -- Unicode path must reject
    assert!(!is_canonical("Ünter"));
    // Titlecase letter (not uppercase, but to_lowercase changes it)
    assert!(!is_canonical("ǅ"));
    // NBSP triggers whitespace rejection
    assert!(!is_canonical("a\u{00A0}b"));
}

#[test]
fn test_canonicalize_tab_handling() {
    // Tabs should be treated as whitespace and replaced with underscores,
    // matching the behavior for spaces, newlines, etc.
    assert_eq!("a_b", &*canonicalize("a\tb"));
    assert_eq!("a_b_c", &*canonicalize("a\t\tb\tc"));
    assert!(matches!(canonicalize("a\tb"), Cow::Owned(_)));
    // Leading/trailing tabs are stripped by trim()
    assert_eq!("tab", &*canonicalize("\ttab\t"));
}

/// Verify that `is_canonical` and the full canonicalization slow path agree:
/// when `is_canonical` returns true, the slow path must produce the same string.
#[cfg(test)]
mod canonicalize_invariant_tests {
    use super::*;
    use proptest::prelude::*;

    /// Force the slow path of canonicalize by bypassing the is_canonical check.
    fn canonicalize_slow_path(name: &str) -> String {
        let trimmed = name.trim();
        let mut result = String::with_capacity(trimmed.len());
        for part in super::IdentifierPartIterator::new(trimmed) {
            let bytes = part.as_bytes();
            let quoted = bytes.len() >= 2 && bytes[0] == b'"' && bytes[bytes.len() - 1] == b'"';
            let part = if quoted {
                let inner = &part[1..bytes.len() - 1];
                if inner.contains('.') {
                    Cow::Owned(inner.replace('.', super::LITERAL_PERIOD_SENTINEL_STR))
                } else {
                    Cow::Borrowed(inner)
                }
            } else {
                Cow::Owned(part.replace('.', "\u{00B7}"))
            };
            let part = part.replace("\\\\", "\\");
            let part = super::replace_whitespace_with_underscore(&part);
            let part = part.to_lowercase();
            result.push_str(&part);
        }
        result
    }

    /// The alphabet that actually reaches every branch of the slow path.
    ///
    /// `\PC{0,100}` alone does not: the branches are selected by quotes,
    /// periods, backslashes, the two literal escapes, whitespace runs, and
    /// characters whose lowercasing is context-sensitive or non-ASCII, and a
    /// generator over all non-control characters produces those roughly never.
    /// Two of these deserve naming. `Σ` is the one character for which
    /// `str::to_lowercase` is context-sensitive (it lowercases to `ς` at the
    /// end of a word and `σ` elsewhere), which is why the fused rewrite is
    /// restricted to ASCII. `\u{00A0}` is the one non-ASCII character
    /// `replace_whitespace_with_underscore` treats as whitespace, and `str::
    /// trim` also strips it, so it exercises both the trim and the fold.
    const SLOW_PATH_ALPHABET: &[&str] = &[
        "a", "Z", "_", "0", ".", " ", "\t", "\n", "\r", "\\", "\"", "·", "\u{2024}", "\u{00A0}",
        "Σ", "σ", "ς", "É", "ǅ", "İ",
    ];

    proptest! {
        #[test]
        fn fast_path_agrees_with_slow_path(s in "\\PC{0,100}") {
            let cow = canonicalize(&s);
            let slow = canonicalize_slow_path(&s);
            // The Cow result must always equal the slow path result
            prop_assert_eq!(&*cow, &*slow,
                "canonicalize fast/slow path mismatch for {:?}", s);
            // When Cow::Borrowed, it must equal the trimmed input
            if let Cow::Borrowed(b) = &cow {
                prop_assert_eq!(*b, s.trim(),
                    "Borrowed result should equal trimmed input for {:?}", s);
            }
        }

        /// The same differential check, driven by [`SLOW_PATH_ALPHABET`] so the
        /// slow path's branches are reached rather than hoped for. Named
        /// separately from the broad-alphabet property because the two catch
        /// different things: that one covers the input space, this one covers
        /// the code.
        #[test]
        fn canonicalize_agrees_with_the_unfused_reference(
            pieces in proptest::collection::vec(
                proptest::sample::select(SLOW_PATH_ALPHABET), 0..24)
        ) {
            let s: String = pieces.concat();
            prop_assert_eq!(&*canonicalize(&s), &*canonicalize_slow_path(&s),
                "canonicalize disagrees with the unfused reference for {:?}", s);
        }
    }

    /// A canonical name is returned BORROWED, non-ASCII separators included.
    ///
    /// The engine's own generated identifiers -- LTM link/loop scores, the
    /// module separator, the literal-period sentinel -- are mostly ASCII with a
    /// few non-ASCII separators, and they are re-canonicalized constantly on
    /// their way through map lookups and AST lowerings. Answering "already
    /// canonical" for them without allocating is the point of the fused scan,
    /// and it is invisible in a correctness test: demoting them to the slow
    /// path returns an EQUAL `Cow::Owned`, so only the discriminant shows it.
    #[test]
    fn engine_generated_names_canonicalize_without_allocating() {
        let names = [
            "population",
            "net_flow_2",
            "model\u{00b7}variable",
            "goal_1\u{2024}5_for_temperature",
            "$\u{205a}ltm\u{205a}link_score\u{205a}food\u{2192}population",
            "$\u{205a}ltm\u{205a}loop_score\u{205a}r1",
            "$\u{205a}ltm\u{205a}agg\u{205a}3",
            "stdlib\u{205a}smth1",
        ];
        for name in names {
            assert!(
                matches!(canonicalize(name), Cow::Borrowed(_)),
                "{name:?} is canonical and must not be re-built"
            );
            assert_eq!(&*canonicalize(name), &*canonicalize_slow_path(name));
        }
    }

    /// `changes_when_lowercased` short-circuits the characters the engine
    /// mints into identifiers itself. The shortcut is only sound because the
    /// Unicode case tables agree, so ask them here rather than asserting it:
    /// this is the test that reds if a future separator is added to
    /// `is_engine_separator` that lowercasing DOES change.
    ///
    /// Checked against the general path (`c.to_lowercase()`) rather than
    /// against a hardcoded `false`, which would restate the shortcut instead
    /// of verifying it.
    #[test]
    fn engine_separators_are_lowercase_invariant() {
        for c in ['\u{00B7}', '\u{205A}', '\u{2192}'] {
            assert!(
                is_engine_separator(c),
                "{c:?} must be on the fast path for this test to be checking it"
            );
            let mut lower = c.to_lowercase();
            assert_eq!(
                lower.next(),
                Some(c),
                "{c:?} lowercases to something else; the fast path in \
                 changes_when_lowercased is unsound for it"
            );
            assert_eq!(
                lower.next(),
                None,
                "{c:?} lowercases to more than one character; the fast path in \
                 changes_when_lowercased is unsound for it"
            );
            assert!(!changes_when_lowercased(c));
        }
    }

    /// Hand-written cases for the interactions the fused ASCII rewrite has to
    /// get right, each of which composes two steps whose order matters.
    #[test]
    fn fused_ascii_fold_matches_the_reference_on_step_interactions() {
        let cases = [
            // Doubled backslash collapses BEFORE the escape scan sees it, so
            // `\\n` is a backslash followed by `n`, i.e. an escape.
            "A\\\\nB",
            // ...whereas a single backslash before `n` is already the escape,
            // and a lone trailing backslash passes through.
            "A\\nB",
            "A\\",
            // Whitespace runs (mixed real and escaped) collapse to one `_`.
            "A \t\nB",
            "A\\n\\rB",
            "A \\n B",
            // A backslash that is not an escape resets the run, so the
            // whitespace on either side of it yields two underscores.
            "A \\ B",
            // Quoted parts keep their interior spacing rules but lose the
            // quotes, and a period inside them is the literal-period sentinel.
            "\"A B\".C",
            "\"a.b\"",
            // Mixed quoted/unquoted parts in one identifier.
            "Mod.\"Var Name\".Sub",
            // Leading/trailing whitespace is trimmed before anything else.
            "  Net Flow  ",
            // A part that is purely whitespace.
            "A. .B",
        ];
        for case in cases {
            assert_eq!(
                &*canonicalize(case),
                &*canonicalize_slow_path(case),
                "canonicalize disagrees with the unfused reference for {case:?}"
            );
        }
    }

    #[test]
    fn titlecase_letters_are_lowered() {
        // Unicode titlecase letters (General Category Lt) are not uppercase
        // but to_lowercase() still changes them (e.g. ǅ -> ǆ).
        // is_canonical must reject them so the slow path can lower them.
        let titlecase_inputs = [
            "\u{01C5}", // ǅ -> ǆ
            "\u{01C8}", // ǈ -> ǉ
            "\u{01CB}", // ǋ -> ǌ
            "\u{01F2}", // ǲ -> ǳ
        ];
        for input in titlecase_inputs {
            let result = canonicalize(input);
            let slow = canonicalize_slow_path(input);
            assert_eq!(
                &*result, &*slow,
                "titlecase mismatch for {:?}: fast={:?}, slow={:?}",
                input, result, slow
            );
            // The slow path should have lowered it, so the result should differ
            // from the input.
            assert_ne!(
                &*result, input,
                "titlecase char {:?} should be lowered",
                input
            );
        }
    }
}

#[test]
fn test_canonical_ident() {
    // Test canonicalization from raw
    let raw = RawIdent::new("Hello World".to_string());
    let canonical = raw.canonicalize();
    assert_eq!(canonical.as_str(), "hello_world");

    // Test direct creation with Ident::new
    let canonical2 = Ident::new("Hello World");
    assert_eq!(canonical.as_str(), canonical2.as_str());

    // Test to_source_repr with Ident::new
    let canonical3 = Ident::new("a.b");
    assert_eq!(canonical3.as_str(), "a·b");
    assert_eq!(canonical3.to_source_repr(), "a.b");

    // Test conversion to String (using Display trait)
    let legacy: String = canonical.to_string();
    assert_eq!(legacy, "hello_world");
}

#[test]
fn test_canonical_dimension_name() {
    let raw = RawDimensionName::new("Time Units".to_string());
    let canonical = raw.canonicalize();
    assert_eq!(canonical.as_str(), "time_units");

    let canonical2 = CanonicalDimensionName::from_raw("Time Units");
    assert_eq!(canonical, canonical2);
}

#[test]
fn test_canonical_element_name() {
    let raw = RawElementName::new("Element Name".to_string());
    let canonical = raw.canonicalize();
    assert_eq!(canonical.as_str(), "element_name");

    let canonical2 = CanonicalElementName::from_raw("Element Name");
    assert_eq!(canonical, canonical2);
}

#[test]
fn test_canonical_ident_with_dots() {
    // Dots OUTSIDE quotes are module-hierarchy separators -> `·`.
    assert_eq!("a·d", &*canonicalize("a.d"));

    // A literal period INSIDE a quoted identifier maps to the reserved
    // sentinel (U+2024), NOT a raw `.` (which would be re-canonicalized
    // into the `·` module separator -- #559). It reverses to `.`
    // via to_source_repr, so user-facing output is byte-unchanged.
    assert_eq!("a\u{2024}d", &*canonicalize("\"a.d\""));
    assert_eq!(Ident::<Canonical>::new("\"a.d\"").to_source_repr(), "a.d");

    // Mixed: unquoted `.` -> `·`, quoted literal `.` -> sentinel.
    assert_eq!("a·b\u{2024}c", &*canonicalize("a.\"b.c\""));
}

#[test]
fn test_ident_join_operation() {
    // Test joining two canonical identifiers
    let module = CanonicalStr::from_canonical_unchecked("model");
    let var = CanonicalStr::from_canonical_unchecked("variable");
    let joined = Ident::<Canonical>::join(&module, &var);
    assert_eq!(joined.as_str(), "model·variable");
    assert_eq!(joined.to_source_repr(), "model.variable");
}

#[test]
fn test_canonical_str_operations() {
    let canonical = Ident::new("module.sub.variable");
    let canonical_str = canonical.as_canonical_str();

    // Test split_at_dot
    if let Some((before, after)) = canonical_str.split_at_dot() {
        assert_eq!(before.as_str(), "module");
        assert_eq!(after.as_str(), "sub·variable");

        // Test nested split on the after part
        if let Some((first, rest)) = after.split_at_dot() {
            assert_eq!(first.as_str(), "sub");
            assert_eq!(rest.as_str(), "variable");
        } else {
            panic!("Expected successful nested split");
        }
    } else {
        panic!("Expected successful split");
    }

    // Test with no dots
    let no_dots = Ident::new("simple");
    assert!(no_dots.as_canonical_str().split_at_dot().is_none());
}

#[test]
fn test_canonical_str_strip_prefix() {
    let ident = Ident::new("stdlib⁚smooth");
    let canonical_str = ident.as_canonical_str();

    if let Some(stripped) = canonical_str.strip_prefix("stdlib⁚") {
        assert_eq!(stripped.as_str(), "smooth");
    } else {
        panic!("Expected successful prefix strip");
    }

    // Test that stripped result maintains canonical form
    let ident2 = Ident::new("model.Sub Module");
    let canonical_str2 = ident2.as_canonical_str();
    if let Some(stripped) = canonical_str2.strip_prefix("model·") {
        assert_eq!(stripped.as_str(), "sub_module");
    } else {
        panic!("Expected successful prefix strip");
    }
}

#[test]
fn test_canonical_str_utility_methods() {
    let ident = Ident::new("model.variable");
    let canonical_str = ident.as_canonical_str();

    // Test starts_with
    assert!(canonical_str.starts_with("model·"));
    assert!(!canonical_str.starts_with("other·"));

    assert_eq!(canonical_str.as_str(), "model·variable");

    // `find` reports BYTE offsets, and the module separator U+00B7 is 2 bytes
    // in UTF-8: it sits at 5, so the variable half starts at 7, not at 6.
    assert_eq!(canonical_str.find("·"), Some(5));
    assert_eq!(canonical_str.find("variable"), Some(7));
    assert_eq!(canonical_str.find("notfound"), None);
}

#[test]
fn test_display_format_edge_cases() {
    // Test empty string
    let empty = canonicalize("");
    assert_eq!(&*empty, "");

    // Test string with only spaces
    let spaces = canonicalize("   ");
    assert_eq!(&*spaces, "");

    // Mixed dots and quotes: unquoted `.` -> `·` (module separator),
    // quoted literal `.` -> the reserved sentinel (#559).
    let complex = canonicalize("a.\"b.c\".d");
    assert_eq!(&*complex, "a·b\u{2024}c·d");
}

#[test]
fn test_unchecked_constructors() {
    // Test unchecked construction of Ident
    let canonical_string = "already_canonical".to_string();
    let ident = Ident::<Canonical>::from_unchecked(canonical_string.clone());
    assert_eq!(ident.as_str(), "already_canonical");

    // Test unchecked construction of CanonicalStr
    let canonical_slice = CanonicalStr::from_canonical_unchecked("canonical·str");
    assert_eq!(canonical_slice.as_str(), "canonical·str");
}

#[test]
fn test_fmt_display_implementations() {
    let ident = Ident::new("Model.Var");
    assert_eq!(format!("{ident}"), "model·var");

    let canonical_str = ident.as_canonical_str();
    assert_eq!(format!("{canonical_str}"), "model·var");
}

/// Tests for the interned storage backing the canonical identifier newtypes
/// (`Ident<Canonical>`, `CanonicalElementName`, `CanonicalDimensionName`).
///
/// The behavioral contract these pin down:
/// - lexicographic `Ord`/`PartialOrd` (determinism of runlists / `BTreeSet`
///   ordering), which must equal sorting the equivalent `&str`s even though
///   the interned handle's natural `Ord` could be pointer-based;
/// - value equality + de-duplication: two values built from strings that
///   canonicalize to the same form are `==`, hash equal, AND share one
///   backing allocation (the whole point of interning);
/// - `Clone` is a cheap handle copy that shares the backing allocation rather
///   than re-allocating a `String`;
/// - HashMap lookups keyed by these types still work both by value and via
///   the `Borrow<str>` path, which requires the manual `Hash` to be value
///   based (consistent with `str`'s `Hash`), not pointer based;
/// - the canonicalization edge cases (idempotency, the `LITERAL_PERIOD_SENTINEL`
///   quoted-period idents) and source round-trip continue to hold.
#[cfg(test)]
mod interned_identifier_tests {
    use super::*;
    use std::collections::{BTreeSet, HashMap};

    /// The data pointer behind a canonical `&str`. Two interned handles that
    /// dedup to the same string share this pointer; two independent `String`
    /// allocations of the same content do not.
    fn data_ptr(s: &str) -> *const u8 {
        s.as_ptr()
    }

    // ----- Constraint 2: lexicographic Ord / PartialOrd -----

    #[test]
    fn ident_sort_order_matches_str_sort_order() {
        // Deliberately includes the `·` module separator and unicode so the
        // ordering exercises more than ASCII; the canonical forms are stable.
        let raws = [
            "zebra",
            "apple",
            "model·variable",
            "model·alpha",
            "café",
            "a_b_c",
            "Apple", // canonicalizes to "apple" -> dedups with index 1
            "MODEL·Variable",
        ];
        let idents: Vec<Ident<Canonical>> = raws.iter().map(|s| Ident::new(s)).collect();

        let mut by_ident = idents.clone();
        by_ident.sort();

        let mut by_str: Vec<Ident<Canonical>> = idents.clone();
        by_str.sort_by(|a, b| a.as_str().cmp(b.as_str()));

        let ident_order: Vec<&str> = by_ident.iter().map(|i| i.as_str()).collect();
        let str_order: Vec<&str> = by_str.iter().map(|i| i.as_str()).collect();
        assert_eq!(
            ident_order, str_order,
            "Ident sort order must equal &str sort order"
        );

        // And explicit pairwise lexicographic checks (independent of the sort).
        let a = Ident::new("apple");
        let z = Ident::new("zebra");
        assert!(a < z);
        assert!(z > a);
        assert_eq!(a.cmp(&z), std::cmp::Ordering::Less);
        assert_eq!(a.cmp(&a.clone()), std::cmp::Ordering::Equal);
    }

    #[test]
    fn element_name_sort_order_matches_str_sort_order() {
        let raws = ["Boston", "atlanta", "nyc", "Chicago", "denver"];
        let names: Vec<CanonicalElementName> = raws
            .iter()
            .map(|s| CanonicalElementName::from_raw(s))
            .collect();

        let mut by_name = names.clone();
        by_name.sort();
        let mut by_str = names.clone();
        by_str.sort_by(|a, b| a.as_str().cmp(b.as_str()));
        assert_eq!(
            by_name.iter().map(|n| n.as_str()).collect::<Vec<_>>(),
            by_str.iter().map(|n| n.as_str()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn dimension_name_sort_order_matches_str_sort_order() {
        let raws = ["Region", "age_group", "scenario", "Cohort"];
        let names: Vec<CanonicalDimensionName> = raws
            .iter()
            .map(|s| CanonicalDimensionName::from_raw(s))
            .collect();

        let mut by_name = names.clone();
        by_name.sort();
        let mut by_str = names.clone();
        by_str.sort_by(|a, b| a.as_str().cmp(b.as_str()));
        assert_eq!(
            by_name.iter().map(|n| n.as_str()).collect::<Vec<_>>(),
            by_str.iter().map(|n| n.as_str()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn btreeset_of_idents_is_lexicographically_ordered() {
        // BTreeSet ordering is the runlist-determinism-critical case.
        let set: BTreeSet<Ident<Canonical>> = ["gamma", "alpha", "beta", "Alpha"]
            .iter()
            .map(|s| Ident::new(s))
            .collect();
        // "Alpha" dedups with "alpha", so 3 distinct elements.
        let ordered: Vec<&str> = set.iter().map(|i| i.as_str()).collect();
        assert_eq!(ordered, vec!["alpha", "beta", "gamma"]);
    }

    // ----- Constraint 3: value equality + de-duplication -----

    #[test]
    fn equal_inputs_are_equal_and_dedup_to_one_allocation() {
        // Two independent constructions of an equal canonical value.
        let a = Ident::new("Hello World");
        let b = Ident::new("hello world"); // canonicalizes identically
        assert_eq!(a, b, "values that canonicalize equally must be ==");
        assert_eq!(
            data_ptr(a.as_str()),
            data_ptr(b.as_str()),
            "equal interned idents must share one backing allocation"
        );

        // A distinct value must NOT share the allocation.
        let c = Ident::new("different");
        assert_ne!(a, c);
        assert_ne!(data_ptr(a.as_str()), data_ptr(c.as_str()));
    }

    #[test]
    fn clone_shares_backing_allocation() {
        let a = Ident::new("some_variable_name");
        let b = a.clone();
        assert_eq!(a, b);
        assert_eq!(
            data_ptr(a.as_str()),
            data_ptr(b.as_str()),
            "Clone must be a cheap handle copy sharing the allocation, not a fresh String"
        );
    }

    #[test]
    fn element_and_dimension_names_dedup() {
        let e1 = CanonicalElementName::from_raw("New York");
        let e2 = CanonicalElementName::from_raw("new_york");
        assert_eq!(e1, e2);
        assert_eq!(data_ptr(e1.as_str()), data_ptr(e2.as_str()));

        let d1 = CanonicalDimensionName::from_raw("Region");
        let d2 = CanonicalDimensionName::from_raw("region");
        assert_eq!(d1, d2);
        assert_eq!(data_ptr(d1.as_str()), data_ptr(d2.as_str()));
    }

    #[test]
    fn from_unchecked_paths_dedup_with_new() {
        // The *_unchecked constructors assume canonical input; they must still
        // route through the interner so they dedup with Ident::new.
        let canonical = "already_canonical_ident";
        let a = Ident::new(canonical);
        let b = Ident::<Canonical>::from_unchecked(canonical.to_string());
        let c = Ident::<Canonical>::from_str_unchecked(canonical);
        assert_eq!(a, b);
        assert_eq!(a, c);
        assert_eq!(data_ptr(a.as_str()), data_ptr(b.as_str()));
        assert_eq!(data_ptr(a.as_str()), data_ptr(c.as_str()));
    }

    // ----- Constraint 3: Hash consistent with Eq AND the Borrow<str> path -----

    #[test]
    fn hashmap_lookup_by_value_and_by_borrowed_str() {
        let mut map: HashMap<Ident<Canonical>, i32> = HashMap::new();
        map.insert(Ident::new("Population"), 42);

        // Look up with an independently-constructed equal key (value path).
        assert_eq!(map.get(&Ident::new("population")), Some(&42));

        // Look up via Borrow<str> with the canonical string slice.
        assert_eq!(map.get("population"), Some(&42));

        // A non-present key.
        assert_eq!(map.get("nonexistent"), None);
    }

    #[test]
    fn hash_is_value_based_consistent_with_str() {
        use std::hash::{BuildHasher, RandomState};
        let state = RandomState::new();
        let ident = Ident::new("hello world");
        // Hashing the Ident must equal hashing its canonical &str, otherwise
        // the Borrow<str> HashMap lookup path is unsound.
        let h_ident = state.hash_one(&ident);
        let h_str = state.hash_one(ident.as_str());
        assert_eq!(
            h_ident, h_str,
            "Ident Hash must be value-based and match str Hash"
        );
    }

    // ----- Constraint 1: idempotency, sentinel, round-trip preserved -----

    #[test]
    fn canonicalization_is_idempotent_through_idents() {
        for raw in [
            "Hello World",
            "a.b",
            "\"a.b\"",
            "\"Goal 1.5 for Temperature\"",
            "model.sub.variable",
        ] {
            let once = Ident::new(raw);
            let twice = Ident::new(once.as_str());
            assert_eq!(once, twice, "Ident::new not idempotent for {raw:?}");
            assert_eq!(once.as_str(), twice.as_str());
        }
    }

    #[test]
    fn quoted_literal_period_sentinel_survives_through_ident() {
        // The U+2024 sentinel must be preserved in canonical form and reverse
        // back to a literal `.` for source output (#559).
        let ident = Ident::new("\"a.b\"");
        assert_eq!(ident.as_str(), "a\u{2024}b");
        assert!(!ident.as_str().contains('.'));
        assert!(!ident.as_str().contains('·'));
        assert_eq!(ident.to_source_repr(), "a.b");

        // Re-interning the canonical sentinel form is a no-op and dedups.
        let again = Ident::new(ident.as_str());
        assert_eq!(ident, again);
        assert_eq!(data_ptr(ident.as_str()), data_ptr(again.as_str()));
    }

    #[test]
    fn source_round_trip_via_as_str_and_to_source_repr() {
        let cases = [
            ("model.variable", "model·variable", "model.variable"),
            ("\"a.b\"", "a\u{2024}b", "a.b"),
            ("plain_name", "plain_name", "plain_name"),
        ];
        for (raw, canonical, source) in cases {
            let ident = Ident::new(raw);
            assert_eq!(ident.as_str(), canonical);
            assert_eq!(ident.to_source_repr(), source);
        }
    }

    // ----- Constraint 6: non-leaking (refcount reclaim) -----

    #[test]
    fn dropping_all_handles_reclaims_the_interned_entry() {
        // Use a process-unique string so no other test/global holds a reference
        // and the reclaim assertion is deterministic on this single thread.
        let unique = "interner_reclaim_probe_\u{2024}_unique_value_xyz_42";
        let interner = Interner::global();
        assert!(!interner.contains(unique), "precondition: not yet interned");

        {
            let a = Ident::new(unique);
            let b = Ident::new(unique);
            assert!(interner.contains(unique), "entry must be live while held");
            // While both live, they share the allocation.
            assert_eq!(data_ptr(a.as_str()), data_ptr(b.as_str()));
        }

        // After both handles drop, the entry MUST be reclaimed (non-leaking).
        assert!(
            !interner.contains(unique),
            "interner leaked: entry survived after all handles dropped"
        );

        // Re-interning works and is observable again.
        let c = Ident::new(unique);
        assert_eq!(c.as_str(), unique);
        assert!(interner.contains(unique));
    }

    #[test]
    fn many_distinct_strings_are_all_reclaimed_after_drop() {
        // The global interner is shared across the whole test binary, so we
        // can't assert an exact global count (other tests intern concurrently).
        // Instead probe a batch of process-unique strings: all present while
        // held, all reclaimed once dropped. `live_entry_count` is exercised
        // here only as a coarse monotonicity sanity check.
        let interner = Interner::global();
        let words: Vec<String> = (0..50)
            .map(|i| format!("batch_reclaim_probe_unique_\u{2024}_{i}"))
            .collect();
        for w in &words {
            assert!(!interner.contains(w), "precondition: {w:?} not interned");
        }

        let names: Vec<Ident<Canonical>> = words.iter().map(|w| Ident::new(w)).collect();
        let count_with_batch = interner.live_entry_count();
        for w in &words {
            assert!(interner.contains(w), "{w:?} must be live while held");
        }
        assert!(
            count_with_batch >= 50,
            "the batch contributes at least its own entries"
        );

        drop(names);
        for w in &words {
            assert!(
                !interner.contains(w),
                "{w:?} leaked after all handles dropped"
            );
        }
    }

    #[test]
    fn concurrent_intern_and_drop_is_consistent_and_reclaims() {
        // Stress the drop/intern race across rayon-like contention: many
        // threads repeatedly intern and drop a small shared set of strings.
        // Afterwards every string must be reclaimed (no live entry remains)
        // and equality must remain pointer-shared for concurrent live handles.
        use std::sync::Arc as StdArc;
        use std::thread;

        let interner = Interner::global();
        let words: StdArc<Vec<String>> = StdArc::new(
            (0..16)
                .map(|i| format!("concurrent_interner_probe_word_{i}"))
                .collect(),
        );
        // Ensure clean baseline for these specific words.
        for w in words.iter() {
            assert!(!interner.contains(w));
        }

        let mut handles = Vec::new();
        for _ in 0..8 {
            let words = StdArc::clone(&words);
            handles.push(thread::spawn(move || {
                for _ in 0..2000 {
                    for w in words.iter() {
                        let a = Ident::new(w);
                        let b = Ident::new(w);
                        // Concurrent live handles of equal content always share
                        // the backing payload (dedup holds under contention).
                        assert_eq!(a, b);
                        assert_eq!(a.as_str(), w.as_str());
                    }
                }
            }));
        }
        for h in handles {
            h.join().unwrap();
        }

        // All transient handles are gone -> every word must be reclaimed.
        for w in words.iter() {
            assert!(
                !interner.contains(w),
                "word {w:?} leaked after concurrent stress"
            );
        }
    }

    // ----- Constraint 4: salsa backdating semantics on the handle -----

    /// Salsa backdates a re-executed query's memo purely by `PartialEq`
    /// (`values_equal`), so the handle's equality must be VALUE equality:
    /// two independently-constructed handles for the same string compare
    /// equal, and different strings compare unequal. The pointer fast-path
    /// in `PartialEq` is value-correct only because the interner
    /// de-duplicates; this pins the observable rule salsa depends on.
    #[test]
    fn handle_equality_is_value_equality_for_salsa_backdating() {
        assert_eq!(
            CanonicalStorage::intern("a_value"),
            CanonicalStorage::intern("a_value"),
            "separately-interned equal strings must compare equal"
        );
        assert_ne!(
            CanonicalStorage::intern("a_value"),
            CanonicalStorage::intern("another_value"),
            "different strings must compare unequal"
        );
        assert_eq!(
            Ident::<Canonical>::new("a_value"),
            Ident::<Canonical>::new("a_value"),
            "Ident equality must delegate to the handle's value equality"
        );
    }
}

// Implementations for identifier types

impl RawIdent {
    /// Create a new raw identifier
    pub fn new(s: String) -> Self {
        RawIdent(s)
    }

    /// Create from a string slice
    pub fn new_from_str(s: &str) -> Self {
        RawIdent(s.to_string())
    }

    /// Canonicalize this identifier (returns new type)
    pub fn canonicalize(&self) -> Ident<Canonical> {
        Ident::new(&self.0)
    }

    /// Get the underlying raw string
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl CanonicalDimensionName {
    /// Create from a raw string, canonicalizing it
    pub fn from_raw(s: &str) -> Self {
        CanonicalDimensionName(CanonicalStorage::intern(&canonicalize(s)))
    }

    /// Get the underlying canonical string
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl RawDimensionName {
    /// Create a new raw dimension name
    pub fn new(s: String) -> Self {
        RawDimensionName(s)
    }

    /// Canonicalize this dimension name
    pub fn canonicalize(&self) -> CanonicalDimensionName {
        CanonicalDimensionName(CanonicalStorage::intern(&canonicalize(&self.0)))
    }

    /// Get the underlying raw string
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl CanonicalElementName {
    /// Create from a raw string, canonicalizing it
    pub fn from_raw(s: &str) -> Self {
        CanonicalElementName(CanonicalStorage::intern(&canonicalize(s)))
    }

    /// Get the underlying canonical string
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl RawElementName {
    /// Create a new raw element name
    pub fn new(s: String) -> Self {
        RawElementName(s)
    }

    /// Canonicalize this element name
    pub fn canonicalize(&self) -> CanonicalElementName {
        CanonicalElementName(CanonicalStorage::intern(&canonicalize(&self.0)))
    }

    /// Get the underlying raw string
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

// Display implementations for better debugging

impl fmt::Display for RawIdent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl fmt::Display for CanonicalDimensionName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0.as_str())
    }
}

impl fmt::Display for RawDimensionName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl fmt::Display for CanonicalElementName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0.as_str())
    }
}

impl fmt::Display for RawElementName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<CanonicalDimensionName> for DimensionName {
    fn from(canonical: CanonicalDimensionName) -> Self {
        canonical.0.as_str().to_owned()
    }
}

impl From<CanonicalElementName> for ElementName {
    fn from(canonical: CanonicalElementName) -> Self {
        canonical.0.as_str().to_owned()
    }
}

// AsRef implementations for convenient use in APIs

impl AsRef<str> for RawIdent {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for CanonicalDimensionName {
    fn as_ref(&self) -> &str {
        self.0.as_str()
    }
}

impl AsRef<str> for RawDimensionName {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for CanonicalElementName {
    fn as_ref(&self) -> &str {
        self.0.as_str()
    }
}

impl AsRef<str> for RawElementName {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

// ===== New Phantom Type-based Identifier System =====
// This system provides zero-copy substring operations while maintaining
// canonicalization guarantees through the type system.

/// Marker type for canonical identifiers
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Canonical;

/// Marker type for raw (non-canonical) identifiers
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Raw;

/// An owned identifier with state tracking (canonical or raw).
///
/// In practice the inner string is always canonical (`Ident<Raw>` is never
/// instantiated), so the storage is the interned [`CanonicalStorage`] handle:
/// constructing an `Ident` for an already-seen identifier is allocation-free
/// and `Clone` is a refcount bump. The derived `PartialEq`/`Eq`/`Hash`/`Ord`/
/// `PartialOrd` delegate to that handle's manual impls (value equality,
/// value-based hash consistent with `Borrow<str>`, lexicographic ordering),
/// preserving the previous `String`-backed semantics. Salsa backdates query
/// memos purely by `PartialEq`, so that value equality is also what decides
/// salsa early-cutoff for any value carrying an `Ident`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Ident<State = Canonical> {
    inner: CanonicalStorage,
    _phantom: PhantomData<State>,
}

/// A borrowed canonical string slice wrapper
/// This type guarantees the string is in canonical form
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(PartialEq, Eq, Hash)]
pub struct CanonicalStr<'a> {
    inner: &'a str,
}

impl<'a> CanonicalStr<'a> {
    /// Create a CanonicalStr from a string known to be canonical
    ///
    /// Note: Caller must guarantee that the string is already in canonical form
    pub fn from_canonical_unchecked(s: &'a str) -> Self {
        CanonicalStr { inner: s }
    }

    /// Get the underlying string slice
    pub fn as_str(&self) -> &str {
        self.inner
    }

    /// Convert canonical identifier to source code representation.
    ///
    /// Replaces middle dots (·) used internally for module hierarchy separators
    /// back to periods (.) for display in source code or user-facing output.
    pub fn to_source_repr(&self) -> Cow<'_, str> {
        canonical_to_source(self.inner)
    }

    /// Find and split at the first middle dot, maintaining canonical guarantee
    pub fn split_at_dot(&self) -> Option<(CanonicalStr<'a>, CanonicalStr<'a>)> {
        self.inner.find('·').map(|pos| {
            let before = CanonicalStr::from_canonical_unchecked(&self.inner[..pos]);
            let after = CanonicalStr::from_canonical_unchecked(&self.inner[pos + '·'.len_utf8()..]);
            (before, after)
        })
    }

    /// Strip a prefix if present, maintaining canonical guarantee
    pub fn strip_prefix(&self, prefix: &str) -> Option<CanonicalStr<'a>> {
        self.inner
            .strip_prefix(prefix)
            .map(CanonicalStr::from_canonical_unchecked)
    }

    /// Check if this identifier starts with a given prefix
    pub fn starts_with(&self, prefix: &str) -> bool {
        self.inner.starts_with(prefix)
    }

    /// Find the position of a substring
    pub fn find(&self, pat: &str) -> Option<usize> {
        self.inner.find(pat)
    }
}

impl Ident<Canonical> {
    /// Create a canonical identifier from a raw string.
    ///
    /// This is the primary constructor: it canonicalizes the input and wraps
    /// the result in an owned `Ident`. Internally uses `canonicalize()` which
    /// avoids allocation when the input is already canonical.
    pub fn new(s: &str) -> Self {
        Ident {
            // `canonicalize` borrows when already canonical; the interner takes
            // a `&str` either way, allocating the backing storage only on the
            // first sighting of this canonical form.
            inner: CanonicalStorage::intern(&canonicalize(s)),
            _phantom: PhantomData,
        }
    }

    /// Create from an already-canonicalized string
    ///
    /// Note: Caller must guarantee the string is already canonical
    pub fn from_unchecked(s: String) -> Self {
        Ident {
            inner: CanonicalStorage::intern(&s),
            _phantom: PhantomData,
        }
    }

    /// Create from an already-canonicalized string slice
    ///
    /// Note: Caller must guarantee the string is already canonical
    pub fn from_str_unchecked(s: &str) -> Self {
        Ident {
            inner: CanonicalStorage::intern(s),
            _phantom: PhantomData,
        }
    }

    /// Get as a CanonicalStr
    pub fn as_canonical_str(&self) -> CanonicalStr<'_> {
        CanonicalStr::from_canonical_unchecked(self.inner.as_str())
    }

    /// Join two canonical identifiers with a middle dot separator
    pub fn join(module: &CanonicalStr, var: &CanonicalStr) -> Self {
        Ident {
            inner: CanonicalStorage::intern(&format!("{}·{}", module.as_str(), var.as_str())),
            _phantom: PhantomData,
        }
    }

    /// Get the underlying canonical string
    pub fn as_str(&self) -> &str {
        self.inner.as_str()
    }

    /// Consume self and return the underlying String
    pub fn into_string(self) -> String {
        self.inner.as_str().to_owned()
    }

    /// Convert canonical identifier to source code representation.
    ///
    /// Replaces middle dots (·) used internally for module hierarchy separators
    /// back to periods (.) for display in source code or user-facing output.
    ///
    /// For example:
    /// - Internal canonical: "model·variable"
    /// - Source representation: "model.variable"
    ///
    /// This is the inverse of the canonicalization process that converts
    /// periods to middle dots to distinguish module separators from literal
    /// periods in quoted identifiers.
    pub fn to_source_repr(&self) -> String {
        canonical_to_source(self.inner.as_str()).into_owned()
    }
}

// Implement AsRef for convenient usage
impl AsRef<str> for Ident<Canonical> {
    fn as_ref(&self) -> &str {
        self.inner.as_str()
    }
}

// Implement Borrow for HashMap lookups.
//
// NB: this is what makes the value-based `Hash` on `CanonicalStorage`
// mandatory -- a `HashMap<Ident<Canonical>, V>` can be probed with a `&str`
// key, which is hashed with `str`'s hasher; the stored key's hash (delegated
// through the derive to `CanonicalStorage::hash`) must match it.
impl std::borrow::Borrow<str> for Ident<Canonical> {
    fn borrow(&self) -> &str {
        self.inner.as_str()
    }
}

// Re-tagging one canonical newtype as another, for the cases where the source
// value's TYPE already proves the string is canonical: `Ident<Canonical>` and
// the dimension/element names are three tags over one interned storage, so
// there is nothing to compute and nothing to allocate -- the payload is shared
// and the conversion is a refcount bump.
//
// Spelling this `CanonicalDimensionName::from_raw(ident.as_str())` instead is
// what these exist to stop: that re-runs the whole canonicalize pass over a
// string that is canonical by construction, then takes a shard of the global
// interner to look up a payload the caller is already holding. On the array
// lowering path (`Expr3::from_expr2` expanding a bare array reference into one
// star range per declared dimension) that was measured at 6% of a C-LEARN
// compile's total instructions.
impl From<&Ident<Canonical>> for CanonicalDimensionName {
    fn from(ident: &Ident<Canonical>) -> Self {
        CanonicalDimensionName(ident.inner.clone())
    }
}

impl From<&Ident<Canonical>> for CanonicalElementName {
    fn from(ident: &Ident<Canonical>) -> Self {
        CanonicalElementName(ident.inner.clone())
    }
}

impl From<&CanonicalDimensionName> for CanonicalElementName {
    fn from(name: &CanonicalDimensionName) -> Self {
        CanonicalElementName(name.0.clone())
    }
}

// The same, for the two other newtypes over `CanonicalStorage`. Probing a map
// with a `&str` skips the global interner entirely: constructing a
// `CanonicalDimensionName`/`CanonicalElementName` just to ask whether a key is
// present takes a sharded mutex, allocates an `Arc` payload on first sighting,
// and bumps/drops a refcount every time -- all to produce a value the lookup
// discards. Sound for exactly the reason above: `CanonicalStorage::hash` hashes
// the string content, so a `&str` probe hashes identically to the stored key.
impl std::borrow::Borrow<str> for CanonicalDimensionName {
    fn borrow(&self) -> &str {
        self.0.as_str()
    }
}

impl std::borrow::Borrow<str> for CanonicalElementName {
    fn borrow(&self) -> &str {
        self.0.as_str()
    }
}

impl<'a> AsRef<str> for CanonicalStr<'a> {
    fn as_ref(&self) -> &str {
        self.inner
    }
}

// Display implementations
impl fmt::Display for Ident<Canonical> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.inner.as_str())
    }
}

impl<'a> fmt::Display for CanonicalStr<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.inner)
    }
}

// ===== Helper Functions for Regex-Free Parsing =====

/// Replace whitespace sequences with underscores.
/// Handles: literal `\n` and `\r` (two-character sequences), actual newlines/carriage returns,
/// tabs, spaces, and non-breaking spaces (U+00A0). Consecutive matches become a single underscore.
fn replace_whitespace_with_underscore(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    let mut in_whitespace = false;

    while let Some(c) = chars.next() {
        // Check for escaped sequences: literal \n or \r (two characters)
        if c == '\\'
            && let Some(&next) = chars.peek()
            && (next == 'n' || next == 'r')
        {
            chars.next(); // consume the 'n' or 'r'
            if !in_whitespace {
                result.push('_');
                in_whitespace = true;
            }
            continue;
        } else if c == '\\' {
            // Not an escape sequence we handle, pass through
            in_whitespace = false;
            result.push(c);
        } else if c == '\n' || c == '\r' || c == '\t' || c == ' ' || c == '\u{00A0}' {
            // Actual whitespace characters
            if !in_whitespace {
                result.push('_');
                in_whitespace = true;
            }
        } else {
            in_whitespace = false;
            result.push(c);
        }
    }

    result
}

/// Iterator over identifier parts (quoted and unquoted sections).
/// Handles quoted strings with escaped quotes inside them.
/// Matches the regex: [^"]+|"((\\")|[^"])*"
struct IdentifierPartIterator<'a> {
    remaining: &'a str,
}

impl<'a> IdentifierPartIterator<'a> {
    fn new(s: &'a str) -> Self {
        IdentifierPartIterator { remaining: s }
    }
}

impl<'a> Iterator for IdentifierPartIterator<'a> {
    type Item = &'a str;

    fn next(&mut self) -> Option<Self::Item> {
        if self.remaining.is_empty() {
            return None;
        }

        let bytes = self.remaining.as_bytes();

        if bytes[0] == b'"' {
            // Quoted section: find the closing quote, handling escaped quotes
            let mut i = 1;
            while i < bytes.len() {
                if bytes[i] == b'\\' && i + 1 < bytes.len() && bytes[i + 1] == b'"' {
                    // Skip escaped quote
                    i += 2;
                } else if bytes[i] == b'"' {
                    // Found closing quote
                    let part = &self.remaining[..i + 1];
                    self.remaining = &self.remaining[i + 1..];
                    return Some(part);
                } else {
                    i += 1;
                }
            }
            // Unclosed quote - return rest as is
            let part = self.remaining;
            self.remaining = "";
            Some(part)
        } else {
            // Unquoted section: find the next quote or end
            let end = self.remaining.find('"').unwrap_or(self.remaining.len());
            let part = &self.remaining[..end];
            self.remaining = &self.remaining[end..];
            if part.is_empty() {
                self.next()
            } else {
                Some(part)
            }
        }
    }
}

#[cfg(test)]
mod whitespace_replacement_tests {
    use super::*;

    #[test]
    fn test_replace_actual_newline() {
        assert_eq!(replace_whitespace_with_underscore("a\nb"), "a_b");
    }

    #[test]
    fn test_replace_actual_carriage_return() {
        assert_eq!(replace_whitespace_with_underscore("a\rb"), "a_b");
    }

    #[test]
    fn test_replace_crlf() {
        assert_eq!(replace_whitespace_with_underscore("a\r\nb"), "a_b");
    }

    #[test]
    fn test_replace_escaped_newline() {
        // Literal backslash-n in the string (two characters: '\' and 'n')
        assert_eq!(replace_whitespace_with_underscore("a\\nb"), "a_b");
    }

    #[test]
    fn test_replace_escaped_carriage_return() {
        // Literal backslash-r in the string (two characters: '\' and 'r')
        assert_eq!(replace_whitespace_with_underscore("a\\rb"), "a_b");
    }

    #[test]
    fn test_replace_space() {
        assert_eq!(
            replace_whitespace_with_underscore("hello world"),
            "hello_world"
        );
    }

    #[test]
    fn test_replace_non_breaking_space() {
        // U+00A0 non-breaking space
        assert_eq!(replace_whitespace_with_underscore("a\u{00A0}b"), "a_b");
    }

    #[test]
    fn test_replace_tab() {
        assert_eq!(replace_whitespace_with_underscore("a\tb"), "a_b");
        // Tabs collapse with other whitespace
        assert_eq!(replace_whitespace_with_underscore("a\t \nb"), "a_b");
    }

    #[test]
    fn test_consecutive_whitespace_collapsed() {
        // Multiple spaces should become single underscore
        assert_eq!(replace_whitespace_with_underscore("a   b"), "a_b");
        // Mixed whitespace types should collapse
        assert_eq!(replace_whitespace_with_underscore("a \n \r b"), "a_b");
    }

    #[test]
    fn test_leading_trailing_whitespace() {
        assert_eq!(replace_whitespace_with_underscore(" a b "), "_a_b_");
    }

    #[test]
    fn test_empty_string() {
        assert_eq!(replace_whitespace_with_underscore(""), "");
    }

    #[test]
    fn test_no_whitespace() {
        assert_eq!(replace_whitespace_with_underscore("hello"), "hello");
    }

    #[test]
    fn test_unicode_preserved() {
        assert_eq!(replace_whitespace_with_underscore("Å b"), "Å_b");
    }

    #[test]
    fn test_multiple_segments() {
        assert_eq!(replace_whitespace_with_underscore("a b c d"), "a_b_c_d");
    }
}

#[cfg(test)]
mod identifier_part_iterator_tests {
    use super::*;

    #[test]
    fn test_simple_unquoted() {
        let parts: Vec<_> = IdentifierPartIterator::new("abc").collect();
        assert_eq!(parts, vec!["abc"]);
    }

    #[test]
    fn test_simple_quoted() {
        let parts: Vec<_> = IdentifierPartIterator::new("\"abc\"").collect();
        assert_eq!(parts, vec!["\"abc\""]);
    }

    #[test]
    fn test_mixed_unquoted_quoted() {
        // a."b c" should yield "a." and "\"b c\""
        let parts: Vec<_> = IdentifierPartIterator::new("a.\"b c\"").collect();
        assert_eq!(parts, vec!["a.", "\"b c\""]);
    }

    #[test]
    fn test_multiple_quoted_sections() {
        // "a/d"."b c" should yield "\"a/d\"", ".", "\"b c\""
        let parts: Vec<_> = IdentifierPartIterator::new("\"a/d\".\"b c\"").collect();
        assert_eq!(parts, vec!["\"a/d\"", ".", "\"b c\""]);
    }

    #[test]
    fn test_escaped_quote_inside_quoted() {
        // "b \"c\"" should be a single part with escaped quotes
        let parts: Vec<_> = IdentifierPartIterator::new("\"b \\\"c\\\"\"").collect();
        assert_eq!(parts, vec!["\"b \\\"c\\\"\""]);
    }

    #[test]
    fn test_complex_mixed() {
        // "a/d"."b \"c\"" should yield parts correctly
        let parts: Vec<_> = IdentifierPartIterator::new("\"a/d\".\"b \\\"c\\\"\"").collect();
        assert_eq!(parts, vec!["\"a/d\"", ".", "\"b \\\"c\\\"\""]);
    }

    #[test]
    fn test_empty_string() {
        let parts: Vec<_> = IdentifierPartIterator::new("").collect();
        assert!(parts.is_empty());
    }

    #[test]
    fn test_only_dots() {
        let parts: Vec<_> = IdentifierPartIterator::new("...").collect();
        assert_eq!(parts, vec!["..."]);
    }

    #[test]
    fn test_unquoted_with_dots() {
        let parts: Vec<_> = IdentifierPartIterator::new("a.b.c").collect();
        assert_eq!(parts, vec!["a.b.c"]);
    }
}

// ===== Engine-specific additions =====

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum UnitError {
    /// A syntax error in a `<units>` string. The reason rides on the
    /// `EquationError` itself, like every other error this crate raises --
    /// `ConsistencyError` and `InferenceError` carry their own only because
    /// neither has an `EquationError` to put it on.
    DefinitionError(EquationError),
    ConsistencyError(ErrorCode, Loc, Option<String>),
    /// For inference errors that may span multiple variables.
    /// Each source is (variable_identifier, optional_location_in_that_equation).
    InferenceError {
        code: ErrorCode,
        sources: Vec<(String, Option<Loc>)>,
        details: Option<String>,
    },
}

impl fmt::Display for UnitError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            UnitError::DefinitionError(err) => write!(f, "unit definition:{err}"),
            UnitError::ConsistencyError(err, loc, details) => {
                if let Some(details) = details {
                    write!(f, "unit consistency:{loc}:{err} -- {details}")
                } else {
                    write!(f, "unit consistency:{loc}:{err}")
                }
            }
            UnitError::InferenceError {
                code,
                sources,
                details,
            } => {
                // Format sources as "var@loc" or just "var" if no location
                let sources_str = if sources.is_empty() {
                    "unknown".to_string()
                } else {
                    sources
                        .iter()
                        .map(|(var, loc)| {
                            if let Some(loc) = loc {
                                format!("'{var}'@{loc}")
                            } else {
                                format!("'{var}'")
                            }
                        })
                        .collect::<Vec<_>>()
                        .join(", ")
                };
                if let Some(details) = details {
                    write!(f, "unit inference [{sources_str}]: {code} -- {details}")
                } else {
                    write!(f, "unit inference [{sources_str}]: {code}")
                }
            }
        }
    }
}

pub type UnitResult<T> = std::result::Result<T, UnitError>;

// Macros for error creation

#[macro_export]
macro_rules! eprintln(
    ($($arg:tt)*) => {{
        use std::io::Write;
        let r = writeln!(&mut ::std::io::stderr(), $($arg)*);
        r.expect("failed printing to stderr");
    }}
);

/// `Err` of an `EquationError` at `start..end`. The four-argument form carries
/// the reason the raising site had in hand; the three-argument form is for a
/// failure the code and the span already describe (see `EquationError::details`).
#[macro_export]
macro_rules! eqn_err(
    ($code:tt, $start:expr, $end:expr) => {{
        use $crate::common::{EquationError, ErrorCode};
        Err(EquationError::new(ErrorCode::$code, $start, $end))
    }};
    ($code:tt, $start:expr, $end:expr, $details:expr) => {{
        use $crate::common::{EquationError, ErrorCode};
        Err(EquationError::detailed(ErrorCode::$code, $start, $end, $details))
    }};
);

#[macro_export]
macro_rules! model_err(
    ($code:tt, $str:expr) => {{
        use $crate::common::{Error, ErrorCode, ErrorKind};
        Err(Error::new(
            ErrorKind::Model,
            ErrorCode::$code,
            Some($str),
        ))
    }}
);

#[macro_export]
macro_rules! sim_err {
    ($code:tt, $str:expr) => {{
        use $crate::common::{Error, ErrorCode, ErrorKind};
        Err(Error::new(
            ErrorKind::Simulation,
            ErrorCode::$code,
            Some($str),
        ))
    }};
    ($code:tt) => {{
        use $crate::common::{Error, ErrorCode, ErrorKind};
        Err(Error::new(ErrorKind::Simulation, ErrorCode::$code, None))
    }};
}

#[test]
fn test_unit_error_inference_display() {
    use crate::ast::Loc;

    // Test InferenceError with no sources (edge case)
    let err = UnitError::InferenceError {
        code: ErrorCode::UnitMismatch,
        sources: vec![],
        details: None,
    };
    let display = format!("{err}");
    assert!(
        display.contains("unknown"),
        "Empty sources should show 'unknown'"
    );
    assert!(display.contains("unit_mismatch"));

    // Test InferenceError with single source, no location
    let err = UnitError::InferenceError {
        code: ErrorCode::UnitMismatch,
        sources: vec![("my_var".to_string(), None)],
        details: None,
    };
    let display = format!("{err}");
    assert!(display.contains("'my_var'"), "Should contain variable name");
    assert!(!display.contains("@"), "Should not have @ when no location");

    // Test InferenceError with single source, with location
    let err = UnitError::InferenceError {
        code: ErrorCode::UnitMismatch,
        sources: vec![("my_var".to_string(), Some(Loc::new(5, 10)))],
        details: None,
    };
    let display = format!("{err}");
    assert!(
        display.contains("'my_var'@"),
        "Should contain variable with @ for location"
    );
    assert!(
        display.contains("5:10"),
        "Should contain location 5:10, got: {}",
        display
    );

    // Test InferenceError with multiple sources
    let err = UnitError::InferenceError {
        code: ErrorCode::UnitMismatch,
        sources: vec![
            ("var_a".to_string(), Some(Loc::new(0, 5))),
            ("var_b".to_string(), None),
        ],
        details: Some("conflicting units".to_string()),
    };
    let display = format!("{err}");
    assert!(display.contains("'var_a'@"));
    assert!(display.contains("'var_b'"));
    assert!(
        display.contains(", "),
        "Should have comma-separated sources"
    );
    assert!(
        display.contains("conflicting units"),
        "Should contain details"
    );
    assert!(
        display.contains("--"),
        "Should have -- separator for details"
    );
}
