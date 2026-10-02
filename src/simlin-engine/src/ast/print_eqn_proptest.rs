// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! `parse(print_eqn(e)) == e` over the whole of `Expr0`
//! ([`expr_strategy`](super::expr0_strategy::expr_strategy): every operator,
//! every index form, calls of zero to three arguments, the number spellings
//! the lexer reads) and over a NAME POOL that reaches every clause of
//! [`needs_quoting`].
//!
//! The MDL writer's fixpoint proptest (`mdl::writer_proptest`) re-reads with
//! `mdl::parser`, whose binary precedence table is inverted (GH #914), so its
//! generator is restricted to arithmetic. That leaves `Not`, `Neq`, `Transpose`,
//! `Mod`, the comparisons, and `and`/`or` -- most of `print_eqn`'s surface, and
//! three of the four #913 shapes -- unexercised by any property. This module
//! re-parses with the XMILE grammar, which `print_eqn` targets, so it can cover
//! everything.
//!
//! "The same tree" is equality of canonical forms under [`Aliases::Spelling`]:
//! `print_ident` canonicalizes as it prints, and a quoted name comes back from
//! the parser with its quotes still attached to the `RawIdent`, so RAW ident
//! equality is not the property `print_eqn` promises -- CANONICAL ident
//! equality is.

use proptest::prelude::*;

use super::expr0_strategy::{NAME_POOL, expr_strategy};
use super::*;
use crate::builtins::BuiltinSig;
use crate::lexer::{LexerType, Token};

/// Does `text` lex as ONE identifier covering the whole input?
///
/// This is the lexer-side statement of "bare-spellable", derived by running
/// the lexer rather than by restating its character classes -- which is the
/// point: `needs_quoting` restates them, and a restatement can be incomplete
/// (GH #976).
fn lexes_as_one_whole_ident(text: &str) -> bool {
    let mut lexer = crate::lexer::Lexer::new(text, LexerType::Equation);
    match (lexer.next(), lexer.next()) {
        (Some(Ok((start, Token::Ident(word), end))), None) => {
            start == 0 && end == text.len() && word == text
        }
        _ => false,
    }
}

/// Does `text` PARSE as a reference to the variable of that name? Stronger
/// than lexing as one identifier: `Expr0::new` reads a bare zero-argument
/// builtin name (`pi`, `time`) as the builtin's call.
fn parses_as_a_reference_to_itself(text: &str) -> bool {
    matches!(
        Expr0::new(text, LexerType::Equation),
        Ok(Some(Expr0::Var(name, _))) if name.as_str() == text
    )
}

/// Names in the shapes canonicalization can produce, plus arbitrary short
/// strings over the alphabet those shapes draw from. `"` is excluded, and
/// [`a_canonical_name_containing_a_quote_is_unspellable`] says why.
fn name_strategy() -> impl Strategy<Value = String> {
    prop_oneof![
        prop::sample::select(&NAME_POOL[..]).prop_map(str::to_string),
        "[a-zA-Z0-9_·⁚$→]{1,6}",
    ]
}

/// The one name shape `needs_quoting` cannot rescue, pinned rather than
/// quietly excluded from the property above.
///
/// `canonicalize` preserves an embedded `"` (an XMILE `name="a&quot;b"`
/// reaches the compiler as the canonical `a"b`), and `Lexer::quoted_ident`
/// terminates on the FIRST `"` with no escape sequence of any kind -- so
/// `a"b` has no bare spelling AND no quoted spelling. `needs_quoting`
/// correctly says "quote it"; there is simply nothing to say.
///
/// Such a name IS reachable -- the XMILE reader admits
/// `<aux name="a&quot;b">` with zero diagnostics -- and a rename TO one would
/// persist a corrupted model through the same `patch.rs` path as GH #976: the
/// rename reprints every dependent equation, so `c = a + 1` would become
/// `c = "x"y" + 1` and a valid model would stop compiling with
/// `UnclosedQuotedIdent`. That direction is refused at the front door by
/// `patch::apply_rename_variable`, which is where the loudness belongs; giving
/// the name a spelling instead would mean an escape in the lexer's
/// quoted-identifier rule, a language change and not a printer one.
///
/// So what remains is exactly this: a name that can be DEFINED (by either
/// reader) but never REFERENCED. This test is the characterization pin for
/// that state, and it reds if the lexer ever grows an escape -- which is
/// when `print_ident` needs revisiting.
#[test]
fn a_canonical_name_containing_a_quote_is_unspellable() {
    let canonical = canonicalize("a\"b");
    assert_eq!(
        "a\"b",
        canonical.as_ref(),
        "the quote survives canonicalization"
    );
    assert!(needs_quoting(&canonical));
    assert!(!lexes_as_one_whole_ident(&canonical), "no bare spelling");

    let printed = print_ident(&canonical);
    assert_eq!("\"a\"b\"", printed);
    assert!(
        !lexes_as_one_whole_ident(&printed),
        "no quoted spelling either: the lexer has no escape inside a quoted ident"
    );
}

/// Every zero-argument builtin's name and alias, from the signature table, so
/// a builtin added there has a row here.
fn nullary_builtin_names() -> Vec<&'static str> {
    BuiltinSig::ALL
        .iter()
        .filter(|sig| sig.max_args == Some(0))
        .flat_map(|sig| std::iter::once(sig.name).chain(sig.aliases.iter().copied()))
        .collect()
}

/// A model refers to a VARIABLE named like a zero-argument builtin by quoting
/// the name (`"pi" * r`); bare, the name is the builtin's call. The printer
/// keeps the quotes, so the reference reads back as the variable. Printed
/// bare, a rename that reprints the equation would replace the variable's
/// value with the constant.
#[test]
fn a_reference_to_a_variable_named_like_a_builtin_prints_quoted() {
    let names = nullary_builtin_names();
    assert!(names.contains(&"pi") && names.contains(&"dt"), "{names:?}");
    for name in names {
        for spelling in [name.to_string(), name.to_uppercase()] {
            assert!(needs_quoting(&spelling), "{spelling}");
            let text = format!("\"{spelling}\" * 2");
            let parsed = Expr0::new(&text, LexerType::Equation)
                .expect("the equation parses")
                .expect("it is not empty");
            let printed = print_eqn(&parsed);
            assert_eq!(printed, format!("\"{name}\" * 2"));
            let reparsed = Expr0::new(&printed, LexerType::Equation)
                .expect("the printed equation parses")
                .expect("it is not empty");
            assert!(
                matches!(&reparsed, Expr0::Op2(_, l, _, _) if matches!(**l, Expr0::Var(..))),
                "`{text}` printed as `{printed}`, which does not read the variable"
            );
            assert!(
                parsed.canonical(Aliases::Spelling) == reparsed.canonical(Aliases::Spelling),
                "`{text}` printed as `{printed}`"
            );
            // The builtin itself is printed as its call.
            let call = Expr0::new(name, LexerType::Equation)
                .expect("the bare name parses")
                .expect("it is not empty");
            assert_eq!(print_eqn(&call), format!("{name}()"));
        }
    }
}

proptest! {
    #[test]
    fn print_eqn_roundtrips_over_the_full_operator_set(expr in expr_strategy()) {
        let printed = print_eqn(&expr);
        let reparsed = Expr0::new(&printed, LexerType::Equation);
        prop_assert!(
            matches!(reparsed, Ok(Some(_))),
            "print_eqn output did not re-parse: {printed:?} ({reparsed:?})"
        );
        let reparsed = reparsed.unwrap().unwrap();
        prop_assert!(
            expr.canonical(Aliases::Spelling) == reparsed.canonical(Aliases::Spelling),
            "print_eqn output {} re-parsed to a DIFFERENT tree:\n{:?}\nwas\n{:?}",
            printed,
            reparsed.strip_loc(),
            expr.clone().strip_loc()
        );
    }

    /// The completeness guard for [`needs_quoting`]: a name it calls
    /// bare-spellable must ACTUALLY read back as a reference to that name --
    /// lex as one identifier, and parse as the variable rather than as a
    /// builtin's call. Stating it against the lexer and the parser (rather
    /// than against a second copy of their rules) is what makes the predicate
    /// checkable: a leading digit, a keyword and a zero-argument builtin are
    /// each a rule of theirs that a restatement in the printer can miss.
    ///
    /// The converse is deliberately not asserted. Over-quoting is always
    /// safe, and `ltm_augment::quote_ident` relies on that to keep quoting
    /// `·`-qualified names the lexer would happily read bare.
    #[test]
    fn a_bare_spellable_name_reads_back_as_that_name(name in name_strategy()) {
        let canonical = canonicalize(&name);
        prop_assume!(!canonical.is_empty());
        if !needs_quoting(&canonical) {
            prop_assert!(
                lexes_as_one_whole_ident(&canonical),
                "needs_quoting says `{}` can be spelled bare, but the lexer does not \
                 read it as a single identifier",
                canonical
            );
            prop_assert!(
                parses_as_a_reference_to_itself(&canonical),
                "needs_quoting says `{}` can be spelled bare, but the parser does not \
                 read it as a reference to that variable",
                canonical
            );
        }
    }
}
