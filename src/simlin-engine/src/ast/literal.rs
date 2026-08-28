// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::hash::{Hash, Hasher};

/// The numeric value of a `Const` node in any of the four AST layers, compared
/// by **bit pattern** rather than by IEEE equality.
///
/// `Expr0`..`Expr3` (and everything holding one:
/// `db::query::ParsedVariableResult`, the per-variable lowered projections,
/// `db::ltm::LtmEquation`,
/// `ltm_agg::AggNodesResult`, ...) derive `PartialEq`. Salsa decides whether to
/// *backdate* a re-executed tracked function's memo by comparing the old value
/// with the new one via `PartialEq` (`values_equal`). A bare `f64` makes that
/// comparison **non-reflexive**: `NaN != NaN`, so a value holding a NaN literal
/// never compares equal to itself, is never backdated, and reports "changed" on
/// every bit-identical re-execution -- taking the whole downstream query cone
/// with it. Every stdlib SMOOTH/DELAY/TREND template declares
/// `initial_value = NAN` and `db::sync` splices those into every project, so
/// this was reachable with no user action at all (GH #987, #981).
///
/// Comparing the bits fixes that at the root **of the AST**, for every consumer
/// of these ASTs at once, and it stays fixed structurally: a new `Const`-like
/// variant, or any new float-bearing field on a type the AST reaches, must use
/// this type, because the AST enums also derive `Eq` and a bare `f64` cannot
/// satisfy it. That is a compile error rather than a silently reintroduced
/// incrementality cliff, and it is enforced TRANSITIVELY -- adding an `f64` to
/// `ArrayView`, which `Expr3` reaches only indirectly, fails the same way.
///
/// **Bit comparison costs this layer nothing**, because both distinctions it
/// draws are unreachable in an AST literal by construction -- not accepted
/// tradeoffs, but distinctions that cannot occur:
///
/// * **NaN payloads.** There is exactly one NaN spelling at the language
///   surface, the lexer's `nan` keyword, and it yields one canonical
///   `f64::NAN`. A practitioner cannot author a payload, and nothing between the
///   parser and here performs arithmetic, so every NaN literal in an AST is
///   bit-identical to every other. Bit comparison distinguishes nothing that
///   exists. (`crate::float`'s module docs carry the domain reason this is the
///   right posture rather than a shortcut: a NaN is a diagnostic signal, not a
///   value with structure worth preserving.)
/// * **Signed zero.** `lexer::scan_number` accepts no leading sign (exponent
///   sign only), so `-0` parses as a NEGATION applied to the literal `0`, never
///   as a `Const` holding `-0.0`. The two spellings were already structurally
///   unequal. Every other producer of a `Literal` -- a dimension offset, a
///   dimension length, a subscript index, the `Literal::new(0.0)` stubs -- is
///   non-negative.
///
/// Both premises are tripwire tests rather than comments
/// (`literal_tests::plus_and_minus_zero_are_different_literals` and
/// `literal_ast_tests::a_negative_zero_literal_is_a_negation_not_a_signed_const`),
/// so if the lexer ever learns signed number tokens, someone finds out. And no
/// other consumer of these ASTs is affected either way: each reads the number
/// out with [`Literal::value`] and compares `f64`s with its own tolerance
/// (`mdl::writer::exprs_equal`, `ltm::polarity`'s sign tests, the compiler's
/// static index resolution).
///
/// The API is exactly two operations, [`Literal::new`] and [`Literal::value`].
/// The accessor is deliberately named rather than a `Deref` to `f64`: at a call
/// site that reads the number out, bit-equality is not what is wanted, and a
/// silent deref would make the distinction invisible.
///
/// # Float equality in this crate: the position, and the types still outside it
///
/// **Wherever a float feeds a cache key, we want BIT equality.** The reason is
/// domain, not taste, and it lives in `crate::float`'s module docs: a NaN in a
/// system dynamics model is a failure signal a practitioner has to trace by
/// hand, not a value whose structure is worth preserving. Nothing distinguishes
/// NaN payloads or the two signed zeros meaningfully, and nothing can author
/// them, so bit comparison never separates two floats a user could tell apart --
/// while IEEE `==` makes a value unequal to its own rebuild, which is what
/// breaks caching.
///
/// `Literal` implements that position for AST literals. It is not implemented
/// for:
///
/// * the compiled bytecode types -- `bytecode::ByteCode::literals`,
///   `bytecode::ByteCodeContext::graphical_functions`, `results::Specs`, and
///   their symbolic twins `compiler::symbolic::SymbolicByteCode::literals` /
///   `PerVarBytecodes::graphical_functions` (GH #642);
/// * `variable::Table`'s `x`/`y: Vec<f64>` lookup points and its two
///   `datamodel::GraphicalFunctionScale`s, which ride on `VarKind::Aux` into
///   the same parse and per-variable lowering memos this type serves.
///
/// Both keep the derived, IEEE-based `PartialEq`. That is an accepted state, not
/// an open defect -- **what is accepted is the cost of NOT converting them, not
/// a semantic property worth keeping.** Stated exactly, because IEEE equality
/// diverges from bit equality in BOTH directions:
///
/// 1. **Stricter on NaN -> a missed backdate (perf only).** `NaN != NaN`, so a
///    bit-identical rebuild reports "changed" and the downstream cone
///    re-executes. This is #642, closed on this reasoning. Note the corrected
///    premise, so nobody re-litigates a false one: #642's body argues it is
///    benign because "the only consumer, `compile_project_incremental`, is
///    non-tracked". That is WRONG one level down -- `PerVarBytecodes`
///    (`compiler/symbolic.rs`) is the value of the *tracked*
///    `compile_var_fragment`, which the *tracked* `assemble_module` reads. The
///    missed backdate is real; we accept it because assembly re-executes on
///    nearly any model edit anyway, so the marginal cost is about one extra
///    `assemble_module`.
/// 2. **Looser on signed zero -> a theoretical stale read.** Unlike the AST,
///    `-0.0` IS reachable in a compiled literal pool: `compiler::fold` folds
///    `0 * -1` to `-0.0` (bits `0x8000_0000_0000_0000`), and `1.0 / -0.0` is
///    `-inf`, so the sign is observable. `vec![-0.0] == vec![0.0]`, so a pool
///    differing from its rebuild ONLY in a zero's sign compares equal, salsa
///    declines to overwrite, and the older pool is kept. Measured, not reasoned.
///    Reaching it needs a model edit that changes only a folded zero's sign in
///    an otherwise byte-identical fragment and then divides by it; it is
///    pre-existing and unreached by the corpus. It is NOT what #642 describes,
///    and it points the same way: converting those types would close both
///    directions at once and give up nothing, since bit comparison is free here
///    for the reasons above.
///
/// **What would change the decision** (judgements about cost go stale, so name
/// the trigger): bytecode-fragment recompilation showing up in an interactive
/// profile, or any of these types acquiring a further tracked consumer that
/// makes the missed backdate cost more than one assembly. Either is a reason to
/// revisit; "it feels untidy" is not.
#[derive(Copy, Clone)]
pub struct Literal(f64);

impl Literal {
    /// Wrap a parsed numeric literal.
    pub const fn new(value: f64) -> Self {
        Literal(value)
    }

    /// The numeric value, for arithmetic, folding, and code generation.
    ///
    /// Everything downstream of equality wants the plain `f64`; only comparison
    /// and hashing go through the bit pattern.
    pub const fn value(self) -> f64 {
        self.0
    }
}

/// Forwards to the inner `f64` so that a `Const` prints as `Const("NaN", NaN,
/// ..)` rather than gaining a wrapper layer in every AST debug dump.
///
/// Unconditional rather than gated on `debug-derive` (the crate's rule for new
/// types) under that gate's documented exception for types appearing in
/// `assert_eq!` -- `literal_tests` compares `Literal`s directly, and a
/// one-field forwarding impl costs nothing.
impl std::fmt::Debug for Literal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Debug::fmt(&self.0, f)
    }
}

/// Bit-pattern equality: reflexive on NaN, and therefore a genuine equivalence
/// relation (hence the [`Eq`] impl below, which is what lets the AST enums
/// derive `Eq` and makes reflexivity a compile-checked property of every future
/// variant).
impl PartialEq for Literal {
    fn eq(&self, other: &Self) -> bool {
        self.0.to_bits() == other.0.to_bits()
    }
}

impl Eq for Literal {}

/// Consistent with [`PartialEq`]: equal literals hash equal because both are
/// defined on the same bit pattern.
///
/// Nothing hashes a `Literal` today -- none of the AST enums derives `Hash`.
/// It exists so that the obvious future change does not silently break the
/// `Hash`/`Eq` contract: `ltm_agg`'s synthetic-agg dedup map is keyed by printed
/// equation text precisely *because* `Expr2` is not `Hash`, and a `Hash` derived
/// over a bare `f64` would disagree with bit equality on NaN.
impl Hash for Literal {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.to_bits().hash(state);
    }
}

#[cfg(test)]
mod literal_tests {
    use super::Literal;
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    fn hash_of(lit: Literal) -> u64 {
        let mut hasher = DefaultHasher::new();
        lit.hash(&mut hasher);
        hasher.finish()
    }

    /// The whole point: `f64`'s own equality is not reflexive on NaN, and this
    /// type's is.
    #[test]
    fn a_nan_literal_equals_itself() {
        // `black_box` keeps this from being a NaN comparison the compiler can
        // see through and lint; the non-reflexivity of bare `f64` equality is
        // exactly the defect being asserted, not an accident.
        let bare = std::hint::black_box(f64::NAN);
        assert!(
            bare != std::hint::black_box(f64::NAN),
            "bare f64 equality is not reflexive on NaN -- the defect this type exists to fix"
        );
        assert_eq!(Literal::new(f64::NAN), Literal::new(f64::NAN));
        assert_eq!(
            hash_of(Literal::new(f64::NAN)),
            hash_of(Literal::new(f64::NAN))
        );
    }

    /// The one semantic change: bit equality separates the two zeros, which
    /// IEEE equality does not. Asserted in BOTH directions so that neither the
    /// separation nor the numeric equality it departs from can move silently.
    #[test]
    fn plus_and_minus_zero_are_different_literals() {
        assert_eq!(0.0_f64, -0.0_f64, "IEEE equality does not distinguish them");
        assert_ne!(Literal::new(0.0), Literal::new(-0.0));
        assert_eq!(Literal::new(-0.0), Literal::new(-0.0));
        assert_eq!(Literal::new(0.0), Literal::new(0.0));
        assert_eq!(
            Literal::new(-0.0).value().to_bits(),
            (-0.0_f64).to_bits(),
            "the value is passed through unchanged; only comparison differs"
        );
    }

    /// Ordinary literals are unaffected -- the change must not make two
    /// equation texts that used to compare equal stop doing so.
    #[test]
    fn ordinary_literals_compare_as_before() {
        for value in [0.0_f64, 1.0, -1.0, 1e-300, 1e300, f64::INFINITY, 0.1 + 0.2] {
            assert_eq!(Literal::new(value), Literal::new(value));
            assert_eq!(hash_of(Literal::new(value)), hash_of(Literal::new(value)));
        }
        assert_ne!(Literal::new(1.0), Literal::new(2.0));
        assert_ne!(Literal::new(f64::INFINITY), Literal::new(f64::NEG_INFINITY));
    }
}

/// The same property one level up, through the PUBLIC parse API: an `Expr0`
/// built twice from identical NaN-bearing text is equal to itself and backdates.
///
/// `Expr0` is retained by `db::query::ParsedVariableResult` (GH #987) and by
/// `db::ltm::LtmArm` (GH #981). The higher layers are covered where they are
/// consumed: `Expr2` by the per-variable lowering query and
/// `ltm_agg::tests::a_nan_literal_in_a_reducer_does_not_defeat_agg_backdating`,
/// and `LtmEquation` by `db::ltm::equation`'s own probe.
#[cfg(test)]
mod literal_ast_tests {
    use crate::ast::Expr0;
    use crate::lexer::LexerType;

    fn parse(eqn: &str) -> Expr0 {
        Expr0::new(eqn, LexerType::Equation)
            .expect("the fixture equations parse")
            .expect("the fixture equations are non-empty")
    }

    #[test]
    fn two_parses_of_a_nan_bearing_equation_are_equal() {
        assert_eq!(parse("1 + nan"), parse("1 + nan"));
        assert_eq!(parse("nan"), parse("nan"));
        assert_ne!(
            parse("1 + nan"),
            parse("2 + nan"),
            "a genuine difference must still be visible through a NaN-bearing tree"
        );
    }

    /// Why the `+0.0`/`-0.0` divergence introduced by bit comparison is not
    /// reachable from equation text: `lexer::scan_number` never consumes a
    /// leading sign, so `-0` parses as a NEGATION of the literal `0` and the
    /// `Const` itself always holds a positive zero. The two spellings therefore
    /// produce structurally different trees and were already unequal.
    ///
    /// This is a tripwire, not a contract: if the lexer ever learns signed
    /// number tokens, the divergence becomes reachable and this reds.
    #[test]
    fn a_negative_zero_literal_is_a_negation_not_a_signed_const() {
        use crate::ast::UnaryOp;
        let neg = parse("-0");
        match &neg {
            Expr0::Op1(UnaryOp::Negative, inner, _) => match inner.as_ref() {
                Expr0::Const(text, value, _) => {
                    assert_eq!(text, "0");
                    assert!(
                        value.value().is_sign_positive(),
                        "the lexer hands the parser an unsigned number token"
                    );
                }
                other => panic!("expected a Const under the negation, got {other:?}"),
            },
            other => panic!("expected `-0` to parse as a negation, got {other:?}"),
        }
        assert_ne!(
            neg,
            parse("0"),
            "the two spellings differ structurally, independent of float equality"
        );
    }
}
