// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::borrow::Cow;
use std::collections::{BTreeSet, HashMap};
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
/// hash. Compilation fans out across rayon threads, so sharding keeps lock
/// contention low without a concurrent-map dependency.
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
/// - `salsa::Update`: interned values are immutable, so `maybe_update`
///   overwrites and reports a change iff the new value differs from the old by
///   string content. `Arc<Interned>`/`str` are not covered by salsa's blanket
///   `Update` impls, so this is provided manually here; the public newtypes
///   keep `#[derive(salsa::Update)]`, whose per-field dispatch finds this impl.
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

// SAFETY: `CanonicalStorage` is an owned, immutable interned handle. It owns
// its (refcounted) data, contains no borrowed `'db` references, and its `Eq`
// is value equality (consistent with `Hash`), so comparing an old-revision
// value with a new-revision value is well defined. `maybe_update` therefore
// follows the standard owned-`Eq` pattern: overwrite and report a change iff
// the values differ. This mirrors salsa's `update_fallback` but is written by
// hand because neither `Arc<Interned>` nor `str` is covered by salsa's blanket
// `Update` impls.
//
// The crate is `#![deny(unsafe_code)]`; this is the one opt-in here, mirroring
// the precedent in `vm.rs`. The `unsafe` is confined to dereferencing the
// `*mut Self` salsa hands us, under the documented `Update` contract.
#[allow(unsafe_code)]
unsafe impl salsa::Update for CanonicalStorage {
    unsafe fn maybe_update(old_pointer: *mut Self, new_value: Self) -> bool {
        // SAFETY: by the `Update` contract `old_pointer` points to a valid,
        // fully-owned `CanonicalStorage` from a (possibly older) revision; an
        // owned interned handle has no dangling borrows, so taking a `&mut` is
        // sound.
        let old = unsafe { &mut *old_pointer };
        if *old != new_value {
            *old = new_value;
            true
        } else {
            false
        }
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
#[derive(Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct RawIdent(String);

/// A canonicalized dimension name
///
/// Backed by interned, de-duplicated storage (see [`CanonicalStorage`]): the
/// derived `PartialEq`/`Eq`/`Hash`/`Ord`/`PartialOrd`/`salsa::Update` all
/// delegate to that handle's manual impls (value equality + value hash +
/// lexicographic order), so the observable behavior is identical to the old
/// `String` backing while construction and clone avoid allocation.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord, salsa::Update)]
pub struct CanonicalDimensionName(CanonicalStorage);

/// A raw dimension name as it appears in source
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct RawDimensionName(String);

/// A canonicalized element name (dimension element)
///
/// Backed by interned, de-duplicated storage (see [`CanonicalStorage`]); the
/// derived trait impls delegate to that handle, matching the old `String`
/// backing's behavior without per-construction allocation.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord, salsa::Update)]
pub struct CanonicalElementName(CanonicalStorage);

/// A raw element name as it appears in source
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct RawElementName(String);

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, salsa::Update)]
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
        };

        write!(f, "{name}")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub struct EquationError {
    pub start: u16,
    pub end: u16,
    pub code: ErrorCode,
}

impl fmt::Display for EquationError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}:{}:{}", self.start, self.end, self.code)
    }
}

impl From<Error> for EquationError {
    fn from(err: Error) -> Self {
        EquationError {
            code: err.code,
            start: 0,
            end: 0,
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
            c if c.to_lowercase().ne(std::iter::once(c)) => return false,
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
pub fn canonicalize(name: &str) -> Cow<'_, str> {
    // Fast path: if the name is already trimmed and canonical, avoid allocation.
    let trimmed = name.trim();
    if is_canonical(trimmed) {
        // Return the trimmed slice (which may equal the original if there was
        // no leading/trailing whitespace).
        return Cow::Borrowed(trimmed);
    }

    // Slow path: full canonicalization with allocation.
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
        } else {
            // Replace periods with middle dots (·) for module hierarchy separators.
            // This allows us to distinguish between:
            // - Module separators: model.variable -> model·variable
            // - Literal periods in quoted names: "a.b" -> a<U+2024>b
            Cow::Owned(part.replace('.', "·"))
        };

        let part = part.replace("\\\\", "\\");
        let part = replace_whitespace_with_underscore(&part);
        let part = part.to_lowercase();

        canonicalized_name.push_str(&part);
    }

    Cow::Owned(canonicalized_name)
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
fn test_stdlib_model_name_canonicalization() {
    // Test canonicalization of stdlib model names
    let stdlib_name = "stdlib⁚smth1";
    let canonical = canonicalize(stdlib_name);
    assert_eq!(&*canonical, "stdlib⁚smth1");

    // Already-canonical stdlib name should borrow, not allocate
    assert!(matches!(canonical, Cow::Borrowed(_)));
}

#[test]
fn test_stdlib_variable_canonicalization() {
    // Test that stdlib variable names are canonicalized correctly
    let names = vec!["input", "output", "Output", "delay_time", "initial_value"];
    for name in names {
        let canonical = canonicalize(name);
        let expected = canonicalize(name);
        assert_eq!(&*canonical, &*expected, "Failed for {name}");
    }

    // Specifically test Output -> output conversion
    assert_eq!(&*canonicalize("Output"), "output");
}

#[test]
fn test_new_ident_basic_operations() {
    // Test basic creation and conversion
    let ident = Ident::new("Hello World");
    assert_eq!(ident.as_str(), "hello_world");

    // Test source representation conversion
    let ident2 = Ident::new("a.b");
    assert_eq!(ident2.as_str(), "a·b");
    assert_eq!(ident2.to_source_repr(), "a.b");

    // A literal period in a quoted ident canonicalizes to the sentinel
    // (#559) but still renders back to `.` for source/display output.
    let ident3 = Ident::new("\"a.b\"");
    assert_eq!(ident3.as_str(), "a\u{2024}b");
    assert_eq!(ident3.to_source_repr(), "a.b");
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
fn test_ident_with_subscript() {
    let ident = Ident::new("my_array");
    let subscripted = ident.with_subscript("1,2");
    assert_eq!(subscripted.as_str(), "my_array[1,2]");
    assert_eq!(subscripted.to_source_repr(), "my_array[1,2]");

    // Test with identifier containing middle dot
    let ident2 = Ident::new("model.var");
    let subscripted2 = ident2.with_subscript("i");
    assert_eq!(subscripted2.as_str(), "model.var[i]");
    assert_eq!(subscripted2.to_source_repr(), "model.var[i]");
}

#[test]
fn test_ident_strip_prefix() {
    let ident = Ident::new("model.variable");

    // Test successful prefix stripping
    if let Some(stripped) = ident.strip_prefix("model·") {
        assert_eq!(stripped.as_str(), "variable");
    } else {
        panic!("Expected successful prefix strip");
    }

    // Test unsuccessful prefix stripping
    assert!(ident.strip_prefix("other·").is_none());

    // Test stripping empty prefix
    if let Some(stripped) = ident.strip_prefix("") {
        assert_eq!(stripped.as_str(), "model·variable");
    } else {
        panic!("Expected successful empty prefix strip");
    }
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
fn test_ident_ref_operations() {
    let owned = Ident::new("model.variable");
    let borrowed = owned.as_ref();

    // Test basic operations
    assert_eq!(borrowed.as_str(), "model·variable");
    assert_eq!(borrowed.to_source_repr(), Cow::Borrowed("model.variable"));

    // Test strip_prefix on borrowed
    if let Some(stripped) = borrowed.strip_prefix("model·") {
        assert_eq!(stripped.as_str(), "variable");

        // Test that we can convert back to owned
        let owned_again = stripped.to_owned();
        assert_eq!(owned_again.as_str(), "variable");
    } else {
        panic!("Expected successful prefix strip");
    }
}

#[test]
fn test_ident_ref_zero_copy() {
    // This test verifies that IdentRef provides zero-copy substring operations
    let owned = Ident::new("very.long.module.path.to.variable");
    let borrowed = owned.as_ref();

    // Strip multiple prefixes without allocation
    let mut current = borrowed;
    let prefixes = ["very·", "long·", "module·", "path·", "to·"];

    for prefix in &prefixes {
        if let Some(stripped) = current.strip_prefix(prefix) {
            current = stripped;
        } else {
            panic!("Expected successful strip of {prefix}");
        }
    }

    assert_eq!(current.as_str(), "variable");
}

#[test]
fn test_canonical_str_utility_methods() {
    let ident = Ident::new("model.variable");
    let canonical_str = ident.as_canonical_str();

    // Test starts_with
    assert!(canonical_str.starts_with("model·"));
    assert!(!canonical_str.starts_with("other·"));

    // Test find
    // The string is "model·variable" where · is at byte position 5
    assert_eq!(canonical_str.find("·"), Some(5));

    // First let's verify what the actual string is
    let s = canonical_str.as_str();
    assert_eq!(s, "model·variable");

    // str::find() returns byte positions, and "·" is 3 bytes in UTF-8
    // "model" = bytes 0-4, "·" = bytes 5-7, "variable" starts at byte 8
    // But wait - str::find() actually returns the byte index!
    let var_pos = s.find("var").unwrap();
    assert_eq!(canonical_str.find("var"), Some(var_pos));
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

    // Test unchecked construction of IdentRef
    let canonical_str = "also_canonical";
    let ident_ref = IdentRef::<Canonical>::from_canonical_unchecked(canonical_str);
    assert_eq!(ident_ref.as_str(), "also_canonical");

    // Test unchecked construction of CanonicalStr
    let canonical_slice = CanonicalStr::from_canonical_unchecked("canonical·str");
    assert_eq!(canonical_slice.as_str(), "canonical·str");
}

#[test]
fn test_as_ref_implementations() {
    let ident = Ident::new("test");
    let _str_ref: &str = <Ident<Canonical> as AsRef<str>>::as_ref(&ident);
    assert_eq!(_str_ref, "test");

    let ident_ref = ident.as_ref();
    let _str_ref2: &str = <IdentRef<'_, Canonical> as AsRef<str>>::as_ref(&ident_ref);
    assert_eq!(_str_ref2, "test");

    let canonical_str = ident.as_canonical_str();
    let _str_ref3: &str = canonical_str.as_ref();
    assert_eq!(_str_ref3, "test");
}

#[test]
fn test_fmt_display_implementations() {
    let ident = Ident::new("Model.Var");
    assert_eq!(format!("{ident}"), "model·var");

    let ident_ref = ident.as_ref();
    assert_eq!(format!("{ident_ref}"), "model·var");

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

    // ----- Constraint 4: salsa::Update semantics on the handle -----

    #[test]
    fn salsa_update_reports_change_only_on_value_difference() {
        use salsa::Update;

        // Different value -> overwrite and report changed.
        let mut slot = CanonicalStorage::intern("old_value");
        let changed = {
            // SAFETY (test): `&mut slot` is a valid, owned `CanonicalStorage`;
            // we pass its pointer and a fresh owned value, matching the
            // `Update::maybe_update` contract.
            #[allow(unsafe_code)]
            unsafe {
                CanonicalStorage::maybe_update(
                    &mut slot as *mut _,
                    CanonicalStorage::intern("new_value"),
                )
            }
        };
        assert!(changed, "differing values must report a change");
        assert_eq!(slot.as_str(), "new_value");

        // Equal value -> no overwrite, report unchanged.
        let unchanged = {
            #[allow(unsafe_code)]
            unsafe {
                CanonicalStorage::maybe_update(
                    &mut slot as *mut _,
                    CanonicalStorage::intern("new_value"),
                )
            }
        };
        assert!(!unchanged, "equal values must report no change");
        assert_eq!(slot.as_str(), "new_value");
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
    /// Create from an already-canonicalized string (internal use only)
    #[allow(dead_code)]
    pub(crate) fn from_canonical_unchecked(s: String) -> Self {
        CanonicalDimensionName(CanonicalStorage::intern(&s))
    }

    /// Create from a raw string, canonicalizing it
    pub fn from_raw(s: &str) -> Self {
        CanonicalDimensionName(CanonicalStorage::intern(&canonicalize(s)))
    }

    /// Get the underlying canonical string
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }

    /// Convert to the legacy DimensionName type (for gradual migration)
    pub fn to_dimension_name(&self) -> DimensionName {
        self.0.as_str().to_owned()
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
    /// Create from an already-canonicalized string (internal use only)
    #[allow(dead_code)]
    pub(crate) fn from_canonical_unchecked(s: String) -> Self {
        CanonicalElementName(CanonicalStorage::intern(&s))
    }

    /// Create from a raw string, canonicalizing it
    pub fn from_raw(s: &str) -> Self {
        CanonicalElementName(CanonicalStorage::intern(&canonicalize(s)))
    }

    /// Get the underlying canonical string
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }

    /// Convert to the legacy ElementName type (for gradual migration)
    pub fn to_element_name(&self) -> ElementName {
        self.0.as_str().to_owned()
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
/// `PartialOrd`/`salsa::Update` delegate to that handle's manual impls (value
/// equality, value-based hash consistent with `Borrow<str>`, lexicographic
/// ordering), preserving the previous `String`-backed semantics.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, salsa::Update)]
pub struct Ident<State = Canonical> {
    inner: CanonicalStorage,
    _phantom: PhantomData<State>,
}

/// A borrowed identifier reference with state tracking
/// This is the key type that enables zero-copy substring operations
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Copy, Clone, PartialEq, Eq, Hash)]
pub struct IdentRef<'a, State = Canonical> {
    inner: &'a str,
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

    /// Get a borrowed reference to this identifier
    pub fn as_ref(&self) -> IdentRef<'_, Canonical> {
        IdentRef {
            inner: self.inner.as_str(),
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

    /// Create an identifier with array subscript notation
    pub fn with_subscript(&self, subscript: &str) -> Self {
        Ident {
            inner: CanonicalStorage::intern(&format!("{}[{}]", self.to_source_repr(), subscript)),
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

    /// Strip a prefix, returning a borrowed view if successful
    pub fn strip_prefix<'a>(&'a self, prefix: &str) -> Option<IdentRef<'a, Canonical>> {
        self.inner.as_str().strip_prefix(prefix).map(|s| IdentRef {
            inner: s,
            _phantom: PhantomData,
        })
    }
}

impl<'a> IdentRef<'a, Canonical> {
    /// Create from a string slice known to be canonical
    ///
    /// Note: Caller must guarantee the string is already canonical
    pub fn from_canonical_unchecked(s: &'a str) -> Self {
        IdentRef {
            inner: s,
            _phantom: PhantomData,
        }
    }

    /// Get the underlying string slice
    pub fn as_str(&self) -> &'a str {
        self.inner
    }

    /// Get as a CanonicalStr
    pub fn as_canonical_str(&self) -> CanonicalStr<'a> {
        CanonicalStr::from_canonical_unchecked(self.inner)
    }

    /// Convert to an owned Ident
    pub fn to_owned(&self) -> Ident<Canonical> {
        Ident {
            inner: CanonicalStorage::intern(self.inner),
            _phantom: PhantomData,
        }
    }

    /// Strip a prefix, maintaining the canonical guarantee
    pub fn strip_prefix(&self, prefix: &str) -> Option<IdentRef<'a, Canonical>> {
        self.inner.strip_prefix(prefix).map(|s| IdentRef {
            inner: s,
            _phantom: PhantomData,
        })
    }

    /// Convert canonical identifier to source code representation.
    ///
    /// Replaces middle dots (·) used internally for module hierarchy separators
    /// back to periods (.) for display in source code or user-facing output.
    pub fn to_source_repr(&self) -> Cow<'a, str> {
        canonical_to_source(self.inner)
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

impl<'a> AsRef<str> for IdentRef<'a, Canonical> {
    fn as_ref(&self) -> &str {
        self.inner
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

impl<'a> fmt::Display for IdentRef<'a, Canonical> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.inner)
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

#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub enum UnitError {
    DefinitionError(EquationError, Option<String>),
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
            UnitError::DefinitionError(err, details) => {
                if let Some(details) = details {
                    write!(f, "unit definition:{err} -- {details}")
                } else {
                    write!(f, "unit definition:{err}")
                }
            }
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

#[macro_export]
macro_rules! eqn_err(
    ($code:tt, $start:expr, $end:expr) => {{
        use $crate::common::{EquationError, ErrorCode};
        Err(EquationError{ start: $start, end: $end, code: ErrorCode::$code})
    }}
);

#[macro_export]
macro_rules! var_eqn_err(
    ($ident:expr, $code:tt, $start:expr, $end:expr) => {{
        use $crate::common::{EquationError, ErrorCode};
        Err(($ident, EquationError{ start: $start, end: $end, code: ErrorCode::$code}))
    }}
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

pub fn topo_sort<'out>(
    runlist: Vec<&'out Ident<Canonical>>,
    dependencies: &'out HashMap<Ident<Canonical>, BTreeSet<Ident<Canonical>>>,
) -> Vec<&'out Ident<Canonical>> {
    use std::collections::HashSet;

    let runlist_len = runlist.len();
    let mut result: Vec<&'out Ident<Canonical>> = Vec::with_capacity(runlist_len);
    let mut used: HashSet<&Ident<Canonical>> = HashSet::new();

    // We want to do a postorder, recursive traversal of variables to ensure
    // dependencies are calculated before the variables that reference them.
    // By this point, we have already errored out if we have e.g. a cycle
    fn add<'a>(
        dependencies: &'a HashMap<Ident<Canonical>, BTreeSet<Ident<Canonical>>>,
        result: &mut Vec<&'a Ident<Canonical>>,
        used: &mut HashSet<&'a Ident<Canonical>>,
        ident: &'a Ident<Canonical>,
    ) {
        if used.contains(ident) {
            return;
        }
        used.insert(ident);
        // An ident with no dependencies entry is a dangling reference -- skip it
        // rather than panicking. A dangling module reference (an empty or missing
        // `model_name`) on the legacy `from_salsa` path can leave such an ident
        // in the dependency set; it is not a real variable to place in the
        // runlist, so dropping it keeps this test-only path from crashing with an
        // "internal compiler error" on user-controllable input (GH #806). The
        // production salsa path uses `topo_sort_str` and rejects a
        // dangling/cyclic module graph up front, so valid runlists -- where every
        // ident has a deps entry -- are unaffected (result == runlist).
        if let Some(deps) = dependencies.get(ident) {
            for dep in deps.iter() {
                add(dependencies, result, used, dep)
            }
            result.push(ident);
        }
    }

    for ident in runlist.into_iter() {
        add(dependencies, &mut result, &mut used, ident);
    }

    // For a well-formed runlist every ident has a dependencies entry and every
    // dependency is itself in the runlist, so `result` holds exactly the runlist
    // idents (`result.len() == runlist_len`). A dangling reference (GH #806) is
    // skipped above, so the result may be shorter; we no longer assert equality
    // (it would re-introduce a panic on the bad input this guards against).
    debug_assert!(result.len() <= runlist_len);
    result
}
