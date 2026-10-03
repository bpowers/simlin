// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! MDL equation text and sketch writer.
//!
//! Converts `Expr0` AST nodes into Vensim MDL-format equation text and
//! serializes datamodel views to MDL sketch format.
//! The key transformation vs the XMILE printer (`ast::print_eqn`) is
//! converting canonical (underscored, lowercase) identifiers back to
//! MDL-style spaced names and using MDL operator syntax.

use std::collections::{HashMap, HashSet};
use std::fmt::Write;

use super::builtins::{SymbolClass, classify_symbol, to_lower_space};
use super::convert::is_external_data_placeholder;
use crate::ast::{BinaryOp, Expr0, IndexExpr0, NodeShape, UnaryOp, Visitor, paren_if_necessary};
use crate::builtins::UntypedBuiltinFn;
use crate::common::{Error, ErrorCode, ErrorKind, Result};
use crate::datamodel::view_element::{self, LinkPolarity, LinkShape};
use crate::datamodel::{self, DimensionElements, Equation, GraphicalFunction, View, ViewElement};
use crate::lexer::LexerType;
use unicode_xid::UnicodeXID;

/// Replace underscores with spaces -- the reverse of `space_to_underbar()`.
fn underbar_to_space(name: &str) -> String {
    name.replace('_', " ")
}

/// The section-terminator sequences that end the equations section early if
/// they appear raw inside ANY free-text field, truncating the model. The
/// canonical form uses three backslashes (`\\\---///` / `///---\\\`), but the
/// lexer's `check_eq_end` (lexer.rs) ALSO accepts a two-backslash variant
/// (`\\---///` / `///---\\`), which the units field -- being lexer-tokenized --
/// is vulnerable to. `find_comment_terminator` only matches the three-backslash
/// form, so comments are only vulnerable to the canonical variant, but the
/// sanitizer neutralizes all four regardless of field for simplicity.
///
/// A three-backslash run contains the two-backslash run as a substring, so the
/// pre-pass MUST replace the three-backslash forms FIRST (longest match) or a
/// canonical run gets mangled into a stray backslash.
const SECTION_TERMINATOR_OPEN: &str = "\\\\\\---///";
const SECTION_TERMINATOR_CLOSE: &str = "///---\\\\\\";
const SECTION_TERMINATOR_OPEN_SHORT: &str = "\\\\---///";
const SECTION_TERMINATOR_CLOSE_SHORT: &str = "///---\\\\";

/// How a free-text field treats internal line breaks.
#[derive(Clone, Copy)]
enum FreeTextLineMode {
    /// Preserve internal line breaks, normalized to canonical LF -- a multi-line
    /// comment / group-doc field is legal.
    Multiline,
    /// Collapse every line-break run to a single space -- the field must occupy
    /// one physical line (a group-banner name, a `22:` settings token) or treats
    /// line breaks as insignificant whitespace (a tokenized units field).
    SingleLine,
}

/// The single choke point that neutralizes MDL structural characters in
/// modeler-authored free text before it is embedded in the file. Every
/// free-text sink (variable units/documentation, group name/doc, `22:`
/// unit-equivalence tokens) routes through this so the policy lives in one
/// place. GH #849.
///
/// ## What the reader treats as structure (see reader.rs / lexer.rs / settings.rs)
///
/// None of these fields are escaped on read, so any character the reader reads
/// as structure corrupts the file if emitted raw:
///
/// - `|` (`Token::Pipe`) terminates a variable entry. In a `~`-comment it is
///   found by `reader::find_comment_terminator` scanning RAW bytes; in a units
///   field it is `SectionEnd::Pipe`; in a group banner `lexer::try_group_star`
///   stops its skip-to-terminator scan on it. A raw `|` anywhere in a comment,
///   units, group name, or group doc ends the construct early and the trailing
///   remainder re-parses as phantom variables (the confirmed repro).
/// - `~` (`Token::Tilde`) separates a variable's equation / units / comment.
///   A raw `~` inside the *units* field is read as the second tilde and ends
///   units early. (A `~` inside a `~`-comment is legal prose and is preserved,
///   apart from the reader's trailing `:SUP`/`:SUPPLEMENTARY` flag heuristic --
///   which is a feature, not corruption -- so comments do NOT list `~`.)
/// - `\\\---///` and `///---\\\` terminate the whole equations section; either
///   run inside any free-text field truncates the model.
/// - `,` separates the name / aliases / equation of a `22:` unit-equivalence
///   line (`settings::parse_unit_equivalence`); a line break ends the line.
/// - A line break ends a group-banner name line (`try_group_star` stops the
///   name at whitespace) and a `22:` settings line, so those fields are
///   single-line.
///
/// ## Policy
///
/// Line endings are normalized LOSSLESSLY to canonical LF (`\r\n` and lone
/// `\r` -> `\n`). `write_project` runs a single LF->CRLF pass at the very end,
/// so normalizing here is what makes a field carrying `\r` (from a CRLF source
/// or a prior round trip) a fixpoint instead of accumulating a carriage return
/// every write (GH #849 idempotence).
///
/// Structural characters that have no escape in MDL free text are replaced with
/// a documented safe substitute (never dropped silently): `|` -> `/` and the
/// section-terminator runs plus the caller's field-specific separators
/// (`extra_forbidden`: `~` in units, `,` in a `22:` token) -> a space. `|` maps
/// to a NON-whitespace substitute so a field-final `|` cannot become a
/// trim-sensitive trailing space in the reader-trimmed comment field (which
/// would break idempotence). Substitution is preferred over erroring because
/// the richer writer-diagnostics channel is a separate change (GH #856), so
/// `project_to_mdl` keeps returning `Result<String>` here.
fn sanitize_free_text(raw: &str, line_mode: FreeTextLineMode, extra_forbidden: &[char]) -> String {
    // Neutralize the multi-char section-terminator runs first: the per-char
    // pass below cannot recognize them, and replacing them with a space cannot
    // re-form either run (neither contains a space).
    // Replace the three-backslash (canonical) runs BEFORE the two-backslash
    // variants: a canonical run contains the short run as a substring, so the
    // reverse order would leave a stray backslash behind.
    let pre = if raw.contains('\\') && (raw.contains("---///") || raw.contains("///---")) {
        raw.replace(SECTION_TERMINATOR_OPEN, " ")
            .replace(SECTION_TERMINATOR_CLOSE, " ")
            .replace(SECTION_TERMINATOR_OPEN_SHORT, " ")
            .replace(SECTION_TERMINATOR_CLOSE_SHORT, " ")
    } else {
        raw.to_owned()
    };

    let mut out = String::with_capacity(pre.len());
    let mut prev_was_break = false;
    let mut chars = pre.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            // Normalize CRLF and lone CR to the same break handling as LF; a
            // following LF is consumed so CRLF collapses to a single break.
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                push_line_break(&mut out, line_mode, &mut prev_was_break);
            }
            '\n' => push_line_break(&mut out, line_mode, &mut prev_was_break),
            '|' => {
                out.push('/');
                prev_was_break = false;
            }
            c if extra_forbidden.contains(&c) => {
                out.push(' ');
                prev_was_break = false;
            }
            c => {
                out.push(c);
                prev_was_break = false;
            }
        }
    }
    out
}

/// Emit a normalized line break for `sanitize_free_text`: a canonical LF for a
/// multi-line field, or a single collapsed space for a single-line field.
fn push_line_break(out: &mut String, line_mode: FreeTextLineMode, prev_was_break: &mut bool) {
    match line_mode {
        FreeTextLineMode::Multiline => {
            // Preserve internal breaks exactly (do not collapse blank lines).
            out.push('\n');
        }
        FreeTextLineMode::SingleLine => {
            if !*prev_was_break {
                out.push(' ');
                *prev_was_break = true;
            }
        }
    }
}

fn is_mdl_quoted_ident(name: &str) -> bool {
    let bytes = name.as_bytes();
    bytes.len() >= 2 && bytes[0] == b'"' && bytes[bytes.len() - 1] == b'"'
}

/// Check whether an MDL identifier requires quoting after underscore->space conversion.
/// Spaces are allowed in bare MDL names, but characters outside identifier classes
/// (for example `$`, `/`, `|`) must be quoted.
fn needs_mdl_quoting(name: &str) -> bool {
    if name.is_empty() || name != name.trim() {
        return true;
    }
    // The reader takes a bare name it knows as a function (or as WITH LOOKUP,
    // TABBED ARRAY or a GET call) for the start of that construct, whatever
    // follows it, so a variable of that name is written quoted. Whether
    // Vensim reserves these names is unverified; a quoted name is a name
    // either way.
    if !matches!(classify_symbol(name), SymbolClass::Regular) {
        return true;
    }

    let mut chars = name.chars();
    match chars.next() {
        None => return true,
        Some(c) if !UnicodeXID::is_xid_start(c) && c != '_' => return true,
        _ => {}
    }

    // The MDL lexer reads an apostrophe after a name's first character as
    // part of the name (`mdl::lexer::RawLexer::is_symbol_char`), and Vensim
    // writes such a name bare (`DimC'` in
    // `test/sdeverywhere/models/arrays_cname`); a leading one opens a
    // literal.
    for c in chars {
        if c == ' ' || c == '\'' {
            continue;
        }
        if !UnicodeXID::is_xid_continue(c) && c != '_' {
            return true;
        }
    }

    false
}

/// Spell a name's text as the interior of an MDL quoted name.
///
/// The MDL lexer reads a backslash inside quotes as the start of a two-char
/// escape and keeps the pair as written, which is also how an imported name
/// stores it: `"a \"b\" c"` imports as the name `a \"b\" c`. So a backslash and
/// the character after it are written as they stand, a bare quote gains its
/// backslash, and a real newline becomes the two-char `\n` display newline.
/// That makes the spelling a fixed point -- spelling a spelled name changes
/// nothing, so a name read back from the file is written as it was -- where
/// doubling every backslash grew the escaping on each save. A trailing
/// backslash is doubled so it cannot escape the closing quote. What Vensim
/// makes of a backslash before anything but a quote or `n` is unverified.
fn escape_mdl_quoted_ident(name: &str) -> String {
    let mut escaped = String::with_capacity(name.len());
    let mut chars = name.chars();
    while let Some(c) = chars.next() {
        match c {
            // Literal newlines must become the two-character escape `\n`.
            '\n' => escaped.push_str("\\n"),
            '\\' => match chars.next() {
                Some(next) => {
                    escaped.push('\\');
                    escaped.push(next);
                }
                None => escaped.push_str("\\\\"),
            },
            '"' => escaped.push_str("\\\""),
            _ => escaped.push(c),
        }
    }
    escaped
}

/// Format a canonical identifier for MDL output, preserving spaces and
/// adding quotes when the bare form would not round-trip through MDL parsing.
///
/// A display newline -- the two-character `\n` the datamodel stores, or a
/// real newline -- is written as the `\n` escape inside a quoted name, as
/// Vensim itself writes such a name in the equations and the sketch alike
/// (`"Stock with \n Newline Character"` in
/// `test/test-models/tests/special_characters/test_special_variable_names.mdl`),
/// and the reader keeps the escape, so the name reads back as the same name.
fn format_mdl_ident(name: &str) -> String {
    // An already-quoted identifier is literal to Vensim -- its interior is
    // verbatim. Detect it BEFORE any transformation: running
    // `underbar_to_space` over the whole string would turn interior
    // underscores into spaces (changing which variable Vensim resolves,
    // e.g. `"rate_of_change!"` vs `"rate of change!"`) and re-escaping would
    // grow the escaping each pass. Pass it through unchanged (#846), but for
    // a real line break, written as the `\n` escape (raw, it would end the
    // equation), and a final backslash, doubled so it cannot escape the
    // closing quote.
    if is_mdl_quoted_ident(name) {
        let mut inner = name[1..name.len() - 1]
            .replace("\r\n", "\\n")
            .replace('\n', "\\n");
        if inner.ends_with('\\') && !inner.ends_with("\\\\") {
            inner.push('\\');
        }
        return format!("\"{inner}\"");
    }
    let display = underbar_to_space(name);
    if needs_mdl_quoting(&display) {
        format!("\"{}\"", escape_mdl_quoted_ident(&display))
    } else {
        display
    }
}

/// Collapse a display newline -- the two-character `\n` or a real newline --
/// and the whitespace around it to a single space, so `Stock with \n Newline`
/// becomes `Stock with Newline`, not a run of spaces the reader would take for
/// a different name. A name's own leading or trailing space stays.
fn collapse_display_newlines(name: &str) -> String {
    let lines = split_display_lines(name);
    let last = lines.len() - 1;
    lines
        .iter()
        .enumerate()
        .map(|(i, line)| {
            // Only the whitespace at a break goes (an ident spells a space
            // `_`); a name's own leading or trailing space is part of it.
            let is_space = |c: char| c.is_whitespace() || c == '_';
            let line = if i > 0 {
                line.trim_start_matches(is_space)
            } else {
                line
            };
            if i < last {
                line.trim_end_matches(is_space)
            } else {
                line
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Whether `name` holds a display newline.
fn has_display_newline(name: &str) -> bool {
    name.contains("\\n") || name.contains('\n')
}

/// `model`'s views with each element named as its variable is, one space
/// between words (`single_spaced`): an element keeps a display newline where
/// its variable's name holds one, and drops it where the variable's name has
/// none (a Stella model breaks a name over two
/// lines in its diagram alone, `Maximum\nfishery size` drawing
/// `maximum_fishery_size`).
///
/// The equations and the sketch are written from these names, and the MDL
/// reader, like Vensim, links a sketch element to the variable its name
/// spells, so the two have to spell one name -- the variable's, which is what
/// reads back. The engine's names fold a break into the space around it, so
/// both spellings name one variable before the save.
fn views_named_as_defined(model: &datamodel::Model) -> Vec<View> {
    let broken: HashSet<String> = model
        .variables
        .iter()
        .filter(|var| has_display_newline(var.get_ident()))
        .map(|var| crate::common::canonicalize(var.get_ident()).into_owned())
        .collect();
    // The name an element is written under, when that is not the name it has.
    let named_as_defined = |name: &str| {
        let mut written = name.to_string();
        if has_display_newline(&written)
            && !broken.contains(crate::common::canonicalize(&written).as_ref())
        {
            written = collapse_display_newlines(&written);
        }
        written = single_spaced(&written);
        (written != name).then_some(written)
    };
    let mut views = model.views.clone();
    for view in &mut views {
        let View::StockFlow(sf) = view;
        sf.elements.update(|element| match element {
            ViewElement::Aux(aux) => named_as_defined(&aux.name).map(|name| {
                ViewElement::Aux(view_element::Aux {
                    name,
                    ..aux.clone()
                })
            }),
            ViewElement::Stock(stock) => named_as_defined(&stock.name).map(|name| {
                ViewElement::Stock(view_element::Stock {
                    name,
                    ..stock.clone()
                })
            }),
            ViewElement::Flow(flow) => named_as_defined(&flow.name).map(|name| {
                ViewElement::Flow(view_element::Flow {
                    name,
                    ..flow.clone()
                })
            }),
            ViewElement::Link(_)
            | ViewElement::Module(_)
            | ViewElement::Alias(_)
            | ViewElement::Cloud(_)
            | ViewElement::Group(_) => None,
        });
    }
    views
}

/// Split a display name on its line breaks -- the literal two-character `\n`
/// XMILE name attributes use (`Maximum\nfishery size`), or a real newline
/// character. Always returns at least one segment.
///
/// Used when sizing a sketch element's box to the modeler's chosen multi-line
/// layout.
fn split_display_lines(name: &str) -> Vec<String> {
    name.replace("\\n", "\n")
        .split('\n')
        .map(str::to_string)
        .collect()
}

/// `name` with each run of spaces or tabs one space and none at either end:
/// a label wrapped after a space (`fractional \ngrowth rate`, whose break
/// XML reads as a second space) names a variable whose words one space
/// separates, and the reader matches names as written. A display newline
/// escape is kept. What Vensim does with repeated spaces inside a name is
/// unverified.
fn single_spaced(name: &str) -> String {
    name.split([' ', '\t'])
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Build a mapping from canonical variable ident to display name (with
/// original casing, spaces instead of underscores) by walking view elements.
///
/// The first occurrence of a name wins, so if a variable appears in multiple
/// views the first view's casing is used.
fn build_display_name_map(views: &[View]) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for view in views {
        let View::StockFlow(sf) = view;
        for element in &sf.elements {
            let name = match element {
                ViewElement::Aux(a) => &a.name,
                ViewElement::Stock(s) => &s.name,
                ViewElement::Flow(f) => &f.name,
                _ => continue,
            };
            let canonical = crate::common::canonicalize(name).into_owned();
            map.entry(canonical)
                .or_insert_with(|| underbar_to_space(name));
        }
    }
    map
}

/// The display name for an ident: a view element's spelling of it, or the
/// ident itself when no view element draws it.
fn display_name_for_ident(ident: &str, display_names: &HashMap<String, String>) -> String {
    match display_names.get(crate::common::canonicalize(ident).as_ref()) {
        Some(name) if needs_mdl_quoting(name) => {
            format!("\"{}\"", escape_mdl_quoted_ident(name))
        }
        Some(name) => name.clone(),
        None => format_mdl_ident(ident),
    }
}

/// Arrayed element keys encode multidimensional indices as comma-separated
/// canonical names (for example `c,a,f`). Preserve tuple structure so MDL
/// parsers can split indices, and format each token independently: as the
/// dimension of `dims` at its position spells the element
/// ([`WriterContext::declared_element`]), so the modeler's spelling survives a
/// save, and a position over an indexed dimension, which holds a number, as
/// the name the dimension's definition gives that element
/// ([`WriterContext::element_at`]).
fn format_mdl_element_key(element_key: &str, dims: &[String], ctx: &WriterContext) -> String {
    element_key
        .split(',')
        .enumerate()
        .map(|(at, part)| {
            let Some(dim) = dims.get(at) else {
                return format_mdl_ident(part);
            };
            ctx.indexed_element(dim, part.trim())
                .or_else(|| ctx.declared_element(dim, part))
                .unwrap_or_else(|| format_mdl_ident(part))
        })
        .collect::<Vec<_>>()
        .join(",")
}

/// The name an indexed dimension's element at `position` (1-based) is
/// written under. Vensim's subscript elements are names, so a dimension that
/// is only a size is written as the range of these.
fn indexed_element_name(dimension: &str, position: usize) -> String {
    format_mdl_ident(&format!("{dimension}{position}"))
}

/// Map zero-argument XMILE builtins that are bare keywords in MDL.
/// In Vensim, these are written without parentheses (e.g. `Time` not `TIME()`).
fn mdl_bare_keyword(xmile_name: &str) -> Option<&'static str> {
    match xmile_name {
        "time" => Some("Time"),
        "dt" | "time_step" => Some("TIME STEP"),
        "starttime" | "initial_time" => Some("INITIAL TIME"),
        "endtime" | "stoptime" | "final_time" => Some("FINAL TIME"),
        _ => None,
    }
}

/// Numeric literal emitted for a genuine `pi` builtin reference. Vensim has no
/// `PI` builtin and its parser rejects the zero-arg call `PI()`, so the writer
/// substitutes a literal with enough precision to round-trip (#850). The value
/// is f64 pi to full precision (17 significant digits), so it parses back to the
/// exact same bit pattern the `pi` builtin would evaluate to.
const PI_LITERAL: &str = "3.141592653589793";

/// A single non-fatal degradation encountered while writing a Project to MDL.
///
/// The MDL surface cannot represent everything the datamodel can (see the
/// module-level "Lossiness contract" note in `mdl/mod.rs`). Rather than fail
/// the whole export or silently drop the affected data, the writer records an
/// `ExportWarning` and emits the closest representable form. Callers that want
/// to surface degradation to the user consume the warnings via
/// [`crate::mdl::project_to_mdl_with_warnings`]; the plain
/// [`crate::mdl::project_to_mdl`] discards them for the many callers that only
/// need the text. Warnings are a side channel and never change the emitted
/// text, so they do not affect the corpus round-trip ratchets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportWarning {
    /// A specific, human-readable description naming the affected variable or
    /// dimension (e.g. `graphical function for 'demand curve' uses discrete
    /// interpolation, ...`).
    pub message: String,
}

impl ExportWarning {
    fn new(message: impl Into<String>) -> Self {
        ExportWarning {
            message: message.into(),
        }
    }
}

/// Model-scoped context threaded through the MDL expression printer so it can
/// emit output its own parser accepts and that does not silently rebind
/// identifiers.
///
/// Without it, `MdlPrintVisitor` is context-free and cannot tell a genuine
/// 0-arity builtin (`pi`, `time`, `time_step`, ...) from a like-named user
/// variable (the parser reifies BOTH into a zero-arg `Expr0::App`), nor recover
/// the dimension a wildcard subscript `x[*]` reduces over (the importer dropped
/// the `Dim!` bang form to a bare `*`).
#[derive(Default)]
pub struct WriterContext {
    /// Canonical idents of the model's variables. A zero-arg-`App` reference
    /// whose name appears here is a user variable shadowing the like-named
    /// builtin, so it must be emitted as the identifier rather than the builtin.
    var_idents: HashSet<String>,
    /// Canonical variable ident -> its declared dimension names, in order.
    /// Drives wildcard-subscript recovery: `SUM(a[*])` where `a` is declared
    /// `a[DimA]` becomes `SUM(a[DimA!])` (Vensim's bang form).
    var_dims: HashMap<String, Vec<String>>,
    /// Canonical idents of standalone lookup-only variables whose graphical
    /// function extrapolates. Vensim MDL has no definition-level extrapolate
    /// flag: a lookup table is marked extrapolating only when a call site
    /// references it via `TABXL` (see `mdl::convert` and xmutil's
    /// `Expression::CheckTableUses`). So a `LOOKUP(table, x)` call to such a
    /// table is emitted as `TABXL(table, x)` instead of the native
    /// `table ( x )` form, which is what makes the table re-import as
    /// Extrapolate rather than being silently clamped to Continuous (#854).
    extrapolate_lookups: HashSet<String>,
    /// Canonical dimension name -> its named element names (display form, in
    /// declared order). Drives EXCEPT-default reconstruction (#858): the writer
    /// needs the full declared element set of an arrayed variable's dimensions
    /// to compute which elements the dropped `default_equation` covered.
    /// Indexed dimensions are absent (they have no named elements to except).
    dim_elements: HashMap<String, Vec<String>>,
    /// Canonical name of each indexed dimension -> its name as declared and
    /// its size. Its elements are written under names
    /// ([`indexed_element_name`]).
    indexed_dims: HashMap<String, (String, usize)>,
    /// Each named element (in the reader's `to_lower_space` form) -> the
    /// canonical name of the dimension the reader gives it to, by the
    /// reader's own rule.
    element_owners: HashMap<String, String>,
    /// Canonical names of the project's macros: a call of one is a call
    /// Vensim reads.
    macros: HashSet<String>,
    /// The project's dimensions, in declared order.
    dimensions: Vec<datamodel::Dimension>,
    /// Each variable of the model, by canonical name, with the name as its
    /// definition is written ([`WriterContext::reference`]).
    written_names: HashMap<String, String>,
    /// Subset of `extrapolate_lookups` that is actually referenced by at least
    /// one `LOOKUP(table, _)` call the printer rewrites to a kind-preserving
    /// `TABXL`. A standalone extrapolating lookup NOT in this set has no call
    /// site to mark it, so its kind silently clamps on re-import -- the writer
    /// warns for exactly those (#854/#856).
    referenced_extrapolate_lookups: HashSet<String>,
}

impl WriterContext {
    /// Build from a model's variables and the project dimensions: index every
    /// variable's canonical ident and, for arrayed / apply-to-all variables,
    /// its declared dimensions; index each named dimension's element list.
    fn from_model(model: &datamodel::Model, dimensions: &[datamodel::Dimension]) -> Self {
        let mut var_idents = HashSet::new();
        let mut var_dims = HashMap::new();
        let mut extrapolate_lookups = HashSet::new();
        for var in &model.variables {
            // A reader other than MDL's keeps a name as its file spells it
            // (`A_Values`), and every lookup here is by canonical name.
            let ident = crate::common::canonicalize(var.get_ident()).into_owned();
            if let Some(dims) = declared_dims(var) {
                var_dims.insert(ident.clone(), dims);
            }
            if var_is_extrapolating_lookup(var) {
                extrapolate_lookups.insert(ident.clone());
            }
            var_idents.insert(ident);
        }
        let mut dim_elements = HashMap::new();
        let mut indexed_dims = HashMap::new();
        for dim in dimensions {
            let canonical: String = crate::common::canonicalize(&dim.name).into();
            match &dim.elements {
                DimensionElements::Named(elems) => {
                    dim_elements.insert(canonical, elems.clone());
                }
                DimensionElements::Indexed(size) => {
                    indexed_dims.insert(canonical, (dim.name.clone(), *size as usize));
                }
            }
        }
        let element_owners = super::convert::element_owners(dimensions);
        // Only scan for call sites when there is an extrapolating lookup to
        // preserve -- the overwhelmingly common model has none, so this is a
        // no-op there (and a one-time equation parse pass otherwise).
        let referenced_extrapolate_lookups = if extrapolate_lookups.is_empty() {
            HashSet::new()
        } else {
            collect_extrapolating_lookup_call_sites(model, &extrapolate_lookups)
        };
        WriterContext {
            var_idents,
            var_dims,
            extrapolate_lookups,
            dim_elements,
            indexed_dims,
            element_owners,
            macros: HashSet::new(),
            dimensions: dimensions.to_vec(),
            // As a definition is written with no view to name it;
            // `with_written_names` gives each the name its view draws.
            written_names: model
                .variables
                .iter()
                .map(|var| {
                    (
                        crate::common::canonicalize(var.get_ident()).into_owned(),
                        display_name_for_ident(var.get_ident(), &HashMap::new()),
                    )
                })
                .collect(),
            referenced_extrapolate_lookups,
        }
    }

    /// How a reference to the variable `ident` is written: as the variable's
    /// own name is, by the one function that spells a definition
    /// (`display_name_for_ident`). The engine's names fold case, a display
    /// newline and a quoted spelling into one name, so an equation can spell
    /// a reference otherwise than its variable's definition, but the MDL
    /// reader matches names as written, and a reference spelled otherwise
    /// than its definition names nothing. A name the model does not define
    /// is spelled on its own (`format_mdl_ident`).
    fn reference(&self, ident: &str) -> String {
        match self
            .written_names
            .get(crate::common::canonicalize(ident).as_ref())
        {
            Some(defined) => defined.clone(),
            None => format_mdl_ident(ident),
        }
    }

    /// The context, knowing the names `model`'s definitions are written
    /// under (`display_name_for_ident` over `display_names`).
    fn with_written_names(
        mut self,
        model: &datamodel::Model,
        display_names: &HashMap<String, String>,
    ) -> Self {
        for var in &model.variables {
            self.written_names.insert(
                crate::common::canonicalize(var.get_ident()).into_owned(),
                display_name_for_ident(var.get_ident(), display_names),
            );
        }
        self
    }

    /// The context, knowing the macros `project` defines.
    fn with_macros(mut self, project: &datamodel::Project) -> Self {
        self.macros = project
            .models
            .iter()
            .filter(|model| model.macro_spec.is_some())
            .map(|model| crate::common::canonicalize(&model.name).into_owned())
            .collect();
        self
    }

    /// True when a call of `function` (an engine name) written as `mdl_name`
    /// is one Vensim reads: a function the reader's table holds, a macro of
    /// the project, or a call of one of the model's own tables.
    fn vensim_reads_call(&self, function: &str, mdl_name: &str) -> bool {
        let canonical = crate::common::canonicalize(function);
        !matches!(classify_symbol(mdl_name), SymbolClass::Regular)
            || self.macros.contains(canonical.as_ref())
            || self.is_variable(&canonical)
    }

    /// The written name of the element at `position` (1-based, as its text)
    /// of the indexed dimension `dim`; `None` when `dim` is not an indexed
    /// dimension or holds no such position.
    fn indexed_element(&self, dim: &str, position: &str) -> Option<String> {
        let (name, size) = self
            .indexed_dims
            .get(crate::common::canonicalize(dim).as_ref())?;
        let position: usize = position.parse().ok()?;
        (1..=*size)
            .contains(&position)
            .then(|| indexed_element_name(name, position))
    }

    /// The written name of the element at `position` (1-based) of `dim`,
    /// named or indexed.
    fn element_at(&self, dim: &str, position: usize) -> Option<String> {
        match self.dim_named_elements(dim) {
            Some(elements) => elements
                .get(position.checked_sub(1)?)
                .map(|element| format_mdl_ident(element)),
            None => self.indexed_element(dim, &position.to_string()),
        }
    }

    /// How `dimension.element`, an element named as a value (its position in
    /// the dimension), is written; `None` when `ident` names no element this
    /// way.
    ///
    /// Vensim names the element alone. That is its position in the dimension
    /// the reader gives the element to, so an element named through another
    /// dimension that holds it at another position (a subrange) is written as
    /// the number it is.
    fn element_value(&self, ident: &str) -> Option<String> {
        let canonical = crate::common::canonicalize(ident);
        if self.is_variable(&canonical) {
            return None;
        }
        let (dim, element) = canonical.split_once('\u{b7}')?;
        let Some(elements) = self.dim_elements.get(dim) else {
            return self
                .indexed_element(dim, element)
                .map(|_| element.to_owned());
        };
        let at = elements
            .iter()
            .position(|e| crate::common::canonicalize(e) == element)?;
        let owner = self.element_owners.get(&to_lower_space(&elements[at]))?;
        let position_in_owner = self
            .dim_elements
            .get(crate::common::canonicalize(owner).as_ref())?
            .iter()
            .position(|e| crate::common::canonicalize(e) == element)?;
        if position_in_owner == at {
            Some(format_mdl_ident(&elements[at]))
        } else {
            Some((at + 1).to_string())
        }
    }

    /// How `dim` spells its element `element` (any spelling of it), written;
    /// `None` when `dim` is not a named dimension holding it.
    fn declared_element(&self, dim: &str, element: &str) -> Option<String> {
        let canonical = crate::common::canonicalize(element.trim());
        self.dim_named_elements(dim)?
            .iter()
            .find(|e| crate::common::canonicalize(e) == canonical)
            .map(|e| format_mdl_ident(e))
    }

    /// The project's dimension `name` (any spelling).
    fn dimension(&self, name: &str) -> Option<&datamodel::Dimension> {
        let name = crate::common::canonicalize(name);
        self.dimensions
            .iter()
            .find(|d| crate::common::canonicalize(&d.name) == name)
    }

    /// The named elements (display form, declared order) of dimension `name`,
    /// or `None` for an unknown or indexed dimension.
    fn dim_named_elements(&self, name: &str) -> Option<&[String]> {
        self.dim_elements
            .get(crate::common::canonicalize(name).as_ref())
            .map(Vec::as_slice)
    }

    /// True when `canonical` names a standalone lookup-only variable whose
    /// graphical function extrapolates, so a `LOOKUP()` call to it must be
    /// emitted as `TABXL()` to survive the round trip (#854).
    fn is_extrapolating_lookup(&self, canonical: &str) -> bool {
        self.extrapolate_lookups.contains(canonical)
    }

    /// True when `canonical` is a standalone extrapolating lookup that NO
    /// `LOOKUP` call site references, so no kind-preserving `TABXL` is emitted
    /// and its extrapolate kind would silently clamp to Continuous on
    /// re-import. The writer warns for exactly these (#854/#856).
    fn is_unreferenced_extrapolating_lookup(&self, canonical: &str) -> bool {
        self.extrapolate_lookups.contains(canonical)
            && !self.referenced_extrapolate_lookups.contains(canonical)
    }

    /// True when `canonical` names a declared model variable.
    fn is_variable(&self, canonical: &str) -> bool {
        self.var_idents.contains(canonical)
    }

    /// Declared dimension name at subscript position `pos` of the variable
    /// `canonical_ident`, or `None` if the variable is scalar/absent or the
    /// position is out of range (callers fall back to a bare `*`).
    fn dim_at(&self, canonical_ident: &str, pos: usize) -> Option<&str> {
        self.var_dims
            .get(canonical_ident)
            .and_then(|dims| dims.get(pos))
            .map(String::as_str)
    }
}

/// True when `var` is a standalone lookup-only table (empty / sentinel
/// equation plus a graphical function) whose graphical function extrapolates.
///
/// Only the standalone (lookup-only) form is reported: an *embedded* `WITH
/// LOOKUP` variable applies an inline, unnamed table that no `TABXL` call site
/// can reference, so its Extrapolate kind is genuinely unrepresentable in MDL
/// and is handled by an [`ExportWarning`] instead (see
/// `warn_unrepresentable_gf_kinds`).
fn var_is_extrapolating_lookup(var: &datamodel::Variable) -> bool {
    let (gf, equation) = match var {
        datamodel::Variable::Flow(f) => (f.gf.as_ref(), &f.equation),
        datamodel::Variable::Aux(a) => (a.gf.as_ref(), &a.equation),
        _ => return false,
    };
    let Some(gf) = gf else { return false };
    if gf.kind != datamodel::GraphicalFunctionKind::Extrapolate {
        return false;
    }
    matches!(equation, Equation::Scalar(eqn) if is_lookup_only_equation(eqn))
}

/// The set of `extrapolate_lookups` idents that some variable references via a
/// `LOOKUP(table, _)` call -- i.e. exactly the calls the printer rewrites to a
/// kind-preserving `TABXL`. A standalone extrapolating lookup absent from this
/// set has no such call site, so its kind would silently clamp on re-import.
fn collect_extrapolating_lookup_call_sites(
    model: &datamodel::Model,
    extrapolate_lookups: &HashSet<String>,
) -> HashSet<String> {
    let mut referenced = HashSet::new();
    for var in &model.variables {
        let Some(equation) = var.get_equation() else {
            continue;
        };
        let mut texts: Vec<&str> = Vec::new();
        match equation {
            Equation::Scalar(s) => texts.push(s),
            Equation::ApplyToAll(_, s) => texts.push(s),
            Equation::Arrayed(_, elements, default, _) => {
                texts.extend(elements.iter().map(|(_, eqn, _, _)| eqn.as_str()));
                if let Some(d) = default {
                    texts.push(d);
                }
            }
        }
        for text in texts {
            if let Ok(Some(ast)) = Expr0::new(text, LexerType::Equation) {
                collect_lookup_refs(&ast, extrapolate_lookups, &mut referenced);
            }
        }
    }
    referenced
}

/// Walk `expr`, recording into `referenced` the canonical ident of every
/// `LOOKUP(table, _)` call whose bare-variable table argument names one of
/// `extrapolate_lookups`. The `func == "lookup"` / bare-`Var` first-argument
/// shape mirrors the printer's TABXL rewrite exactly, so this counts a call
/// site iff a `TABXL` is actually emitted for it.
fn collect_lookup_refs(
    expr: &Expr0,
    extrapolate_lookups: &HashSet<String>,
    referenced: &mut HashSet<String>,
) {
    match expr {
        Expr0::Const(_, _, _) | Expr0::Var(_, _) => {}
        Expr0::App(UntypedBuiltinFn(func, args), _) => {
            if func == "lookup"
                && args.len() == 2
                && let Expr0::Var(table, _) = &args[0]
            {
                let canon = crate::common::canonicalize(table.as_str());
                if extrapolate_lookups.contains(canon.as_ref()) {
                    referenced.insert(canon.into());
                }
            }
            for a in args {
                collect_lookup_refs(a, extrapolate_lookups, referenced);
            }
        }
        Expr0::Subscript(_, indices, _) => {
            for idx in indices {
                match idx {
                    IndexExpr0::Range(l, r, _) => {
                        collect_lookup_refs(l, extrapolate_lookups, referenced);
                        collect_lookup_refs(r, extrapolate_lookups, referenced);
                    }
                    IndexExpr0::Expr(e) => collect_lookup_refs(e, extrapolate_lookups, referenced),
                    IndexExpr0::Wildcard(_)
                    | IndexExpr0::StarRange(_, _)
                    | IndexExpr0::DimPosition(_, _) => {}
                }
            }
        }
        Expr0::Op1(_, e, _) => collect_lookup_refs(e, extrapolate_lookups, referenced),
        Expr0::Op2(_, l, r, _) => {
            collect_lookup_refs(l, extrapolate_lookups, referenced);
            collect_lookup_refs(r, extrapolate_lookups, referenced);
        }
        Expr0::If(c, t, f, _) => {
            collect_lookup_refs(c, extrapolate_lookups, referenced);
            collect_lookup_refs(t, extrapolate_lookups, referenced);
            collect_lookup_refs(f, extrapolate_lookups, referenced);
        }
    }
}

/// The declared dimension names (in order) of an arrayed / apply-to-all
/// variable, or `None` for a scalar or a `Module` (which has no equation).
fn declared_dims(var: &datamodel::Variable) -> Option<Vec<String>> {
    match var.get_equation()? {
        Equation::Scalar(_) => None,
        Equation::ApplyToAll(dims, _) => Some(dims.clone()),
        Equation::Arrayed(dims, _, _, _) => Some(dims.clone()),
    }
}

/// Map XMILE canonical function names back to their Vensim MDL equivalents.
/// This inverts the `format_function_name()` table in `xmile_compat.rs`.
/// The input is expected to already be lowercase (as stored in `Expr0::App`).
///
/// `init` is deliberately absent: its MDL name depends on arity (1-arg
/// `INITIAL` vs 2-arg `ACTIVE INITIAL`), so it is dispatched at the call site
/// where `args.len()` is known (#852).
fn xmile_to_mdl_function_name(xmile_name: &str) -> String {
    match xmile_name {
        "smth1" => "SMOOTH".to_owned(),
        "smth3" => "SMOOTH3".to_owned(),
        "delay" => "DELAY FIXED".to_owned(),
        "delay1" => "DELAY1".to_owned(),
        "delay3" => "DELAY3".to_owned(),
        "delayn" => "DELAY N".to_owned(),
        "smthn" => "SMOOTH N".to_owned(),
        // The engine's TRUNC and REM are Vensim's INTEGER and MODULO. Its INT
        // floors, which no Vensim function does, so it keeps its own name
        // (`unknown_function_warning`).
        "trunc" => "INTEGER".to_owned(),
        "rem" => "MODULO".to_owned(),
        "lookupinv" => "LOOKUP INVERT".to_owned(),
        "uniform" => "RANDOM UNIFORM".to_owned(),
        "safediv" => "ZIDZ".to_owned(),
        "forcst" => "FORECAST".to_owned(),
        "normalpink" => "RANDOM PINK NOISE".to_owned(),
        "normal" => "RANDOM NORMAL".to_owned(),
        "lookup" => "LOOKUP".to_owned(),
        "integ" => "INTEG".to_owned(),
        "size" => "ELMCOUNT".to_owned(),
        // Vensim's ranking function (vensim.com/documentation/fn_vector_rank.html).
        "rank" => "VECTOR RANK".to_owned(),
        // Built-in function names are always plain ASCII identifiers.
        _ => underbar_to_space(xmile_name).to_uppercase(),
    }
}

/// Reorder arguments for functions whose XMILE and MDL arg orders differ.
fn reorder_args(mdl_name: &str, mut args: Vec<String>) -> Vec<String> {
    match mdl_name {
        // XMILE: delayn(input, dt, n, init) -> MDL: DELAY N(input, dt, init, n)
        // XMILE: smthn(input, dt, n, init) -> MDL: SMOOTH N(input, dt, init, n)
        "DELAY N" | "SMOOTH N" => {
            if args.len() >= 4 {
                args.swap(2, 3);
            }
            args
        }
        // XMILE: normal(mean, sd, seed, min, max) -> MDL: RANDOM NORMAL(min, max, mean, sd, seed)
        "RANDOM NORMAL" => {
            if args.len() >= 5 {
                let mean = args[0].clone();
                let sd = args[1].clone();
                let seed = args[2].clone();
                let min = args[3].clone();
                let max = args[4].clone();
                args[0] = min;
                args[1] = max;
                args[2] = mean;
                args[3] = sd;
                args[4] = seed;
                args
            } else if args.len() == 2 {
                // Vensim has no 2-arg normal; RANDOM NORMAL requires all five
                // args. Synthesize unbounded truncation bounds (Vensim's
                // +/-1e38 infinities) and an auto seed so the result is a plain
                // normal(mean, sd) (#852).
                let mean = args[0].clone();
                let sd = args[1].clone();
                vec![
                    format_f64(f64::NEG_INFINITY),
                    format_f64(f64::INFINITY),
                    mean,
                    sd,
                    "0".to_owned(),
                ]
            } else {
                args
            }
        }
        _ => args,
    }
}

/// The shape `expr` is written as, for [`ast::paren_if_necessary`], the one
/// grouping rule, which the writer shares with `print_eqn`: an `If` is written
/// as `IF THEN ELSE(...)`, the `MOD` operator as `MOD(...)` and a recognized
/// builtin expansion (`LN(x) / LN(2)` as `LOG(x, 2)`) as its call, so each is
/// a call to the operator around it, and needs no grouping.
///
/// The rule keys on `ast::BinaryOp::precedence()`, the XMILE/Vensim table, NOT
/// on the precedence `mdl::parser` implements -- which is inverted for the
/// binary operators (it puts `+`/`-` at the lowest level and `:AND:` above the
/// comparisons; GH #914). Vensim's table is the correct target, since Vensim is
/// who reads the file. But `writer_proptest` then re-reads the writer's output
/// with `mdl::parser`, so its fixpoint property is only sound over the
/// operators where the two tables agree, which `expr0_strategy` keeps to.
fn written_shape(expr: &Expr0) -> NodeShape {
    if recognize_vensim_patterns(expr, &mut |_| String::new()).is_some() {
        return NodeShape::Call;
    }
    match expr {
        Expr0::If(..) | Expr0::Op2(BinaryOp::Mod, _, _, _) => NodeShape::Call,
        other => other.shape(),
    }
}

/// Returns true when `expr` is a 0-arity builtin call with the given name.
fn is_call(expr: &Expr0, name: &str) -> bool {
    matches!(expr, Expr0::App(UntypedBuiltinFn(f, args), _) if f == name && args.is_empty())
}

/// Returns true when `expr` is a reference to the variable with the given
/// canonical name, however the reference spells it (the reader's SAMPLE IF
/// TRUE expansion writes `SELF`).
fn is_var(expr: &Expr0, name: &str) -> bool {
    matches!(expr, Expr0::Var(id, _) if crate::common::canonicalize(id.as_str()) == name)
}

/// Returns true when `expr` is the constant `v`, bit for bit: a recognizer
/// that took a number near `v` for it would write another number.
fn is_const(expr: &Expr0, v: f64) -> bool {
    matches!(expr, Expr0::Const(_, n, _) if n.value().to_bits() == v.to_bits())
}

// ---- pattern recognizers ----

/// Match RANDOM 0 1: `uniform(0, 1)`.
fn recognize_random_0_1(expr: &Expr0) -> Option<String> {
    if let Expr0::App(UntypedBuiltinFn(f, args), _) = expr
        && f == "uniform"
        && args.len() == 2
        && is_const(&args[0], 0.0)
        && is_const(&args[1], 1.0)
    {
        return Some("RANDOM 0 1()".to_owned());
    }
    None
}

/// Match LOG 2-arg: `ln(x) / ln(base)`.
fn recognize_log_2arg(expr: &Expr0, walk: &mut impl FnMut(&Expr0) -> String) -> Option<String> {
    if let Expr0::Op2(BinaryOp::Div, l, r, _) = expr
        && let Expr0::App(UntypedBuiltinFn(lf, la), _) = l.as_ref()
        && let Expr0::App(UntypedBuiltinFn(rf, ra), _) = r.as_ref()
        && lf == "ln"
        && rf == "ln"
        && la.len() == 1
        && ra.len() == 1
    {
        return Some(format!("LOG({}, {})", walk(&la[0]), walk(&ra[0])));
    }
    None
}

/// Whether `expr` is the time half a step on, `time() + dt() / 2`, which is
/// what Vensim's PULSE compares (`xmile_compat`'s `pulse` arm).
fn is_time_plus(expr: &Expr0) -> bool {
    match_binop(expr, BinaryOp::Add).is_some_and(|(time, half_step)| {
        is_call(time, "time")
            && match_binop(half_step, BinaryOp::Div)
                .is_some_and(|(dt, two)| is_call(dt, "dt") && is_const(two, 2.0))
    })
}

/// Match PULSE, as the reader expands it (`xmile_compat`'s `pulse` arm):
/// `if (time_plus > A :AND: time_plus < A + W) then 1 else 0`, where the
/// window `W` is the width: `dt()` for a width of 0, the number for any other
/// number, and `if B = 0 then dt() else B` for a width `B` that is not one.
fn recognize_pulse(expr: &Expr0, walk: &mut impl FnMut(&Expr0) -> String) -> Option<String> {
    let (cond, t, f) = match_if(expr)?;
    if !is_const(t, 1.0) || !is_const(f, 0.0) {
        return None;
    }
    let (and_l, and_r) = match_binop(cond, BinaryOp::And)?;
    let (gt_l, a1) = match_binop(and_l, BinaryOp::Gt)?;
    let (lt_l, lt_r) = match_binop(and_r, BinaryOp::Lt)?;
    if !is_time_plus(gt_l) || !is_time_plus(lt_l) {
        return None;
    }
    let (a2, window) = match_binop(lt_r, BinaryOp::Add)?;
    if !a1.eq_ignoring_loc(a2) {
        return None;
    }
    let width = if is_call(window, "dt") {
        "0".to_owned()
    } else if number_literal(window).is_some_and(|v| v != 0.0) {
        walk(window)
    } else {
        let (is_zero, step, width) = match_if(window)?;
        let (b, zero) = match_binop(is_zero, BinaryOp::Eq)?;
        if !is_const(zero, 0.0) || !is_call(step, "dt") || !b.eq_ignoring_loc(width) {
            return None;
        }
        walk(width)
    };
    Some(format!("PULSE({}, {width})", walk(a1)))
}

/// The number `expr` is, with any sign.
fn number_literal(expr: &Expr0) -> Option<f64> {
    match expr {
        Expr0::Const(_, n, _) => Some(n.value()),
        Expr0::Op1(UnaryOp::Negative, inner, _) => number_literal(inner).map(|v| -v),
        Expr0::Op1(UnaryOp::Positive, inner, _) => number_literal(inner),
        _ => None,
    }
}

/// Match PULSE TRAIN, as the reader expands it (`xmile_compat`'s `pulse
/// train` arm):
/// `if (time_plus > A :AND: time() <= D :AND: (time_plus - A) MOD C < max(dt(), B)) then 1 else 0`.
fn recognize_pulse_train(expr: &Expr0, walk: &mut impl FnMut(&Expr0) -> String) -> Option<String> {
    let (cond, t, f) = match_if(expr)?;
    if !is_const(t, 1.0) || !is_const(f, 0.0) {
        return None;
    }
    let (outer_and_l, outer_and_r) = match_binop(cond, BinaryOp::And)?;
    let (inner_and_l, inner_and_r) = match_binop(outer_and_l, BinaryOp::And)?;

    let (gt_l, a1) = match_binop(inner_and_l, BinaryOp::Gt)?;
    let (lte_l, d) = match_binop(inner_and_r, BinaryOp::Lte)?;
    if !is_time_plus(gt_l) || !is_call(lte_l, "time") {
        return None;
    }

    let (mod_expr, window) = match_binop(outer_and_r, BinaryOp::Lt)?;
    let (sub_expr, c) = match_binop(mod_expr, BinaryOp::Mod)?;
    let (sub_l, a2) = match_binop(sub_expr, BinaryOp::Sub)?;
    if !is_time_plus(sub_l) || !a1.eq_ignoring_loc(a2) {
        return None;
    }
    let Expr0::App(UntypedBuiltinFn(max, max_args), _) = window else {
        return None;
    };
    let [step, b] = &max_args[..] else {
        return None;
    };
    if max != "max" || !is_call(step, "dt") {
        return None;
    }

    Some(format!(
        "PULSE TRAIN({}, {}, {}, {})",
        walk(a1),
        walk(b),
        walk(c),
        walk(d)
    ))
}

/// Match SAMPLE IF TRUE: `if cond then input else previous(self, init)`.
fn recognize_sample_if_true(
    expr: &Expr0,
    walk: &mut impl FnMut(&Expr0) -> String,
) -> Option<String> {
    let (cond, input, else_branch) = match_if(expr)?;
    if let Expr0::App(UntypedBuiltinFn(f, args), _) = else_branch
        && f == "previous"
        && args.len() == 2
        && is_var(&args[0], "self")
    {
        return Some(format!(
            "SAMPLE IF TRUE({}, {}, {})",
            walk(cond),
            walk(input),
            walk(&args[1])
        ));
    }
    None
}

/// Match ALLOCATE BY PRIORITY in both forms:
/// - Native: `allocate_by_priority(request, priority, size, width, supply)`
/// - Legacy: `allocate(supply, last_subscript_ident, demand_with_star, priority, width)`
fn recognize_allocate(expr: &Expr0, walk: &mut impl FnMut(&Expr0) -> String) -> Option<String> {
    if let Expr0::App(UntypedBuiltinFn(f, args), _) = expr {
        // Native form: allocate_by_priority(request, priority, size, width, supply)
        // Args are already in MDL order.
        if f == "allocate_by_priority" && args.len() == 5 {
            let request = walk(&args[0]);
            let priority = walk(&args[1]);
            let size = walk(&args[2]);
            let width = walk(&args[3]);
            let supply = walk(&args[4]);
            return Some(format!(
                "ALLOCATE BY PRIORITY({request}, {priority}, {size}, {width}, {supply})"
            ));
        }

        // Legacy form: allocate(supply, last_subscript, demand_with_star, priority, width)
        if f != "allocate" || args.len() != 5 {
            return None;
        }
        let supply = walk(&args[0]);
        let priority = walk(&args[3]);
        let width = walk(&args[4]);

        // args[1] is the last subscript dimension name (a Var)
        let dim_name = if let Expr0::Var(id, _) = &args[1] {
            format_mdl_ident(id.as_str())
        } else {
            return None;
        };

        // args[2] is the demand variable, possibly with a final `*` subscript
        // that should be replaced with the dimension name
        let demand_str = if let Expr0::Subscript(id, subs, _) = &args[2] {
            let demand_name = format_mdl_ident(id.as_str());
            if subs.is_empty() {
                demand_name
            } else {
                let mut sub_strs: Vec<String> = subs
                    .iter()
                    .map(|s| match s {
                        IndexExpr0::Wildcard(_) => dim_name.clone(),
                        IndexExpr0::Expr(e) => walk(e),
                        // StarRange / Range / DimPosition: reuse the caller's
                        // `walk` closure (which carries the WriterContext) for
                        // any nested expression rather than spinning up a
                        // context-free visitor. StarRange is a subrange
                        // wildcard -> bang form (see `walk_index`).
                        IndexExpr0::StarRange(id, _) => {
                            format!("{}!", format_mdl_ident(id.as_str()))
                        }
                        IndexExpr0::Range(l, r, _) => format!("{}:{}", walk(l), walk(r)),
                        IndexExpr0::DimPosition(n, _) => format!("@{n}"),
                    })
                    .collect();
                if let Some(IndexExpr0::StarRange(_, _)) = subs.last()
                    && let Some(l) = sub_strs.last_mut()
                {
                    *l = dim_name.clone();
                }
                format!("{demand_name}[{}]", sub_strs.join(", "))
            }
        } else {
            walk(&args[2])
        };

        return Some(format!(
            "ALLOCATE BY PRIORITY({demand_str}, {priority}, 0, {width}, {supply})"
        ));
    }
    None
}

/// Match TIME BASE: `t + dt_val * time()`, which is the value Vensim gives it
/// ("equivalent to (START + Time*SLOPE)",
/// vensim.com/documentation/fn_time_base.html).
fn recognize_time_base(expr: &Expr0, walk: &mut impl FnMut(&Expr0) -> String) -> Option<String> {
    let (add_l, mul_expr) = match_binop(expr, BinaryOp::Add)?;
    let (dt_val, time_call) = match_binop(mul_expr, BinaryOp::Mul)?;
    if !is_call(time_call, "time") {
        return None;
    }
    Some(format!("TIME BASE({}, {})", walk(add_l), walk(dt_val)))
}

/// Match RANDOM POISSON:
/// `poisson(mean / dt(), seed, min, max) * factor + sdev`.
fn recognize_random_poisson(
    expr: &Expr0,
    walk: &mut impl FnMut(&Expr0) -> String,
) -> Option<String> {
    // Outer: Add(Mul(App("poisson", ...), factor), sdev)
    let (mul_expr, sdev) = match_binop(expr, BinaryOp::Add)?;
    let (poisson_call, factor) = match_binop(mul_expr, BinaryOp::Mul)?;
    if let Expr0::App(UntypedBuiltinFn(f, args), _) = poisson_call
        && f == "poisson"
        && args.len() == 4
    {
        let (mean, dt_call) = match_binop(&args[0], BinaryOp::Div)?;
        if !is_call(dt_call, "dt") {
            return None;
        }
        let min = &args[2];
        let max = &args[3];
        let seed = &args[1];
        return Some(format!(
            "RANDOM POISSON({}, {}, {}, {}, {}, {})",
            walk(min),
            walk(max),
            walk(mean),
            walk(sdev),
            walk(factor),
            walk(seed)
        ));
    }
    None
}

// ---- helper matchers ----

fn match_if(expr: &Expr0) -> Option<(&Expr0, &Expr0, &Expr0)> {
    if let Expr0::If(cond, t, f, _) = expr {
        Some((cond, t, f))
    } else {
        None
    }
}

fn match_binop(expr: &Expr0, expected_op: BinaryOp) -> Option<(&Expr0, &Expr0)> {
    if let Expr0::Op2(op, l, r, _) = expr
        && *op == expected_op
    {
        return Some((l, r));
    }
    None
}

/// Try to recognize known XMILE structural expansions and collapse them
/// back to their compact Vensim builtin form.  Returns `None` when no
/// pattern matches, letting the caller fall through to mechanical conversion.
fn recognize_vensim_patterns(
    expr: &Expr0,
    walk: &mut impl FnMut(&Expr0) -> String,
) -> Option<String> {
    // Order matters: check more specific patterns first.
    if let Some(s) = recognize_random_0_1(expr) {
        return Some(s);
    }
    if let Some(s) = recognize_log_2arg(expr, walk) {
        return Some(s);
    }
    if let Some(s) = recognize_pulse_train(expr, walk) {
        return Some(s);
    }
    if let Some(s) = recognize_pulse(expr, walk) {
        return Some(s);
    }
    if let Some(s) = recognize_sample_if_true(expr, walk) {
        return Some(s);
    }
    if let Some(s) = recognize_allocate(expr, walk) {
        return Some(s);
    }
    if let Some(s) = recognize_time_base(expr, walk) {
        return Some(s);
    }
    if let Some(s) = recognize_random_poisson(expr, walk) {
        return Some(s);
    }
    None
}

struct MdlPrintVisitor<'a> {
    ctx: &'a WriterContext,
    /// What the walk wrote as the nearest thing Vensim has rather than as
    /// itself, for the caller to warn about.
    inexact: Inexact,
}

/// The constructs an equation holds that Vensim has no spelling for. They are
/// recorded where the printer writes them, not found by a scan of the tree: a
/// `MOD` inside a recognized PULSE TRAIN is written as Vensim's call.
#[derive(Default)]
struct Inexact {
    /// The functions called that Vensim does not have, as they were written.
    unknown_functions: Vec<String>,
    /// A floored modulus written as Vensim's MODULO, a truncated remainder,
    /// because the model names something MOD.
    modulus_as_modulo: bool,
    /// A NaN written as `NaN`.
    not_a_number: bool,
}

impl MdlPrintVisitor<'_> {
    /// Walk a subscript index at a known position of a known subscripted
    /// variable. A wildcard here recovers Vensim's bang form `Dim!` from the
    /// variable's declared dimension at that position; every other index kind
    /// is position-independent and delegates to `walk_index` (#847).
    fn walk_index_at(&mut self, expr: &IndexExpr0, subscripted: &str, pos: usize) -> String {
        match expr {
            IndexExpr0::Wildcard(_) => match self.ctx.dim_at(subscripted, pos) {
                Some(dim) => format!("{}!", format_mdl_ident(dim)),
                // No declared dimension for this position (scalar/absent var or
                // out-of-range): leave a bare `*` -- a safe, if less faithful,
                // fallback rather than a panic.
                None => "*".to_string(),
            },
            // An element named by its position: Vensim names it.
            IndexExpr0::Expr(Expr0::Const(_, position, _)) => {
                let position = position.value();
                let named = (position.fract() == 0.0 && position >= 1.0)
                    .then(|| self.ctx.dim_at(subscripted, pos))
                    .flatten()
                    .and_then(|dim| self.ctx.element_at(dim, position as usize));
                named.unwrap_or_else(|| self.walk_index(expr))
            }
            other => self.walk_index(other),
        }
    }

    /// The floored modulus `l MOD r`. Vensim has no modulus operator, and its
    /// MODULO function is a truncated remainder where `MOD` is a floored
    /// modulus (XMILE 1.0 section 3.3.1; vensim.com/documentation/
    /// fn_modulo.html). So it is written as a call of MOD, which Vensim does
    /// not have and the reader reads back as the operator.
    ///
    /// In a model that defines its own `mod`, a MOD call is that variable's,
    /// so the modulus is written as MODULO, the nearest thing Vensim has, and
    /// noted so the caller can warn.
    fn write_modulus(&mut self, l: &Expr0, r: &Expr0) -> String {
        let function = if self.ctx.is_variable("mod") || self.ctx.macros.contains("mod") {
            self.inexact.modulus_as_modulo = true;
            "MODULO"
        } else {
            self.inexact.unknown_functions.push("MOD".to_owned());
            "MOD"
        };
        let l = self.walk(l);
        let r = self.walk(r);
        format!("{function}({l}, {r})")
    }

    /// `expr` as an operand of the operators the writer composes around it.
    fn walk_operand(&mut self, expr: &Expr0) -> String {
        let text = self.walk(expr);
        match expr {
            Expr0::Const(..) | Expr0::Var(..) | Expr0::App(..) | Expr0::Subscript(..) => text,
            Expr0::Op1(..) | Expr0::Op2(..) | Expr0::If(..) => format!("({text})"),
        }
    }

    /// XMILE's `PULSE(volume, first, interval)` in Vensim's terms.
    ///
    /// The two PULSEs are different functions. XMILE's is an impulse:
    /// `volume / DT` for the one step at `first`, and again every `interval`
    /// after when one is given (XMILE 1.0 section 3.5.4). Vensim's
    /// `PULSE(start, width)` is 1 from `start` for `width`
    /// (vensim.com/documentation/fn_pulse.html). So the call is written as
    /// the comparison the engine's own `vm::pulse` makes, `first <= Time <
    /// first + TIME STEP`, on each interval. An interval that is not a
    /// number in the equation decides which at run time, as the engine does:
    /// one at or below zero is a single pulse.
    fn xmile_pulse(&mut self, args: &[Expr0]) -> String {
        let volume = self.walk_operand(&args[0]);
        let first = self.walk_operand(&args[1]);
        let once = format!("Time < {first} + TIME STEP");
        let within = match args.get(2) {
            None => once,
            Some(Expr0::Const(_, interval, _)) if interval.value() <= 0.0 => once,
            Some(interval) => {
                let is_number = matches!(interval, Expr0::Const(..));
                let interval = self.walk_operand(interval);
                let repeating = format!("MODULO(Time - {first}, {interval}) < TIME STEP");
                if is_number {
                    repeating
                } else {
                    format!("IF THEN ELSE({interval} > 0, {repeating}, {once})")
                }
            }
        };
        format!("IF THEN ELSE(Time >= {first} :AND: {within}, {volume} / TIME STEP, 0)")
    }
}

impl Visitor<String> for MdlPrintVisitor<'_> {
    fn walk_index(&mut self, expr: &IndexExpr0) -> String {
        match expr {
            IndexExpr0::Wildcard(_) => "*".to_string(),
            // A star-range `*:Dim` (Vensim `Dim.*`) is a subrange wildcard --
            // "iterate over all of Dim" -- which in Vensim MDL is the bang form
            // `Dim!`. Emitting `*:Dim` produces output the reader rejects, and
            // renders the SAME construct differently from a recovered wildcard,
            // so both are emitted as `Dim!` (#847).
            IndexExpr0::StarRange(id, _) => {
                format!("{}!", format_mdl_ident(id.as_str()))
            }
            IndexExpr0::Range(l, r, _) => format!("{}:{}", self.walk(l), self.walk(r)),
            IndexExpr0::DimPosition(n, _) => format!("@{n}"),
            IndexExpr0::Expr(e) => self.walk(e),
        }
    }

    fn walk(&mut self, expr: &Expr0) -> String {
        // Try pattern recognizers first
        if let Some(s) = recognize_vensim_patterns(expr, &mut |e| self.walk(e)) {
            return s;
        }
        match expr {
            // A number is written as the reader stores it (`format_number`),
            // so a save reads back as the text it writes: `0.0000001` and
            // `1e-07` are one number, and only one spelling is a fixed point.
            Expr0::Const(s, n, _) => {
                if n.value().is_finite() {
                    super::xmile_compat::format_number(n.value())
                } else {
                    self.inexact.not_a_number |= n.value().is_nan();
                    s.clone()
                }
            }
            Expr0::Var(id, _) => self
                .ctx
                .element_value(id.as_str())
                .unwrap_or_else(|| self.ctx.reference(id.as_str())),
            Expr0::App(UntypedBuiltinFn(func, args), _) => {
                // The engine reads a MODULO call as its MOD operator
                // (`builtins_visitor`), unless the project defines a MODULO
                // macro; a variable of that name is no function.
                if func == "modulo" && args.len() == 2 && !self.ctx.macros.contains("modulo") {
                    return self.write_modulus(&args[0], &args[1]);
                }
                // A variable named `pulse` is no function, so the call is the
                // builtin whatever the model names; a macro of that name is
                // the call's.
                if func == "pulse"
                    && (2..=3).contains(&args.len())
                    && !self.ctx.macros.contains("pulse")
                {
                    return self.xmile_pulse(args);
                }
                if args.is_empty() {
                    // A zero-argument `App` is the builtin, and a reference to
                    // a variable of a builtin's name is a `Var` (quoted in the
                    // equation, as the importer writes it), which the `Var`
                    // arm spells as the variable's name; so `dt = TIME STEP`
                    // writes back as it was. Vensim has no PI builtin and rejects the zero-arg call
                    // `PI()`; emit a numeric literal instead (#850).
                    if func == "pi" {
                        return PI_LITERAL.to_owned();
                    }
                    // TIME/DT/... are bare keywords in Vensim (no parentheses).
                    if let Some(kw) = mdl_bare_keyword(func) {
                        return kw.to_owned();
                    }
                }
                // Vensim lookup calls use `table_name ( input )` syntax
                // rather than `LOOKUP(table_name, input)`.
                if func == "lookup"
                    && args.len() == 2
                    && let Expr0::Var(table_ident, _) = &args[0]
                {
                    let table_name = self.ctx.reference(table_ident.as_str());
                    let input = self.walk(&args[1]);
                    // An Extrapolate lookup table is marked extrapolating only
                    // by a `TABXL` call site on re-import (MDL has no
                    // definition-level flag), so emit the call as TABXL rather
                    // than the native `table ( input )` form to preserve the
                    // kind through the round trip (#854).
                    if self
                        .ctx
                        .is_extrapolating_lookup(&crate::common::canonicalize(table_ident.as_str()))
                    {
                        return format!("TABXL({table_name}, {input})");
                    }
                    return format!("{table_name} ( {input} )");
                }
                // A few builtins map to different Vensim names by arity:
                //   - safediv 3+ args -> XIDZ (3-arg), else ZIDZ (2-arg)
                //   - init 1 arg -> INITIAL, else ACTIVE INITIAL (which
                //     requires two args -- emitting it 1-arg is invalid) (#852)
                //   - a smooth or delay with an initial value is Vensim's
                //     `I` function: SMOOTH and DELAY1 take two arguments and
                //     SMOOTHI and DELAY1I three
                //     (vensim.com/documentation/fn_smoothi.html,
                //     fn_delay1i.html)
                //   - a one-argument MAX or MIN is the engine's reduction over
                //     an array, Vensim's VMAX and VMIN; Vensim's MAX and MIN
                //     are of two alternatives (vensim.com/documentation/
                //     fn_vmax.html, fn_vmin.html, fn_max.html, fn_min.html)
                let with_initial = match (func.as_str(), args.len()) {
                    ("smth1", 3) => Some("SMOOTHI"),
                    ("smth3", 3) => Some("SMOOTH3I"),
                    ("delay1", 3) => Some("DELAY1I"),
                    ("delay3", 3) => Some("DELAY3I"),
                    ("max", 1) => Some("VMAX"),
                    ("min", 1) => Some("VMIN"),
                    _ => None,
                };
                let mdl_name = if let Some(name) = with_initial {
                    name.to_owned()
                } else if func == "safediv" && args.len() >= 3 {
                    "XIDZ".to_owned()
                } else if func == "init" {
                    if args.len() == 1 {
                        "INITIAL".to_owned()
                    } else {
                        "ACTIVE INITIAL".to_owned()
                    }
                } else {
                    xmile_to_mdl_function_name(func)
                };
                if !self.ctx.vensim_reads_call(func, &mdl_name) {
                    self.inexact.unknown_functions.push(mdl_name.clone());
                }
                let converted: Vec<String> = args.iter().map(|e| self.walk(e)).collect();
                let reordered = reorder_args(&mdl_name, converted);
                format!("{}({})", mdl_name, reordered.join(", "))
            }
            Expr0::Subscript(id, args, _) => {
                // Canonicalize the subscripted variable's ident once so the
                // wildcard-recovery lookup keys the same way `WriterContext`
                // indexed the model's variables.
                let canonical = crate::common::canonicalize(id.as_str());
                let args: Vec<String> = args
                    .iter()
                    .enumerate()
                    .map(|(pos, e)| self.walk_index_at(e, canonical.as_ref(), pos))
                    .collect();
                format!("{}[{}]", self.ctx.reference(id.as_str()), args.join(", "))
            }
            Expr0::Op1(op, l, _) => {
                // The operand groups through the shared rule in every arm. Vensim
                // has no transpose, so such an equation is degraded regardless
                // (`transpose_warning`); grouping its operand at least makes the
                // text denote the tree the model has, not the different `a + b'`.
                let l = paren_if_necessary(expr.shape(), written_shape(l), false, self.walk(l));
                match op {
                    UnaryOp::Transpose => format!("{l}'"),
                    UnaryOp::Positive => format!("+{l}"),
                    UnaryOp::Negative => format!("-{l}"),
                    // MDL uses the keyword form with a trailing space before the operand
                    UnaryOp::Not => format!(":NOT: {l}"),
                }
            }
            Expr0::Op2(op, l, r, _) => {
                if *op == BinaryOp::Mod {
                    return self.write_modulus(l, r);
                }
                let l = paren_if_necessary(expr.shape(), written_shape(l), false, self.walk(l));
                let r = paren_if_necessary(expr.shape(), written_shape(r), true, self.walk(r));
                let op_str = match op {
                    BinaryOp::Add => "+",
                    BinaryOp::Sub => "-",
                    BinaryOp::Exp => "^",
                    BinaryOp::Mul => "*",
                    BinaryOp::Div => "/",
                    BinaryOp::Mod => unreachable!(),
                    BinaryOp::Gt => ">",
                    BinaryOp::Lt => "<",
                    BinaryOp::Gte => ">=",
                    BinaryOp::Lte => "<=",
                    BinaryOp::Eq => "=",
                    BinaryOp::Neq => "<>",
                    BinaryOp::And => ":AND:",
                    BinaryOp::Or => ":OR:",
                };
                format!("{l} {op_str} {r}")
            }
            Expr0::If(cond, t, f, _) => {
                let cond = self.walk(cond);
                let t = self.walk(t);
                let f = self.walk(f);
                format!("IF THEN ELSE({cond}, {t}, {f})")
            }
        }
    }
}

/// Convert an `Expr0` AST to MDL-format equation text, without model context.
///
/// Back-compat entry point (re-exported from `mdl/mod.rs`): equivalent to
/// [`expr0_to_mdl_ctx`] with an empty [`WriterContext`], so wildcard subscripts
/// fall back to a bare `*` and no identifier shadows a builtin. Production
/// emission threads a real context via `expr0_to_mdl_ctx`.
pub fn expr0_to_mdl(expr: &Expr0) -> String {
    expr0_to_mdl_ctx(expr, &WriterContext::default())
}

/// Convert an `Expr0` AST to MDL-format equation text using model context for
/// wildcard-subscript recovery and builtin/variable disambiguation.
pub fn expr0_to_mdl_ctx(expr: &Expr0, ctx: &WriterContext) -> String {
    expr0_to_mdl_noting(expr, ctx).0
}

/// [`expr0_to_mdl_ctx`], with what the text spells inexactly.
fn expr0_to_mdl_noting(expr: &Expr0, ctx: &WriterContext) -> (String, Inexact) {
    let mut visitor = MdlPrintVisitor {
        ctx,
        inexact: Inexact::default(),
    };
    let text = visitor.walk(expr);
    (text, visitor.inexact)
}

/// Convert an XMILE equation string to MDL text via Expr0 round-trip.
///
/// `ctx` supplies the model's variable/dimension knowledge so wildcard
/// subscripts and shadowed builtins render correctly. `name` is the display name
/// of the variable the equation belongs to, used only for the warning below.
///
/// # The fallback arm warns (#912)
///
/// When `Expr0::new` cannot parse the stored equation there is no AST to render,
/// so all the writer can do is emit the raw XMILE text with underscores turned
/// back into spaces. That text is *not* MDL: it skips the builtin-rename table
/// (`int` -> `INTEGER`, `smth1` -> `SMOOTH`, `safediv` -> `ZIDZ`, ...), and
/// Vensim has no such builtins -- so on re-import an unmapped call reads as a
/// **lookup invocation of an undefined table** (`INT(0)` -> `LOOKUP(INT, 0)`).
/// The meaning changes silently, and compounds across passes. The writer cannot
/// vouch for that text, so it records an [`ExportWarning`] rather than degrade
/// quietly. (An opaque data equation is a *legitimate* verbatim path and returns
/// before this point.)
fn equation_to_mdl(
    xmile_eqn: &str,
    name: &str,
    ctx: &WriterContext,
    warnings: &mut Vec<ExportWarning>,
) -> String {
    if xmile_eqn.is_empty() {
        return String::new();
    }
    // Data equation placeholders (GET DIRECT DATA, GET XLS, etc.) are opaque
    // strings that cannot be parsed as Expr0.  Emit them verbatim, stripping
    // the outer braces that the normalizer adds.
    if is_external_data_placeholder(xmile_eqn) {
        let stripped = xmile_eqn
            .strip_prefix('{')
            .and_then(|s| s.strip_suffix('}'))
            .unwrap_or(xmile_eqn);
        return stripped.to_string();
    }
    match Expr0::new(xmile_eqn, LexerType::Equation) {
        Ok(Some(ast)) => {
            if expr0_contains_transpose(&ast) {
                warnings.push(transpose_warning(name, xmile_eqn));
            }
            if holds_a_nested_active_initial(&ast, true) {
                warnings.push(nested_active_initial_warning(name, xmile_eqn));
            }
            let (text, mut inexact) = expr0_to_mdl_noting(&ast, ctx);
            inexact.unknown_functions.sort();
            inexact.unknown_functions.dedup();
            for function in &inexact.unknown_functions {
                warnings.push(unknown_function_warning(name, function, xmile_eqn));
            }
            if inexact.modulus_as_modulo {
                warnings.push(modulo_warning(name, xmile_eqn));
            }
            if inexact.not_a_number {
                warnings.push(not_a_number_warning(name, xmile_eqn));
            }
            text
        }
        // A non-empty equation that lexes to NOTHING (whitespace, or a bare
        // comment) parsed fine; it just has no AST and no content to mis-render.
        // Passing it through is exact, so it is not a degradation.
        Ok(None) => underbar_to_space(xmile_eqn),
        Err(_) => {
            warnings.push(unparseable_equation_warning(name, xmile_eqn));
            underbar_to_space(xmile_eqn)
        }
    }
}

/// The [`ExportWarning`] for an equation that is or holds NaN: the reader's
/// form of Vensim's `A FUNCTION OF` placeholder, which "is not intended for
/// use in writing equations, and precludes simulation"
/// (vensim.com/documentation/fn_a_function_of.html). It is written as `NaN`,
/// which Simlin reads back; whether Vensim reads it is unverified.
fn not_a_number_warning(name: &str, xmile_eqn: &str) -> ExportWarning {
    ExportWarning::new(format!(
        "the equation for '{name}' is not a number ({xmile_eqn:?}), as a variable \
         defined only as A FUNCTION OF is; it was written as NaN, which Vensim's \
         function reference does not define"
    ))
}

/// Whether `expr` holds a two-argument `INIT` (written as ACTIVE INITIAL)
/// anywhere but at the top of the equation (`top`), where Vensim does not
/// read it: ACTIVE INITIAL "must appear first on the right of the = sign and
/// not be followed by anything else"
/// (vensim.com/documentation/fn_active_initial.html), and the reader keeps
/// only one at the top.
fn holds_a_nested_active_initial(expr: &Expr0, top: bool) -> bool {
    let nested = |e: &Expr0| holds_a_nested_active_initial(e, false);
    match expr {
        Expr0::Const(..) | Expr0::Var(..) => false,
        Expr0::App(UntypedBuiltinFn(f, args), _) => {
            (f == "init" && args.len() == 2 && !top) || args.iter().any(nested)
        }
        Expr0::Subscript(_, indices, _) => indices.iter().any(|index| match index {
            IndexExpr0::Expr(e) => nested(e),
            IndexExpr0::Range(l, r, _) => nested(l) || nested(r),
            IndexExpr0::Wildcard(_)
            | IndexExpr0::StarRange(_, _)
            | IndexExpr0::DimPosition(_, _) => false,
        }),
        Expr0::Op1(_, e, _) => nested(e),
        Expr0::Op2(_, l, r, _) => nested(l) || nested(r),
        Expr0::If(c, t, f, _) => nested(c) || nested(t) || nested(f),
    }
}

/// The [`ExportWarning`] for an ACTIVE INITIAL that is not the whole
/// equation ([`holds_a_nested_active_initial`]).
fn nested_active_initial_warning(name: &str, xmile_eqn: &str) -> ExportWarning {
    ExportWarning::new(format!(
        "the equation for '{name}' has an initial value inside it ({xmile_eqn:?}); it was \
         written as an ACTIVE INITIAL there, which Vensim reads only as a whole equation, \
         and the initial value is not kept"
    ))
}

/// The [`ExportWarning`] for the [`equation_to_mdl`] raw-text fallback (#912).
fn unparseable_equation_warning(name: &str, xmile_eqn: &str) -> ExportWarning {
    ExportWarning::new(format!(
        "the equation for '{name}' could not be parsed ({xmile_eqn:?}), so it was \
         written to MDL as raw text; builtin renames were not applied and the \
         equation may mean something different when re-imported"
    ))
}

/// The [`ExportWarning`] for an equation using the transpose operator (#913).
fn transpose_warning(name: &str, xmile_eqn: &str) -> ExportWarning {
    ExportWarning::new(format!(
        "the equation for '{name}' uses the transpose operator ({xmile_eqn:?}), which \
         Vensim has no equivalent for; the `'` was written through as-is and will not \
         re-import"
    ))
}

/// The [`ExportWarning`] for an equation calling a function Vensim does not
/// have: one the reader's own table of Vensim functions (`mdl/builtins.rs`,
/// after vensim.com/documentation's function reference) does not hold, and
/// that is no macro or table of the model. ROUND, MEAN and PREVIOUS are such
/// functions, and so are INT, the floor (XMILE 1.0 footnote 7), and MOD, the
/// floored modulus: Vensim's INTEGER and MODULO truncate toward zero
/// (vensim.com/documentation/fn_integer.html, fn_modulo.html), and its
/// function reference has no INT or MOD.
///
/// The call is written by name. Simlin reads it back as the function it is
/// (the reader takes a call of a name the file does not define for a
/// function), and Vensim will not read it. The writer deliberately does NOT
/// lower such a call to Vensim primitives. ROUND, say, is composable from IF
/// THEN ELSE / INTEGER / MODULO, but the composition repeats the argument
/// expression, which is wrong outright for a stochastic argument (RANDOM
/// UNIFORM would be drawn once per copy), and it rests on what Vensim does at
/// an exact .5, which is unverified -- so a subtly wrong silent lowering is
/// worse than a loud warning.
fn unknown_function_warning(name: &str, function: &str, xmile_eqn: &str) -> ExportWarning {
    ExportWarning::new(format!(
        "the equation for '{name}' calls {function} ({xmile_eqn:?}), a function \
         Vensim does not have; it was written through as {function}(...), which \
         Vensim will not recognize"
    ))
}

/// The [`ExportWarning`] for a floored modulus written as MODULO. XMILE's
/// MOD is the floored modulus (XMILE 1.0 section 3.3.1) and Vensim's MODULO
/// the remainder of a truncated division
/// (vensim.com/documentation/fn_modulo.html), so the two differ when the
/// operands' signs do. A model that names something MOD has no other
/// spelling for it.
fn modulo_warning(name: &str, xmile_eqn: &str) -> ExportWarning {
    ExportWarning::new(format!(
        "the equation for '{name}' uses MOD ({xmile_eqn:?}), whose result has the sign \
         of its divisor; the model names a variable MOD, so it was written as Vensim's \
         MODULO, whose result has the sign of what is divided, and the two differ for \
         a negative operand"
    ))
}

/// Does this AST contain a transpose anywhere?
///
/// A pure predicate rather than a flag threaded through [`MdlPrintVisitor`]: the
/// visitor returns `String`, so a side channel would mean giving it interior
/// mutability just to report a fact the AST already carries.
fn expr0_contains_transpose(expr: &Expr0) -> bool {
    fn index_has(idx: &IndexExpr0) -> bool {
        match idx {
            IndexExpr0::Wildcard(_)
            | IndexExpr0::StarRange(_, _)
            | IndexExpr0::DimPosition(_, _) => false,
            IndexExpr0::Range(l, r, _) => {
                expr0_contains_transpose(l) || expr0_contains_transpose(r)
            }
            IndexExpr0::Expr(e) => expr0_contains_transpose(e),
        }
    }
    match expr {
        Expr0::Const(_, _, _) | Expr0::Var(_, _) => false,
        Expr0::App(UntypedBuiltinFn(_, args), _) => args.iter().any(expr0_contains_transpose),
        Expr0::Subscript(_, indices, _) => indices.iter().any(index_has),
        Expr0::Op1(UnaryOp::Transpose, _, _) => true,
        Expr0::Op1(_, l, _) => expr0_contains_transpose(l),
        Expr0::Op2(_, l, r, _) => expr0_contains_transpose(l) || expr0_contains_transpose(r),
        Expr0::If(c, t, f, _) => {
            expr0_contains_transpose(c)
                || expr0_contains_transpose(t)
                || expr0_contains_transpose(f)
        }
    }
}

/// Write the inner body of a lookup table into `buf` (no outer parens).
///
/// Format: `[(xmin,ymin)-(xmax,ymax)],(x1,y1),(x2,y2),...`
fn write_lookup_body(buf: &mut String, gf: &GraphicalFunction) {
    let xs: Vec<f64> = match &gf.x_points {
        Some(pts) => pts.clone(),
        None => {
            let n = gf.y_points.len();
            if n <= 1 {
                vec![gf.x_scale.min]
            } else {
                let step = (gf.x_scale.max - gf.x_scale.min) / (n - 1) as f64;
                (0..n).map(|i| gf.x_scale.min + step * i as f64).collect()
            }
        }
    };

    write!(
        buf,
        "[({},{})-({},{})]",
        format_f64(gf.x_scale.min),
        format_f64(gf.y_scale.min),
        format_f64(gf.x_scale.max),
        format_f64(gf.y_scale.max),
    )
    .unwrap();

    for (x, y) in xs.iter().zip(gf.y_points.iter()) {
        write!(buf, ",({},{})", format_f64(*x), format_f64(*y)).unwrap();
    }
}

/// Write a graphical-function (lookup table) wrapped in parens.
///
/// Format: `([(xmin,ymin)-(xmax,ymax)],(x1,y1),(x2,y2),...)`
fn write_lookup(buf: &mut String, gf: &GraphicalFunction) {
    buf.push('(');
    write_lookup_body(buf, gf);
    buf.push(')');
}

/// Returns true when the equation text is the canonical empty lookup-only form
/// (a graphical function with no input expression) or the legacy `"0+0"`
/// sentinel. Such a variable is a standalone lookup definition, written in
/// Vensim's native `name(body)` syntax rather than `name = WITH LOOKUP(input,
/// body)`. Delegates to the shared predicate so the writer, the MDL converter,
/// and the compiler all agree on what counts as lookup-only (issue #606).
fn is_lookup_only_equation(eqn: &str) -> bool {
    crate::variable::is_empty_or_sentinel(eqn)
}

/// Format f64 for MDL output: omit trailing `.0` for whole numbers.
fn format_f64(v: f64) -> String {
    if v.is_infinite() {
        if v.is_sign_positive() {
            "1e+38".to_owned()
        } else {
            "-1e+38".to_owned()
        }
    } else if v == v.trunc() && v.abs() < 1e15 {
        format!("{}", v as i64)
    } else {
        format!("{}", v)
    }
}

/// Write a single MDL variable entry, without model context.
///
/// Test-only: an empty [`WriterContext`], and the [`ExportWarning`]s are
/// discarded. Production emission goes through [`write_variable_entry_ctx_warn`]
/// so wildcard subscripts / shadowed builtins render correctly AND lossy
/// constructs are surfaced. This is deliberately NOT public: a caller handed a
/// silently-discarded warning channel cannot tell a faithful export from a
/// degraded one (#912).
#[cfg(test)]
fn write_variable_entry(
    buf: &mut String,
    var: &datamodel::Variable,
    display_names: &HashMap<String, String>,
) {
    write_variable_entry_ctx(buf, var, display_names, &WriterContext::default());
}

/// Write a single MDL variable entry using model context, discarding warnings.
///
/// The standard MDL format for a variable entry is:
/// ```text
/// Name=\n\tequation\n\t~\tunits\n\t~\tcomment\n\t|
/// ```
///
/// Test-only, for the same reason as [`write_variable_entry`].
#[cfg(test)]
fn write_variable_entry_ctx(
    buf: &mut String,
    var: &datamodel::Variable,
    display_names: &HashMap<String, String>,
    ctx: &WriterContext,
) {
    write_variable_entry_ctx_warn(buf, var, display_names, ctx, &mut Vec::new());
}

/// Write a single MDL variable entry, recording any lossy degradations into
/// `warnings` (#856). See [`write_variable_entry_ctx`] for the format.
fn write_variable_entry_ctx_warn(
    buf: &mut String,
    var: &datamodel::Variable,
    display_names: &HashMap<String, String>,
    ctx: &WriterContext,
    warnings: &mut Vec<ExportWarning>,
) {
    match var {
        datamodel::Variable::Stock(s) => {
            write_stock_variable(buf, s, display_names, ctx, warnings);
            return;
        }
        datamodel::Variable::Module(_) => return,
        _ => {}
    }

    let (ident, equation, units, doc, gf, compat) = match var {
        datamodel::Variable::Flow(f) => (
            &f.ident,
            &f.equation,
            &f.units,
            &f.documentation,
            f.gf.as_ref(),
            &f.compat,
        ),
        datamodel::Variable::Aux(a) => (
            &a.ident,
            &a.equation,
            &a.units,
            &a.documentation,
            a.gf.as_ref(),
            &a.compat,
        ),
        _ => unreachable!(),
    };

    let name = display_name_for_ident(ident, display_names);
    warn_dropped_non_negative(&name, compat, warnings);
    warn_dropped_conveyor_compat(&name, compat, warnings);
    warn_unrepresentable_gf_kinds(&name, ident, equation, gf, ctx, warnings);

    let data_source_eqn = compat_get_direct_equation(compat);
    let effective_gf = if data_source_eqn.is_some() { None } else { gf };

    match equation {
        Equation::Scalar(eqn) => {
            let effective_eqn = data_source_eqn
                .clone()
                .unwrap_or_else(|| wrap_active_initial(eqn, compat));
            write_single_entry(
                buf,
                &name,
                &effective_eqn,
                &[],
                units,
                doc,
                effective_gf,
                ctx,
                warnings,
            );
        }
        Equation::ApplyToAll(dims, eqn) => {
            let dim_names: Vec<&str> = dims.iter().map(|d| d.as_str()).collect();
            let effective_eqn = data_source_eqn
                .clone()
                .unwrap_or_else(|| wrap_active_initial(eqn, compat));
            write_single_entry(
                buf,
                &name,
                &effective_eqn,
                &dim_names,
                units,
                doc,
                effective_gf,
                ctx,
                warnings,
            );
        }
        Equation::Arrayed(dims, slots, default, has_except_default) => {
            let var = arrayed::Arrayed {
                name: &name,
                dims,
                slots,
                default,
                has_except_default: *has_except_default,
                rhs: arrayed::Rhs::Value { compat },
                units,
                doc,
            };
            arrayed::write_arrayed(buf, &var, ctx, warnings);
        }
    }
}

/// Record a warning when a variable carries Vensim's `compat.non_negative`
/// flag: the flag changes simulation semantics (the variable is clamped to be
/// non-negative) but has no MDL equation-text representation, so it is dropped
/// on export (#856).
fn warn_dropped_non_negative(
    name: &str,
    compat: &datamodel::Compat,
    warnings: &mut Vec<ExportWarning>,
) {
    if compat.non_negative {
        warnings.push(ExportWarning::new(format!(
            "'{name}' is marked non-negative (Vensim's :NA: / non-negative flag), \
             which changes simulation semantics but has no MDL equation-text \
             representation; the flag was dropped on export"
        )));
    }
}

/// Record warnings for conveyor/queue compat markers that Vensim MDL cannot
/// represent at all (#887): Vensim has no conveyor or queue primitive, so a
/// conveyor/queue stock exports as a plain INTEG stock and a leak / spreadflow
/// / overflow flow exports as an ordinary flow -- materially different
/// dynamics (a first-order stock instead of a pipeline delay / discrete
/// queue). The fields are checked in a fixed order (conveyor, queue, leakage,
/// spreadflow, overflow) so warning order is deterministic; a variable
/// carrying several markers gets one warning per marker in that order (the
/// engine rejects a stock marked both conveyor and queue, but flow-marker
/// combinations are not rejected anywhere).
fn warn_dropped_conveyor_compat(
    name: &str,
    compat: &datamodel::Compat,
    warnings: &mut Vec<ExportWarning>,
) {
    if compat.conveyor.is_some() {
        warnings.push(ExportWarning::new(format!(
            "'{name}' is a conveyor stock, which Vensim MDL cannot represent; \
             its conveyor semantics (transit time, capacity, leakage routing) \
             were dropped and it was exported as a plain INTEG stock"
        )));
    }
    if compat.queue.is_some() {
        warnings.push(ExportWarning::new(format!(
            "'{name}' is a queue stock, which Vensim MDL cannot represent; \
             its FIFO queue semantics were dropped and it was exported as a \
             plain INTEG stock"
        )));
    }
    if compat.leakage.is_some() {
        warnings.push(ExportWarning::new(format!(
            "'{name}' is a conveyor leakage flow, which Vensim MDL cannot \
             represent; the leak marker (and any leak fraction) was dropped \
             and it was exported as an ordinary flow"
        )));
    }
    if compat.spreadflow.is_some() {
        warnings.push(ExportWarning::new(format!(
            "'{name}' selects a conveyor inflow placement (spreadflow), which \
             Vensim MDL cannot represent; the marker was dropped and it was \
             exported as an ordinary flow"
        )));
    }
    if compat.overflow {
        warnings.push(ExportWarning::new(format!(
            "'{name}' is a queue overflow outflow, which Vensim MDL cannot \
             represent; the marker was dropped and it was exported as an \
             ordinary flow"
        )));
    }
}

/// Record a warning for a graphical-function kind that MDL cannot represent
/// faithfully (#854):
///
/// - **Discrete** (`hold-last` / step interpolation) has no Vensim MDL
///   equivalent; the table is exported as an ordinary piecewise-linear
///   (continuous) lookup.
/// - **Extrapolate** on an *embedded* `WITH LOOKUP` variable: the inline table
///   is unnamed, so no `TABXL` call site can mark it extrapolating; it is
///   exported clamped (continuous). A *standalone* Extrapolate lookup is
///   handled losslessly by rewriting its call sites to `TABXL`
///   ([`WriterContext::is_extrapolating_lookup`]) and so does NOT warn here.
fn warn_unrepresentable_gf_kinds(
    name: &str,
    ident: &str,
    equation: &Equation,
    gf: Option<&GraphicalFunction>,
    ctx: &WriterContext,
    warnings: &mut Vec<ExportWarning>,
) {
    use datamodel::GraphicalFunctionKind::{Continuous, Discrete, Extrapolate};

    // The scalar / apply-to-all graphical function (a per-element arrayed GF is
    // inspected separately below against its own equation form).
    if let Some(gf) = gf {
        let is_standalone =
            matches!(equation, Equation::Scalar(eqn) if is_lookup_only_equation(eqn));
        // A standalone Extrapolate lookup normally round-trips via the TABXL
        // rewrite of its LOOKUP call sites (#854) -- but only when a call site
        // exists. An unreferenced (e.g. dead) table has none, so no TABXL is
        // emitted and its kind clamps to Continuous on re-import; that must
        // warn rather than be silently suppressed (#856).
        let preserved_by_tabxl = is_standalone
            && !ctx.is_unreferenced_extrapolating_lookup(&crate::common::canonicalize(ident));
        match gf.kind {
            Discrete => warnings.push(discrete_gf_warning(name)),
            Extrapolate if !is_standalone => warnings.push(embedded_extrapolate_gf_warning(name)),
            Extrapolate if !preserved_by_tabxl => {
                warnings.push(unreferenced_extrapolate_gf_warning(name))
            }
            Continuous | Extrapolate => {}
        }
    }

    if let Equation::Arrayed(dims, elements, _, _) = equation {
        for (elem_key, _elem_eqn, _comment, elem_gf) in elements {
            let Some(elem_gf) = elem_gf else { continue };
            let elem_name = format!("{name}[{}]", format_mdl_element_key(elem_key, dims, ctx));
            match elem_gf.kind {
                Discrete => warnings.push(discrete_gf_warning(&elem_name)),
                // The standalone-lookup TABXL rewrite only covers scalar
                // lookup-only tables, so ANY Extrapolate on a per-element
                // arrayed GF (standalone or embedded) is unpreserved here.
                Extrapolate => warnings.push(ExportWarning::new(format!(
                    "graphical function for '{elem_name}' extrapolates, but a \
                     per-element arrayed lookup's extrapolate kind cannot be \
                     preserved in MDL; exported clamped (continuous) instead"
                ))),
                Continuous => {}
            }
        }
    }
}

/// Record warnings for the group banner's known reread lossiness (#856): the
/// MDL group marker stores the name on a single line that the reader's
/// `try_group_star` stops at the first whitespace, and the group doc is skipped
/// entirely on re-import. So a multi-word (or underscored) group name is
/// truncated to its first word, and any group documentation is dropped.
fn warn_group_lossiness(group: &datamodel::ModelGroup, warnings: &mut Vec<ExportWarning>) {
    let banner_name = underbar_to_space(&group.name);
    if banner_name.contains(char::is_whitespace) {
        warnings.push(ExportWarning::new(format!(
            "group name '{}' contains spaces; Vensim MDL stores it on a single \
             banner line and truncates it to its first word on re-import",
            group.name
        )));
    }
    if group.doc.as_deref().is_some_and(|d| !d.trim().is_empty()) {
        warnings.push(ExportWarning::new(format!(
            "documentation for group '{}' is dropped on re-import (Vensim MDL does \
             not preserve group-marker documentation)",
            group.name
        )));
    }
}

/// Record a warning per `loop_metadata` entry: MDL has no construct for any
/// of it (the MDL reader never produces `loop_metadata`), so every entry is
/// dropped on export. Four arms, each of which a user would miss: a named
/// loop (its name and description), an unnamed loop with a description, a
/// hidden-loop marker (`deleted`), and an unnamed non-deleted entry -- which
/// is not inert: `db/sync.rs` treats every non-deleted entry as an LTM
/// pinned loop and layout uses it as a fallback, so dropping it changes
/// analysis, not just labels.
fn warn_dropped_loop_metadata(model: &datamodel::Model, warnings: &mut Vec<ExportWarning>) {
    for lm in &model.loop_metadata {
        let message = if !lm.name.is_empty() {
            format!(
                "loop name '{}' (and its description) has no MDL representation and \
                 was dropped on export; Vensim MDL does not store loop metadata",
                lm.name
            )
        } else if !lm.description.is_empty() {
            format!(
                "the description of the unnamed loop over variable uids {:?} has no MDL \
                 representation and was dropped on export; Vensim MDL does not store \
                 loop metadata",
                lm.uids
            )
        } else if lm.deleted {
            format!(
                "the hidden-loop marker for the loop over variable uids {:?} has no MDL \
                 representation and was dropped on export",
                lm.uids
            )
        } else {
            format!(
                "the pinned loop over variable uids {:?} (unnamed loop metadata, used as \
                 an LTM pin) has no MDL representation and was dropped on export",
                lm.uids
            )
        };
        warnings.push(ExportWarning::new(message));
    }
}

fn discrete_gf_warning(name: &str) -> ExportWarning {
    ExportWarning::new(format!(
        "graphical function for '{name}' uses discrete (hold-last) interpolation, \
         which Vensim MDL cannot represent; exported as a continuous \
         (piecewise-linear) lookup instead"
    ))
}

fn embedded_extrapolate_gf_warning(name: &str) -> ExportWarning {
    ExportWarning::new(format!(
        "graphical function for '{name}' extrapolates, but it is an inline WITH \
         LOOKUP whose table cannot be referenced by a TABXL call site; exported \
         clamped (continuous) instead"
    ))
}

fn unreferenced_extrapolate_gf_warning(name: &str) -> ExportWarning {
    ExportWarning::new(format!(
        "graphical function for '{name}' extrapolates, but its lookup table has \
         no LOOKUP call site to emit as a kind-preserving TABXL; exported \
         clamped (continuous) instead"
    ))
}

fn compat_get_direct_equation(compat: &datamodel::Compat) -> Option<String> {
    let ds = compat.data_source.as_ref()?;
    // Vensim's GET DIRECT argument parser uses single quotes as toggle
    // delimiters with no escape mechanism, so we pass arguments through
    // unmodified rather than producing `\'` which would be unparsable.
    let quote = |s: &str| s.to_string();
    let eq = match ds.kind {
        datamodel::DataSourceKind::Data => format!(
            "{{GET DIRECT DATA('{}', '{}', '{}', '{}')}}",
            quote(&ds.file),
            quote(&ds.tab_or_delimiter),
            quote(&ds.row_or_col),
            quote(&ds.cell)
        ),
        datamodel::DataSourceKind::Constants => {
            if ds.cell.is_empty() {
                format!(
                    "{{GET DIRECT CONSTANTS('{}', '{}', '{}')}}",
                    quote(&ds.file),
                    quote(&ds.tab_or_delimiter),
                    quote(&ds.row_or_col)
                )
            } else {
                format!(
                    "{{GET DIRECT CONSTANTS('{}', '{}', '{}', '{}')}}",
                    quote(&ds.file),
                    quote(&ds.tab_or_delimiter),
                    quote(&ds.row_or_col),
                    quote(&ds.cell)
                )
            }
        }
        datamodel::DataSourceKind::Lookups => format!(
            "{{GET DIRECT LOOKUPS('{}', '{}', '{}', '{}')}}",
            quote(&ds.file),
            quote(&ds.tab_or_delimiter),
            quote(&ds.row_or_col),
            quote(&ds.cell)
        ),
        datamodel::DataSourceKind::Subscript => format!(
            "{{GET DIRECT SUBSCRIPT('{}', '{}', '{}', '{}', '')}}",
            quote(&ds.file),
            quote(&ds.tab_or_delimiter),
            quote(&ds.row_or_col),
            quote(&ds.cell)
        ),
    };
    Some(eq)
}

/// Reconstruct a stock's INTEG equation from its decomposed fields.
///
/// The datamodel stores stocks with the initial value in `equation` and
/// inflows/outflows as separate string vectors.  The MDL format requires
/// `INTEG(net_flow, initial_value)`.
///
/// The stock's `compat` is honored the same way the aux/flow path honors it
/// (#857): an ACTIVE INITIAL initial value is re-wrapped via
/// [`wrap_active_initial`], and a GET DIRECT data-source initial is
/// reconstructed via [`compat_get_direct_equation`]. Both would otherwise be
/// silently lost, replacing the stock's real initial with the runtime
/// expression alone.
fn write_stock_variable(
    buf: &mut String,
    stock: &datamodel::Stock,
    display_names: &HashMap<String, String>,
    ctx: &WriterContext,
    warnings: &mut Vec<ExportWarning>,
) {
    // The sets the engine integrates (`datamodel::distinct_stock_flows`): a
    // repeat written into the INTEG would make Vensim integrate the flow twice.
    let inflows = datamodel::distinct_stock_flows(&stock.inflows).flows;
    let outflows = datamodel::distinct_stock_flows(&stock.outflows).flows;
    let mut net_flow = String::new();
    for (i, inflow) in inflows.iter().enumerate() {
        if i > 0 {
            net_flow.push('+');
        }
        net_flow.push_str(&ctx.reference(inflow));
    }
    for outflow in &outflows {
        net_flow.push('-');
        net_flow.push_str(&ctx.reference(outflow));
    }
    if net_flow.is_empty() {
        net_flow.push('0');
    }

    let name = display_name_for_ident(&stock.ident, display_names);
    warn_dropped_non_negative(&name, &stock.compat, warnings);
    warn_dropped_conveyor_compat(&name, &stock.compat, warnings);

    // Reconstruct the INITIAL value from the stock's compat the same way the
    // aux/flow path does: a GET DIRECT data source takes precedence, otherwise
    // an ACTIVE INITIAL wrap. The result is XMILE-form text (or the brace-
    // stripped GET DIRECT form) that `equation_to_mdl` renders to MDL.
    let data_source_eqn = compat_get_direct_equation(&stock.compat);
    let stock_initial = |eqn: &str, warnings: &mut Vec<ExportWarning>| -> String {
        let initial_src = data_source_eqn
            .clone()
            .unwrap_or_else(|| wrap_active_initial(eqn, &stock.compat));
        equation_to_mdl(&initial_src, &name, ctx, warnings)
    };

    match &stock.equation {
        Equation::Scalar(eqn) => {
            let initial = stock_initial(eqn, warnings);
            write_stock_entry(
                buf,
                &name,
                &net_flow,
                &initial,
                &[],
                &stock.units,
                &stock.documentation,
            )
        }
        Equation::ApplyToAll(dims, eqn) => {
            let dim_names: Vec<&str> = dims.iter().map(|d| d.as_str()).collect();
            let initial = stock_initial(eqn, warnings);
            write_stock_entry(
                buf,
                &name,
                &net_flow,
                &initial,
                &dim_names,
                &stock.units,
                &stock.documentation,
            );
        }
        Equation::Arrayed(dims, slots, default, has_except_default) => {
            let var = arrayed::Arrayed {
                name: &name,
                dims,
                slots,
                default,
                has_except_default: *has_except_default,
                rhs: arrayed::Rhs::Stock {
                    net_flow: &net_flow,
                    compat: &stock.compat,
                },
                units: &stock.units,
                doc: &stock.documentation,
            };
            arrayed::write_arrayed(buf, &var, ctx, warnings);
        }
    }
}

fn normalized_stock_initial(initial: &str) -> String {
    if initial.trim().is_empty() {
        "0".to_owned()
    } else {
        initial.to_owned()
    }
}

/// `name` is the pre-formatted display name (with original casing).
fn write_stock_entry(
    buf: &mut String,
    name: &str,
    net_flow: &str,
    initial: &str,
    dims: &[&str],
    units: &Option<String>,
    doc: &str,
) {
    let initial = normalized_stock_initial(initial);

    if dims.is_empty() {
        write!(buf, "{name}=").unwrap();
    } else {
        let dim_strs: Vec<String> = dims.iter().map(|d| format_mdl_ident(d)).collect();
        write!(buf, "{name}[{}]=", dim_strs.join(",")).unwrap();
    }

    buf.push_str("\n\t");
    buf.push_str(&format!("INTEG({net_flow}, {initial})"));
    write_units_and_comment(buf, units, doc);
}

/// If a variable has ACTIVE INITIAL metadata, wrap the equation.
///
/// The datamodel stores `ACTIVE INITIAL(expr, init)` as:
/// - `equation` = expr (the runtime expression)
/// - `compat.active_initial` = Some(init) (the initial value)
fn wrap_active_initial(eqn: &str, compat: &datamodel::Compat) -> String {
    match &compat.active_initial {
        Some(init) => wrap_initial(eqn, init),
        None => eqn.to_owned(),
    }
}

/// `eqn` with the initial equation `init`, as ACTIVE INITIAL. Both are in
/// XMILE form (underscores), so the wrap is `init(eqn, init)` in XMILE form,
/// which `equation_to_mdl` parses as a whole, maps to ACTIVE INITIAL, and
/// respells in spaced MDL form.
fn wrap_initial(eqn: &str, init: &str) -> String {
    format!("init({eqn}, {init})")
}

/// Split an MDL equation string into tokens suitable for line wrapping.
///
/// Tokens preserve the original text exactly -- concatenating them yields
/// the input.  The split points are chosen so that line breaks can be
/// inserted *between* tokens at natural boundaries: after commas (with
/// their trailing space), before binary operators, or after open parens.
fn tokenize_for_wrapping(eqn: &str) -> Vec<String> {
    let mut tokens: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut chars = eqn.chars().peekable();

    while let Some(&c) = chars.peek() {
        match c {
            ',' => {
                current.push(chars.next().unwrap());
                // Absorb trailing space after comma so it stays with the comma token
                if chars.peek() == Some(&' ') {
                    current.push(chars.next().unwrap());
                }
                tokens.push(std::mem::take(&mut current));
            }
            '(' | ')' | '[' | ']' => {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
                current.push(chars.next().unwrap());
                tokens.push(std::mem::take(&mut current));
            }
            '+' | '-' | '*' | '/' | '^' => {
                // Emit the accumulated text before the operator so a
                // line break can be inserted before the operator.
                // But first check if this minus/plus is at the very start
                // or follows an operator/open-paren (i.e. is unary).
                let is_unary = current.is_empty()
                    && tokens.last().is_none_or(|t| {
                        let trimmed = t.trim();
                        trimmed.is_empty()
                            || trimmed.ends_with('(')
                            || trimmed.ends_with(',')
                            || trimmed == "+"
                            || trimmed == "-"
                            || trimmed == "*"
                            || trimmed == "/"
                            || trimmed == "^"
                    });
                if is_unary {
                    current.push(chars.next().unwrap());
                } else {
                    if !current.is_empty() {
                        tokens.push(std::mem::take(&mut current));
                    }
                    // Emit the operator as its own token so line breaks can be inserted before it.
                    current.push(chars.next().unwrap());
                    tokens.push(std::mem::take(&mut current));
                }
            }
            '\'' => {
                // Quoted literal -- consume the whole thing as one piece
                current.push(chars.next().unwrap());
                while let Some(&ch) = chars.peek() {
                    current.push(chars.next().unwrap());
                    if ch == '\'' {
                        break;
                    }
                }
            }
            '"' => {
                // Quoted identifier -- consume the whole thing
                current.push(chars.next().unwrap());
                while let Some(&ch) = chars.peek() {
                    current.push(chars.next().unwrap());
                    if ch == '"' {
                        break;
                    }
                }
            }
            _ => {
                current.push(chars.next().unwrap());
            }
        }
    }

    if !current.is_empty() {
        tokens.push(current);
    }

    tokens
}

/// Wrap a long equation with backslash-newline continuations in Vensim style.
///
/// Short equations (fitting within `max_line_len` characters) pass through
/// unchanged.  Longer ones are split at token boundaries with `\\\n\t\t`
/// continuation sequences (backslash, newline, two tabs for continuation
/// indent under the single-tab equation indent).
fn wrap_equation_with_continuations(eqn: &str, max_line_len: usize) -> String {
    if eqn.len() <= max_line_len {
        return eqn.to_string();
    }

    let tokens = tokenize_for_wrapping(eqn);
    let mut result = String::new();
    let mut current_line_len: usize = 0;

    for token in &tokens {
        // If adding this token would exceed the limit and we already have
        // content on the current line, break before it.
        if current_line_len + token.len() > max_line_len && current_line_len > 0 {
            // Trim trailing whitespace from the current line before the break
            let trimmed_end = result.trim_end_matches(' ').len();
            result.truncate(trimmed_end);
            result.push_str("\\\n\t\t");
            current_line_len = 0;
        }
        result.push_str(token);
        current_line_len += token.len();
    }

    result
}

/// Write one MDL entry (scalar or apply-to-all).
///
/// `name` is the pre-formatted display name (with original casing from
/// view elements, or `format_mdl_ident` fallback).
#[allow(clippy::too_many_arguments)]
fn write_single_entry(
    buf: &mut String,
    name: &str,
    eqn: &str,
    dims: &[&str],
    units: &Option<String>,
    doc: &str,
    gf: Option<&GraphicalFunction>,
    ctx: &WriterContext,
    warnings: &mut Vec<ExportWarning>,
) {
    let dim_suffix = if dims.is_empty() {
        String::new()
    } else {
        let dim_strs: Vec<String> = dims.iter().map(|d| format_mdl_ident(d)).collect();
        format!("[{}]", dim_strs.join(","))
    };

    if let Some(gf) = gf {
        if is_lookup_only_equation(eqn) {
            // Standalone lookup definition: name(\n\tbody)
            write!(buf, "{name}{dim_suffix}(").unwrap();
            buf.push_str("\n\t");
            write_lookup_body(buf, gf);
            buf.push(')');
        } else {
            // Embedded lookup: name=\n\tWITH LOOKUP(input, (body))
            let assign_op = if is_external_data_placeholder(eqn) {
                ":="
            } else {
                "="
            };
            write!(buf, "{name}{dim_suffix}{assign_op}").unwrap();
            let mdl_eqn = equation_to_mdl(eqn, name, ctx, warnings);
            buf.push_str("\n\tWITH LOOKUP(");
            buf.push_str(&mdl_eqn);
            buf.push_str(", ");
            write_lookup(buf, gf);
            buf.push(')');
        }
    } else {
        let assign_op = if is_external_data_placeholder(eqn) {
            ":="
        } else {
            "="
        };
        let mdl_eqn = equation_to_mdl(eqn, name, ctx, warnings);

        // Short, single-line equations use inline format with spaces around
        // the operator (e.g. `average repayment rate = 0.03`).  Longer or
        // multiline equations use the traditional Vensim multiline format.
        let inline_line = format!("{name}{dim_suffix} {assign_op} {mdl_eqn}")
            .trim_end()
            .to_owned();
        if inline_line.len() <= 80 && !mdl_eqn.contains('\n') {
            buf.push_str(&inline_line);
        } else {
            write!(buf, "{name}{dim_suffix}{assign_op}").unwrap();
            let wrapped = wrap_equation_with_continuations(&mdl_eqn, 80);
            buf.push_str("\n\t");
            buf.push_str(&wrapped);
        }
    }

    write_units_and_comment(buf, units, doc);
}

/// Append the `~\tunits\n\t~\tcomment\n\t|` trailer.
///
/// Both free-text fields route through `sanitize_free_text` (GH #849): a raw
/// `~`/`|` in units, a `|` in the comment, an embedded section terminator, or an
/// accumulated carriage return would otherwise corrupt or destabilize the file.
fn write_units_and_comment(buf: &mut String, units: &Option<String>, doc: &str) {
    buf.push_str("\n\t~\t");
    if let Some(u) = units {
        // Units are tokenized between the two `~`, so `~` (the field separator)
        // is structural; line breaks are insignificant whitespace.
        buf.push_str(&sanitize_free_text(u, FreeTextLineMode::SingleLine, &['~']));
    }
    buf.push_str("\n\t~\t");
    // The `~`-comment is raw text up to `|`; multi-line is legal. The reader
    // trims it (`EquationReader::capture_comment`), so it is written trimmed
    // and reads back as written.
    buf.push_str(sanitize_free_text(doc, FreeTextLineMode::Multiline, &[]).trim());
    buf.push_str("\n\t|");
}

/// One item to emit in the ungrouped-variables section: either an ordinary
/// model variable or a reconstructed multi-output macro invocation. Both
/// carry a sort key (the variable's / the LHS's ident) so the whole
/// section stays alphabetically ordered and deterministic across passes.
enum UngroupedEntry<'a> {
    Variable(&'a datamodel::Variable),
    /// `(lhs_ident, full_entry_text)` -- the entry text is the complete
    /// `<lhs> = <macro>(<args> : <bindings>)\n\t~\tunits\n\t~\tdoc\n\t|`
    /// block (no trailing newline; the caller adds it like a normal entry).
    Reconstructed(&'a str, &'a str),
}

impl UngroupedEntry<'_> {
    fn sort_key(&self) -> &str {
        match self {
            UngroupedEntry::Variable(v) => v.get_ident(),
            UngroupedEntry::Reconstructed(lhs, _) => lhs,
        }
    }
}

/// The result of detecting Phase-4-materialized multi-output clusters in a
/// model: the idents to suppress from normal per-variable emission (each
/// `Variable::Module` plus its binding auxes) and the reconstructed
/// `<lhs> = <macro>(...)` entries to emit instead, paired with their LHS
/// ident for stable interleaving into the sorted ungrouped section.
///
/// `failed` holds one human-readable message per macro-backed
/// `Variable::Module` that resolves to a macro but whose materialized
/// cluster is *not* faithfully reconstructable (a binding aux or argument
/// wiring was deleted/renamed post-import -- e.g. an MCP patch). The caller
/// MUST treat a non-empty `failed` as a hard error: silently emitting the
/// rest would drop the invocation and leave the surviving binding auxes
/// referencing a module that is no longer written -- corrupt `.mdl`. This
/// mirrors `project_to_mdl`'s ordinary-`Variable::Module` reject gate; a
/// macro-backed module is only *conditionally* admitted by that gate, on
/// the assumption -- enforced here -- that its cluster is well-formed.
#[derive(Default)]
struct MultiOutputReconstruction {
    suppressed: HashSet<String>,
    entries: Vec<(String, String)>,
    failed: Vec<String>,
}

/// Detect every Phase-4-materialized multi-output invocation cluster in
/// `model` and reconstruct it into the Vensim `:` call syntax.
///
/// A cluster is a `Variable::Module` whose `model_name` resolves to a
/// macro-marked model in `project.models` (looked up via a `name ->
/// &MacroSpec` map built here). For each such module:
///
/// - **Arguments** are recovered positionally: each `ModuleReference.dst`
///   is `"{module_ident}.{param}"`; the bare `{param}` is matched against
///   `MacroSpec.parameters[i]` to get the call position (Phase 4 does NOT
///   guarantee `Module.references` is in positional order), and `src` is
///   the argument's name. A reference whose param does not appear in the
///   spec is skipped defensively.
/// - **Binding auxes** are the main-model `Variable::Aux`es whose `Scalar`
///   equation is exactly `"{module_ident}.{output}"` (an ASCII period --
///   Phase 4's materialized binding-aux equation text is the datamodel
///   `.` form, not the canonical `·`, since `project_to_mdl` operates on
///   datamodel equation strings). The aux reading
///   `{module_ident}.{primary_output}` is the LHS variable; the auxes
///   reading `{module_ident}.{additional_outputs[j]}` are the `:`-list
///   output bindings in order `j`.
/// - The reconstruction is `<lhs> = <MACRO>(<arg1>, ..., <argN> :
///   <addbind1>, ..., <addbindM>)` plus the standard `~ units ~ doc |`
///   trailer, using the primary-output binding aux's units/documentation
///   (its `~`-doc is the original invocation's comment, preserved by
///   Phase 4).
///
/// A module whose `model_name` is *not* a macro is genuinely none of this
/// function's business and is skipped silently (ordinary submodules are
/// rejected earlier by `project_to_mdl`'s gate). But a module that *is*
/// macro-backed yet whose cluster is incomplete (a missing positional
/// argument, primary binding, or additional-output binding -- the result
/// of a post-import edit such as an MCP delete/rename patch) is **not**
/// silently skipped: a faithful `:` reconstruction is impossible, so the
/// reason is recorded on `MultiOutputReconstruction::failed`. The caller
/// turns any such failure into a hard error rather than letting the module
/// fall through to `write_variable_entry`'s `Variable::Module(_) => return`
/// (which would emit nothing and silently corrupt the `.mdl` -- the
/// surviving binding auxes would dangle on an unwritten module). The
/// pre-macro writer relied on that no-op fallback because it never admitted
/// any `Variable::Module`; the macro gate admits macro-backed ones, so the
/// writer must now enforce their well-formedness itself.
fn build_multi_output_reconstructions(
    model: &datamodel::Model,
    project: &datamodel::Project,
    display_names: &HashMap<String, String>,
) -> MultiOutputReconstruction {
    let macro_specs: HashMap<&str, &datamodel::MacroSpec> = project
        .models
        .iter()
        .filter_map(|m| m.macro_spec.as_ref().map(|s| (m.name.as_str(), s)))
        .collect();
    if macro_specs.is_empty() {
        return MultiOutputReconstruction::default();
    }

    // Index the scalar-equation auxes once: trimmed equation text ->
    // (ident, doc, units). A binding aux's equation is exactly
    // `{module_ident}.{output}`.
    //
    // The `or_insert` first-wins is *unreachable for binding-aux
    // detection*: a module's distinct outputs produce distinct
    // `{module_ident}.{output}` keys, so the two binding auxes of one
    // module instance can never collide on a key (a collision would
    // require two auxes with the byte-identical scalar equation -- an
    // alias pair -- not two bindings of the same module). The first-wins
    // choice is retained anyway to intentionally mirror the documented
    // first-wins behavior of `crate::xmile::model::extract_macro_invocations`
    // (the Phase-5 XMILE sibling path): under aliasing only the
    // first-encountered aux becomes the binding; any alias stays an
    // ordinary aux that references the regenerated module output by name
    // and still round-trips.
    let mut scalar_auxes: HashMap<&str, (&str, &str, &Option<String>)> = HashMap::new();
    for v in &model.variables {
        if let datamodel::Variable::Aux(aux) = v
            && let Equation::Scalar(eq) = &aux.equation
        {
            scalar_auxes.entry(eq.trim()).or_insert((
                aux.ident.as_str(),
                aux.documentation.as_str(),
                &aux.units,
            ));
        }
    }

    let mut result = MultiOutputReconstruction::default();

    for v in &model.variables {
        let datamodel::Variable::Module(module) = v else {
            continue;
        };
        let Some(spec) = macro_specs.get(module.model_name.as_str()) else {
            continue;
        };

        // Recover arguments in positional MacroSpec.parameters order.
        let prefix = format!("{}.", module.ident);
        let mut args: Vec<Option<&str>> = vec![None; spec.parameters.len()];
        for r in &module.references {
            let param = r.dst.strip_prefix(&prefix).unwrap_or(r.dst.as_str());
            if let Some(pos) = spec.parameters.iter().position(|p| p == param) {
                args[pos] = Some(r.src.as_str());
            }
        }
        // A missing positional argument means the cluster is malformed
        // (an arity the writer cannot faithfully reconstruct). Record it
        // as a hard failure instead of silently dropping the invocation.
        if args.iter().any(Option::is_none) {
            let missing: Vec<&str> = spec
                .parameters
                .iter()
                .zip(&args)
                .filter_map(|(p, a)| a.is_none().then_some(p.as_str()))
                .collect();
            result.failed.push(format!(
                "macro-module `{}` invoking macro `{}`: missing argument \
                 wiring for parameter(s) {:?}; the materialized multi-output \
                 cluster was edited and can no longer be exported faithfully",
                module.ident, module.model_name, missing
            ));
            continue;
        }

        // Primary-output binding: the aux reading `{module}.{primary}`.
        let primary_key = format!("{}.{}", module.ident, spec.primary_output);
        let Some(&(primary_ident, primary_doc, primary_units)) =
            scalar_auxes.get(primary_key.as_str())
        else {
            result.failed.push(format!(
                "macro-module `{}` invoking macro `{}`: no binding aux reads \
                 `{}` (the primary output `{}`); the materialized \
                 multi-output cluster is missing its primary binding and \
                 cannot be reconstructed",
                module.ident, module.model_name, primary_key, spec.primary_output
            ));
            continue;
        };

        // One additional-output binding per `:`-list entry, in order.
        let mut output_bindings: Vec<&str> = Vec::with_capacity(spec.additional_outputs.len());
        let mut missing_output: Option<(&str, String)> = None;
        for out_name in &spec.additional_outputs {
            let key = format!("{}.{}", module.ident, out_name);
            match scalar_auxes.get(key.as_str()) {
                Some(&(binding, _, _)) => output_bindings.push(binding),
                None => {
                    missing_output = Some((out_name.as_str(), key));
                    break;
                }
            }
        }
        if let Some((out_name, key)) = missing_output {
            result.failed.push(format!(
                "macro-module `{}` invoking macro `{}`: no binding aux reads \
                 `{}` (additional output `{}`); the materialized multi-output \
                 cluster is missing an output binding and cannot be \
                 reconstructed",
                module.ident, module.model_name, key, out_name
            ));
            continue;
        }

        // Build the reconstructed `<lhs> = <MACRO>(<args> : <bindings>)`.
        let macro_name = display_name_for_ident(&module.model_name, display_names);
        let arg_list = args
            .iter()
            .map(|a| display_name_for_ident(a.expect("checked above"), display_names))
            .collect::<Vec<_>>()
            .join(", ");
        let mut call = format!("{macro_name}({arg_list}");
        if !output_bindings.is_empty() {
            let binding_list = output_bindings
                .iter()
                .map(|b| display_name_for_ident(b, display_names))
                .collect::<Vec<_>>()
                .join(", ");
            write!(call, " : {binding_list}").unwrap();
        }
        call.push(')');

        let lhs_name = display_name_for_ident(primary_ident, display_names);
        let mut entry = String::new();
        // Match write_single_entry's inline-vs-multiline rule so the
        // reconstruction formats like any other short equation.
        let inline_line = format!("{lhs_name} = {call}");
        if inline_line.len() <= 80 {
            entry.push_str(&inline_line);
        } else {
            write!(entry, "{lhs_name}=\n\t{call}").unwrap();
        }
        write_units_and_comment(&mut entry, primary_units, primary_doc);

        // Suppress the module + every binding aux from normal emission.
        result.suppressed.insert(module.ident.clone());
        result.suppressed.insert(primary_ident.to_owned());
        for b in &output_bindings {
            result.suppressed.insert((*b).to_owned());
        }
        result.entries.push((primary_ident.to_owned(), entry));
    }

    result
}

/// Write a dimension definition in MDL format.
///
/// Named:   `DimName: Elem1, Elem2, Elem3 ~~|`
/// Indexed: `DimName: (1-N) ~~|`
/// Mapped:  `DimName: Elem1, Elem2 -> MappedDim ~~|`
/// Element-mapped: `DimName: A1, A2 -> (MappedDim: B2, B1) ~~|`
/// Mapped through a subrange: `DimB: B1, B2 -> (DimA: SubA, A3) ~~|`, where
/// B1 maps to every element of SubA
///
/// Test-only wrapper that discards any [`ExportWarning`]s; production emission
/// uses [`write_dimension_def_warn`] (#856). Not public, so no caller can lose
/// the warning channel by accident.
#[cfg(test)]
fn write_dimension_def(
    buf: &mut String,
    dim: &datamodel::Dimension,
    dimensions: &[datamodel::Dimension],
) {
    write_dimension_def_warn(buf, dim, dimensions, &mut Vec::new());
}

/// The dimension, of `dimensions`, whose named elements are exactly
/// `elements` (canonical spellings), preferring a subrange of `target`: the
/// name a mapping writes for a source element that maps to several of
/// `target`'s elements. None when no dimension names exactly those elements.
fn dimension_naming<'a>(
    elements: &[&str],
    target: &str,
    dimensions: &'a [datamodel::Dimension],
) -> Option<&'a datamodel::Dimension> {
    let wanted: HashSet<String> = elements.iter().map(|e| to_lower_space(e)).collect();
    let target = to_lower_space(target);
    let names_them = |dim: &&datamodel::Dimension| match &dim.elements {
        DimensionElements::Named(named) => {
            to_lower_space(&dim.name) != target
                && named.len() == wanted.len()
                && named.iter().all(|e| wanted.contains(&to_lower_space(e)))
        }
        DimensionElements::Indexed(_) => false,
    };
    let is_subrange_of_target = |dim: &datamodel::Dimension| {
        dim.parent
            .as_deref()
            .is_some_and(|parent| to_lower_space(parent) == target)
    };
    let candidates: Vec<&datamodel::Dimension> = dimensions.iter().filter(names_them).collect();
    candidates
        .iter()
        .find(|dim| is_subrange_of_target(dim))
        .or_else(|| candidates.first())
        .copied()
}

fn write_dimension_def_warn(
    buf: &mut String,
    dim: &datamodel::Dimension,
    dimensions: &[datamodel::Dimension],
    warnings: &mut Vec<ExportWarning>,
) {
    let name = format_mdl_ident(&dim.name);
    write!(buf, "{name}:").unwrap();

    match &dim.elements {
        DimensionElements::Named(elems) => {
            buf.push_str("\n\t");
            let elem_strs: Vec<String> = elems.iter().map(|e| format_mdl_ident(e)).collect();
            buf.push_str(&elem_strs.join(", "));
        }
        DimensionElements::Indexed(size) => {
            // Vensim's subscript elements are names, and a numeric range is a
            // range of names that end in a number (`(A1-A5)`).
            let size = *size as usize;
            let first = indexed_element_name(&dim.name, 1);
            if size > 1 {
                let last = indexed_element_name(&dim.name, size);
                write!(buf, "\n\t({first}-{last})").unwrap();
            } else {
                write!(buf, "\n\t{first}").unwrap();
            }
        }
    }

    if let Some(maps_to) = dim.maps_to() {
        write!(buf, " -> {}", format_mdl_ident(maps_to)).unwrap();
    } else if !dim.mappings.is_empty() {
        // Build a source-position index so element-level mappings emit
        // targets in the same order as the source dimension's elements.
        let source_positions: HashMap<String, usize> = match &dim.elements {
            DimensionElements::Named(elems) => elems
                .iter()
                .enumerate()
                .map(|(i, e)| (to_lower_space(e), i))
                .collect(),
            DimensionElements::Indexed(_) => HashMap::new(),
        };
        let parts: Vec<String> = dim
            .mappings
            .iter()
            .map(|mapping| {
                if mapping.element_map.is_empty() {
                    return format_mdl_ident(&mapping.target);
                }
                // Each source element's targets, in the order the map
                // first names them, then in source element order. A source
                // element mapping to several targets (the reader expands a
                // subrange named in the list) is written as the dimension
                // that holds exactly those targets.
                let mut by_source: Vec<(&str, Vec<&str>)> = Vec::new();
                let mut index: HashMap<&str, usize> = HashMap::new();
                for (src, tgt) in &mapping.element_map {
                    let at = *index.entry(src).or_insert_with(|| {
                        by_source.push((src, Vec::new()));
                        by_source.len() - 1
                    });
                    by_source[at].1.push(tgt);
                }
                by_source.sort_by_key(|(src, _)| {
                    source_positions.get(*src).copied().unwrap_or(usize::MAX)
                });
                let mut target_names = Vec::with_capacity(by_source.len());
                for (_, targets) in &by_source {
                    match targets.as_slice() {
                        [one] => target_names.push(format_mdl_ident(one)),
                        several => {
                            let Some(subrange) =
                                dimension_naming(several, &mapping.target, dimensions)
                            else {
                                warnings.push(ExportWarning::new(format!(
                                    "dimension '{}' maps an element to several elements of \
                                     '{}' that no dimension names, which MDL cannot write; \
                                     exported as a plain dimension-name mapping \
                                     (element-level detail lost)",
                                    dim.name, mapping.target
                                )));
                                return format_mdl_ident(&mapping.target);
                            };
                            target_names.push(format_mdl_ident(&subrange.name));
                        }
                    }
                }
                format!(
                    "({}: {})",
                    format_mdl_ident(&mapping.target),
                    target_names.join(", ")
                )
            })
            .collect();
        write!(buf, " -> {}", parts.join(", ")).unwrap();
    }

    buf.push_str("\n\t~~|\n");
}

// ---- Sketch element serialization ----

/// Format a view element name for an MDL sketch record.
///
/// A sketch record names its variable as the equation section does
/// ([`format_mdl_ident`]), so Vensim links the element to the variable:
/// underscores as spaces, a display newline as the quoted `\n` escape, and
/// quotes around a name holding a character that would otherwise break the
/// comma-delimited record (`$`, `|`, `/`, ...).
fn format_sketch_name(name: &str) -> String {
    format_mdl_ident(name)
}

/// Number of `type 1` pipe-connector records `write_flow_pipe_connectors`
/// emits for `flow`, whose ends `cut` says the segment cannot draw.
///
/// This MUST stay in lockstep with `write_flow_pipe_connectors_with_context`:
/// `SketchUidRemap::dense_for_segment` reserves a contiguous UID block of this
/// size for the flow's pipes, so an off-by-one here corrupts the whole
/// segment's UID numbering.
fn flow_pipe_connector_count(flow: &view_element::Flow, cut: CutEnds) -> usize {
    let n = flow.points.len();
    // Endpoint connector to the last point's attachment (the sink), emitted
    // only when the flow has more than one point.
    let sink = usize::from(n > 1 && flow.points.last().and_then(|p| p.attached_to_uid).is_some());
    // One self-connector per interior bend point, unless an end is cut: a cut
    // flow is written straight from its valve to each end.
    let bends = if cut.source.is_some() || cut.sink.is_some() {
        0
    } else {
        n.saturating_sub(2)
    };
    // Endpoint connector to the first point's attachment (the source).
    let source = usize::from(
        flow.points
            .first()
            .and_then(|p| p.attached_to_uid)
            .is_some(),
    );
    sink + bends + source
}

/// The clouds a flow's pipe ends in, in place of ends its segment cannot
/// draw: an end attached to an element the segment holds no record of. The
/// importer routes a flow to its model's stock after
/// the views merge, so a flow drawn in one view can end on a stock drawn only
/// in another, and an MDL pipe names an element of its own view. Each field is
/// the new UID of the cloud written for that end.
///
/// Such a flow is written straight, from its valve to each end: its bends lead
/// toward the other view, and a point there would grow this view's extent,
/// which composition stacks the later views by. Reading the file back, the
/// model still links the stock, so the importer routes the flow to it again
/// from the same valve and drawn end, and leaves the cloud, which then ends
/// nothing, out of the view; the next save cuts the flow the same way.
#[derive(Clone, Copy, Default)]
struct CutEnds {
    source: Option<i32>,
    sink: Option<i32>,
}

/// How far from its valve a cut end's cloud is written.
const CUT_CLOUD_DISTANCE: f64 = 40.0;

/// Where the cloud for a cut end (`end`, a point of `flow` in view
/// coordinates) is written, in the segment's coordinates: `CUT_CLOUD_DISTANCE`
/// from the valve, along the axis the end lies further along and toward it.
fn cut_cloud_point(
    flow: &view_element::Flow,
    end: &view_element::FlowPoint,
    is_sink: bool,
    transform: SketchTransform,
) -> (i32, i32) {
    let (dx, dy) = (end.x - flow.x, end.y - flow.y);
    let (x, y) = if dx == 0.0 && dy == 0.0 {
        let side = if is_sink { 1.0 } else { -1.0 };
        (flow.x + side * CUT_CLOUD_DISTANCE, flow.y)
    } else if dx.abs() >= dy.abs() {
        (flow.x + dx.signum() * CUT_CLOUD_DISTANCE, flow.y)
    } else {
        (flow.x, flow.y + dy.signum() * CUT_CLOUD_DISTANCE)
    };
    transform.point(x, y)
}

/// Remap merged/global datamodel UIDs into dense, view-local sketch IDs.
///
/// Vensim sketches use small, contiguous IDs within each `V300` section, and
/// crucially the records appear in increasing-UID order: Vensim's sketch
/// reader uses each record's UID as an array offset and stops loading the view
/// when it sees a backward reference (it tolerates gaps -- forward jumps --
/// which is why Vensim's own files skip the occasional UID).
///
/// After multi-view MDL files are merged into a single StockFlow, the
/// datamodel UIDs remain globally unique across the merged view, and the
/// per-flow records (cloud, pipes, valve, label) are spread across the element
/// list rather than emitted as a contiguous block. So the writer assigns
/// fresh UIDs in the *exact order it emits records* (see `dense_for_segment`),
/// while leaving geometry lookups keyed on the original IDs.
struct SketchUidRemap {
    /// Old (datamodel) UID -> new dense UID, for auxes, stocks, flow labels,
    /// links, aliases, and clouds.
    element_uids: HashMap<i32, i32>,
    /// Old flow UID -> new valve UID.
    valve_uids: HashMap<i32, i32>,
    /// Old flow UID -> new UID of that flow's first pipe connector. The flow's
    /// pipe connectors occupy a contiguous block immediately before the valve.
    pipe_start_uids: HashMap<i32, i32>,
    /// Old flow UID -> the clouds its cut ends are written with.
    cut_ends: HashMap<i32, CutEnds>,
}

impl SketchUidRemap {
    /// Allocate dense, 1-based UIDs in the order `write_view_segment` emits
    /// sketch records, so the output is in increasing-UID order.
    ///
    /// `flow_clouds` maps each flow UID to the clouds emitted just before that
    /// flow's pipe connectors; it must use the same per-flow ordering as
    /// `write_view_segment` (which is why the caller builds it once and shares
    /// it with both this function and the emit loop).
    fn dense_for_segment(
        elements: &[&ViewElement],
        flow_clouds: &HashMap<i32, Vec<&view_element::Cloud>>,
    ) -> Self {
        // What the segment's records draw.
        let mut drawn: HashSet<i32> = HashSet::new();
        for element in elements {
            match element {
                ViewElement::Aux(_)
                | ViewElement::Stock(_)
                | ViewElement::Link(_)
                | ViewElement::Alias(_) => {
                    drawn.insert(element.get_uid());
                }
                ViewElement::Flow(flow) => {
                    drawn.insert(flow.uid);
                    for cloud in flow_clouds.get(&flow.uid).into_iter().flatten() {
                        drawn.insert(cloud.uid);
                    }
                }
                ViewElement::Cloud(_) | ViewElement::Module(_) | ViewElement::Group(_) => {}
            }
        }
        let is_cut = |end: Option<&view_element::FlowPoint>| {
            end.and_then(|point| point.attached_to_uid)
                .is_some_and(|uid| !drawn.contains(&uid))
        };

        let mut element_uids = HashMap::new();
        let mut valve_uids = HashMap::new();
        let mut pipe_start_uids = HashMap::new();
        let mut cut_ends: HashMap<i32, CutEnds> = HashMap::new();
        let mut next_uid = 1;

        for element in elements {
            match element {
                ViewElement::Aux(aux) => {
                    element_uids.insert(aux.uid, next_uid);
                    next_uid += 1;
                }
                ViewElement::Stock(stock) => {
                    element_uids.insert(stock.uid, next_uid);
                    next_uid += 1;
                }
                ViewElement::Flow(flow) => {
                    // Emission order within a flow: its clouds, then its pipe
                    // connectors, then the valve, then the flow-label
                    // variable -- one contiguous run of UIDs (Vensim's own
                    // files have valve_uid + 1 == label_uid).
                    if let Some(clouds) = flow_clouds.get(&flow.uid) {
                        for cloud in clouds {
                            element_uids.insert(cloud.uid, next_uid);
                            next_uid += 1;
                        }
                    }
                    // A cut end's cloud follows the flow's own clouds.
                    let mut cut = CutEnds::default();
                    if is_cut(flow.points.first()) {
                        cut.source = Some(next_uid);
                        next_uid += 1;
                    }
                    if flow.points.len() > 1 && is_cut(flow.points.last()) {
                        cut.sink = Some(next_uid);
                        next_uid += 1;
                    }
                    if cut.source.is_some() || cut.sink.is_some() {
                        cut_ends.insert(flow.uid, cut);
                    }
                    pipe_start_uids.insert(flow.uid, next_uid);
                    next_uid += flow_pipe_connector_count(flow, cut) as i32;
                    valve_uids.insert(flow.uid, next_uid);
                    next_uid += 1;
                    element_uids.insert(flow.uid, next_uid);
                    next_uid += 1;
                }
                ViewElement::Link(link) => {
                    element_uids.insert(link.uid, next_uid);
                    next_uid += 1;
                }
                // Clouds are emitted with their flow, above; the standalone
                // list entry does not produce its own record.
                ViewElement::Cloud(_) => {}
                ViewElement::Alias(alias) => {
                    element_uids.insert(alias.uid, next_uid);
                    next_uid += 1;
                }
                ViewElement::Module(_) | ViewElement::Group(_) => {}
            }
        }

        Self {
            element_uids,
            valve_uids,
            pipe_start_uids,
            cut_ends,
        }
    }

    /// The clouds `flow_uid`'s cut ends are written with; none for a flow the
    /// segment draws whole.
    fn cut_ends(&self, flow_uid: i32) -> CutEnds {
        self.cut_ends.get(&flow_uid).copied().unwrap_or_default()
    }

    fn element_uid(&self, old_uid: i32) -> i32 {
        self.element_uids.get(&old_uid).copied().unwrap_or(old_uid)
    }

    fn valve_uid(&self, flow_uid: i32) -> Option<i32> {
        self.valve_uids.get(&flow_uid).copied()
    }

    /// New UID of `flow_uid`'s first pipe connector. `dense_for_segment`
    /// reserves a block for every flow it sees, so this is `Some` for any flow
    /// in the segment this remap was built from.
    fn pipe_start_uid(&self, flow_uid: i32) -> Option<i32> {
        self.pipe_start_uids.get(&flow_uid).copied()
    }
}

const STOCK_WIDTH: f64 = 45.0;
const STOCK_HEIGHT: f64 = 35.0;
const STOCK_EDGE_TOLERANCE: f64 = 1.0;

/// Write a type 10 line for an Aux element.
/// Sketch element names use `format_sketch_name` (not `format_mdl_ident`)
/// because MDL sketch lines are comma-delimited positional records where
/// quoting is not used.
#[cfg(test)]
fn write_aux_element(buf: &mut String, aux: &view_element::Aux) {
    let element = ViewElement::Aux(aux.clone());
    let remap = SketchUidRemap::dense_for_segment(&[&element], &HashMap::new());
    write_aux_element_with_context(buf, aux, SketchTransform::identity(), &remap);
}

fn write_aux_element_with_context(
    buf: &mut String,
    aux: &view_element::Aux,
    transform: SketchTransform,
    uid_remap: &SketchUidRemap,
) {
    let name = format_sketch_name(&aux.name);
    let (w, h, shape, bits) = match &aux.compat {
        Some(c) => (c.width as i32, c.height as i32, c.shape, c.bits),
        None => {
            let (w, h) = default_aux_size(&aux.name);
            (w, h, 8, 3)
        }
    };
    let (x, y) = transform.point(aux.x, aux.y);
    let tail = compat_tail(aux.compat.as_ref(), "0,0,-1,0,0,0");
    let uid = uid_remap.element_uid(aux.uid);
    write!(
        buf,
        "10,{},{},{},{},{},{},{},{},{}",
        uid, name, x, y, w, h, shape, bits, tail,
    )
    .unwrap();
}

/// Write a type 10 line for a Stock element.
#[cfg(test)]
fn write_stock_element(buf: &mut String, stock: &view_element::Stock) {
    let element = ViewElement::Stock(stock.clone());
    let remap = SketchUidRemap::dense_for_segment(&[&element], &HashMap::new());
    write_stock_element_with_context(buf, stock, SketchTransform::identity(), &remap);
}

fn write_stock_element_with_context(
    buf: &mut String,
    stock: &view_element::Stock,
    transform: SketchTransform,
    uid_remap: &SketchUidRemap,
) {
    let name = format_sketch_name(&stock.name);
    let (w, h, shape, bits) = match &stock.compat {
        Some(c) => (c.width as i32, c.height as i32, c.shape, c.bits),
        None => (40, 20, 3, 3),
    };
    let (x, y) = transform.point(stock.x, stock.y);
    let tail = compat_tail(stock.compat.as_ref(), "0,0,0,0,0,0");
    let uid = uid_remap.element_uid(stock.uid);
    write!(
        buf,
        "10,{},{},{},{},{},{},{},{},{}",
        uid, name, x, y, w, h, shape, bits, tail,
    )
    .unwrap();
}

/// MDL view titles are written on a single `*<title>` line.
/// Collapse CR/LF runs so untrusted titles cannot break sketch structure.
///
/// This is deliberately NOT the `sanitize_free_text` equation-section choke
/// point: a view title lives in the SKETCH section, whose structural rules
/// differ (the sketch's own records use `|` as an intra-record field separator,
/// e.g. the `$...|...|...` font line, so a `|` in a title is not the
/// equation-entry terminator). Only the line-break collapse is shared in
/// spirit; keeping this separate avoids applying equation-section escaping
/// (`|`->`/`) to sketch text where `|` is not structural.
fn sanitize_view_title_for_mdl(title: &str) -> String {
    let mut out = String::with_capacity(title.len());
    let mut prev_was_line_break = false;

    for ch in title.chars() {
        if matches!(ch, '\n' | '\r') {
            if !prev_was_line_break {
                out.push(' ');
                prev_was_line_break = true;
            }
            continue;
        }

        out.push(ch);
        prev_was_line_break = false;
    }

    out
}

#[derive(Clone, Copy)]
struct SketchTransform {
    x_offset: f64,
    y_offset: f64,
}

impl SketchTransform {
    fn identity() -> Self {
        Self {
            x_offset: 0.0,
            y_offset: 0.0,
        }
    }

    fn point(self, x: f64, y: f64) -> (i32, i32) {
        (
            (x - self.x_offset).round() as i32,
            (y - self.y_offset).round() as i32,
        )
    }
}

fn compat_tail<'a>(
    compat: Option<&'a view_element::ViewElementCompat>,
    default: &'a str,
) -> &'a str {
    compat.and_then(|c| c.tail.as_deref()).unwrap_or(default)
}

fn compat_name_field<'a>(
    compat: Option<&'a view_element::ViewElementCompat>,
    default: &'a str,
) -> &'a str {
    compat
        .and_then(|c| c.name_field.as_deref())
        .unwrap_or(default)
}

/// Distance from a valve to its flow label when the original sketch geometry
/// isn't available. Vensim's own files put the label ~19-20px from the valve;
/// 16px is close enough that a wider/wrapped label collides with the pipe.
const FLOW_LABEL_OFFSET: f64 = 20.0;

fn default_flow_label_point(flow: &view_element::Flow, transform: SketchTransform) -> (i32, i32) {
    let (x, y) = match flow.label_side {
        view_element::LabelSide::Top => (flow.x, flow.y - FLOW_LABEL_OFFSET),
        view_element::LabelSide::Left => (flow.x - FLOW_LABEL_OFFSET, flow.y),
        view_element::LabelSide::Center => (flow.x, flow.y),
        view_element::LabelSide::Bottom => (flow.x, flow.y + FLOW_LABEL_OFFSET),
        view_element::LabelSide::Right => (flow.x + FLOW_LABEL_OFFSET, flow.y),
    };
    transform.point(x, y)
}

/// Estimated bounding box (width, height in px) for a flow label, used when the
/// original sketch didn't record one.
///
/// A single-line label is sized to its whole text, erring wide (~6px/char at
/// the sketch's default font) so Vensim never word-wraps it into the flow's
/// pipe -- a fixed default too narrow for a longer name is what made Vensim
/// wrap "New fish per year" into the pipe in the fishbanks export.
///
/// A label whose display name carries a forced break -- the literal two-char
/// `\n` XMILE name attributes use, or a real newline -- is the modeler's
/// multi-line layout, so it gets the same treatment a multi-line aux does (see
/// `default_aux_size`): width = the widest line at `SKETCH_MULTILINE_PX_PER_CHAR`,
/// height = `SKETCH_LINE_HEIGHT` per line. Without this a broken flow name like
/// `Purchase of new\nships this year` collapses to one ~186px single-line label
/// that overruns the pipe; sizing it to two ~60px lines lets Vensim re-wrap the
/// (still collapsed) name back to the intended two lines. A single line's
/// height matches Vensim's own single-line flow labels.
fn default_flow_label_size(name: &str) -> (i32, i32) {
    let lines = split_display_lines(name);
    if lines.len() <= 1 {
        let width = (name.chars().count() as i32 * 6).max(15);
        return (width, SKETCH_LINE_HEIGHT);
    }
    let width = widest_multiline_width(&lines, 15);
    (width, lines.len() as i32 * SKETCH_LINE_HEIGHT)
}

/// Per-line height (px) for a multi-line sketch text element at the writer's
/// default font (`Times New Roman|12` at 96 dpi). Vensim's own files with that
/// kind of font show ~11px per wrapped line (e.g. a three-line aux box is 33px
/// tall); this matches `default_flow_label_size`'s single-line height too.
const SKETCH_LINE_HEIGHT: i32 = 11;

/// Per-character width estimate (px) for sizing a *multi-line* sketch box to
/// its widest line, at the writer's default font (`Times New Roman|12` at
/// 96 dpi). Calibrated against real Vensim files: the median single-line aux
/// box in that font is ~3.8px/char (measured over 1412 boxes in `test/`), so
/// ~4 reproduces a line's true width.
///
/// The point of matching the *true* width (rather than erring wide) is that the
/// name is written collapsed; the box is the only lever we have over Vensim's
/// word-wrap. A box sized to the widest line forces Vensim to re-wrap the
/// collapsed name back to the modeler's line count and break points. A box
/// erring wide (the historical `6`) instead fits two of the modeler's lines on
/// one row, dropping a line. (Single-line boxes still err wide -- see
/// `default_flow_label_size` -- because there the goal is the opposite: never
/// wrap.)
const SKETCH_MULTILINE_PX_PER_CHAR: i32 = 4;

/// Width (px) of the widest line of a multi-line display name, at the
/// calibrated multi-line per-char estimate. `min` is the historical floor so a
/// name with short lines never collapses to a degenerate box. Single-sources
/// the multi-line width math shared by `default_aux_size` and
/// `default_flow_label_size`.
fn widest_multiline_width(lines: &[String], min: i32) -> i32 {
    lines
        .iter()
        .map(|line| line.chars().count() as i32 * SKETCH_MULTILINE_PX_PER_CHAR)
        .max()
        .unwrap_or(min)
        .max(min)
}

/// Estimated sketch box (width, height in px) for an Aux or Alias element when
/// the original Vensim geometry isn't available (e.g. exporting from XMILE).
///
/// A name that renders on one line keeps the long-standing `40x20` default:
/// Vensim word-wraps a name too long for the box and renders it readably, and
/// `40x20` is what plenty of Vensim's own single-line auxes use. But a name
/// with an explicit break -- the literal two-character `\n` XMILE name
/// attributes use (`Maximum\nfishery size`, `Effect of fish density\non catch
/// per ship`), or a real newline -- is the modeler's chosen multi-line layout.
/// Left at `40x20`, a box crams those lines into 20px of height -- the
/// overlapping "effect of fish density on catch per ship" in the fishbanks
/// export. Sizing the box to the modeler's lines instead -- width = the widest
/// line at `SKETCH_MULTILINE_PX_PER_CHAR` (the calibrated real per-char
/// width), height = `SKETCH_LINE_HEIGHT` per line -- reproduces the intended
/// layout. Widths still floor at the historical `40`, heights at `20`, so a
/// name with short lines never collapses to a degenerate box.
fn default_aux_size(display_name: &str) -> (i32, i32) {
    let lines = split_display_lines(display_name);
    if lines.len() <= 1 {
        return (40, 20);
    }
    let width = widest_multiline_width(&lines, 40);
    let height = (lines.len() as i32 * SKETCH_LINE_HEIGHT).max(20);
    (width, height)
}

/// Write a Flow element as type 1 pipe connectors, type 11 (valve), and
/// type 10 (attached flow variable).
///
/// Vensim requires this exact ordering: pipe connectors first, then valve,
/// then flow label. The UIDs are the ones `SketchUidRemap::dense_for_segment`
/// allocated for the flow, in this order.
#[cfg(test)]
fn write_flow_element(buf: &mut String, flow: &view_element::Flow) {
    let element = ViewElement::Flow(flow.clone());
    let remap = SketchUidRemap::dense_for_segment(&[&element], &HashMap::new());
    write_flow_element_with_context(
        buf,
        flow,
        SketchTransform::identity(),
        &HashMap::new(),
        &HashSet::new(),
        &remap,
    );
}

fn write_flow_element_with_context(
    buf: &mut String,
    flow: &view_element::Flow,
    transform: SketchTransform,
    elem_positions: &HashMap<i32, (i32, i32)>,
    stock_uids: &HashSet<i32>,
    uid_remap: &SketchUidRemap,
) {
    let name = format_sketch_name(&flow.name);
    let (Some(valve_uid), Some(pipe_start)) = (
        uid_remap.valve_uid(flow.uid),
        uid_remap.pipe_start_uid(flow.uid),
    ) else {
        // The remap is built from the segment the flow is written in, so it
        // holds every flow; a flow it does not hold has no record to write.
        debug_assert!(false, "the remap holds no uids for flow {}", flow.uid);
        return;
    };
    let valve_compat = flow.compat.as_ref();
    let label_compat = flow.label_compat.as_ref();
    let (valve_x, valve_y) = transform.point(flow.x, flow.y);

    // The UIDs are allocated in file order, so this flow's pipe connectors
    // occupy a known contiguous block.
    let mut next_connector_uid = pipe_start;

    // Pipe connectors must come before the valve and flow label.
    let had_pipes = write_flow_pipe_connectors_with_context(
        buf,
        flow,
        valve_uid,
        &mut next_connector_uid,
        FlowConnectorContext {
            transform,
            elem_positions,
            stock_uids,
            uid_remap,
        },
    );

    let (valve_w, valve_h, valve_shape, valve_bits) = match valve_compat {
        Some(c) => (c.width as i32, c.height as i32, c.shape, c.bits),
        None => (6, 8, 34, 3),
    };
    let (label_w, label_h, label_shape, label_bits) = match label_compat {
        Some(c) => (c.width as i32, c.height as i32, c.shape, c.bits),
        None => {
            // Pass the raw name (break intact) so a multi-line flow label is
            // sized to its lines; the label record itself still renders the
            // collapsed name via `format_sketch_name` above.
            let (w, h) = default_flow_label_size(&flow.name);
            (w, h, 40, 3)
        }
    };
    let valve_name = compat_name_field(valve_compat, "0");
    let valve_tail = compat_tail(valve_compat, "0,0,1,0,0,0");

    if had_pipes {
        buf.push('\n');
    }
    write!(
        buf,
        "11,{},{},{},{},{},{},{},{},{}",
        valve_uid,
        valve_name,
        valve_x,
        valve_y,
        valve_w,
        valve_h,
        valve_shape,
        valve_bits,
        valve_tail,
    )
    .unwrap();

    let (label_x, label_y) = default_flow_label_point(flow, transform);
    let label_tail = compat_tail(label_compat, "0,0,-1,0,0,0");
    let label_uid = uid_remap.element_uid(flow.uid);
    write!(
        buf,
        "\n10,{},{},{},{},{},{},{},{},{}",
        label_uid, name, label_x, label_y, label_w, label_h, label_shape, label_bits, label_tail,
    )
    .unwrap();
}

struct FlowConnectorContext<'a> {
    transform: SketchTransform,
    elem_positions: &'a HashMap<i32, (i32, i32)>,
    stock_uids: &'a HashSet<i32>,
    uid_remap: &'a SketchUidRemap,
}

fn write_flow_pipe_connectors_with_context(
    buf: &mut String,
    flow: &view_element::Flow,
    valve_uid: i32,
    next_connector_uid: &mut i32,
    ctx: FlowConnectorContext<'_>,
) -> bool {
    let mut wrote_any = false;

    // A flow's pipe connector is a type-1 record with field 7, the
    // thickness, at 22 ("Larger than 20 is used for double parallel lines").
    // Field 4 is the record's `shape`, which "determines the shape of the
    // arrow (arc, polyline and so on)"; the reference enumerates no values
    // (vensim.com/documentation/24305.html), so what they mean for a pipe is
    // undocumented. Vensim's own files write 4 on the pipe to the flow's
    // downstream end (where its material goes) and 100 on the pipe to its
    // upstream end, whether a stock or a cloud is there, and so does this.
    let write_pipe = |buf: &mut String,
                      first: bool,
                      connector_uid: i32,
                      from_uid: i32,
                      to_uid: i32,
                      direction: i32,
                      x: i32,
                      y: i32| {
        if !first {
            buf.push('\n');
        }
        write!(
            buf,
            "1,{},{},{},{},0,0,22,0,0,0,-1--1--1,,1|({},{})|",
            connector_uid, from_uid, to_uid, direction, x, y,
        )
        .unwrap();
    };

    let connector_point = |point: &view_element::FlowPoint| -> (i32, i32) {
        let point_xy = ctx.transform.point(point.x, point.y);
        let Some(endpoint_uid) = point.attached_to_uid else {
            return point_xy;
        };
        if !ctx.stock_uids.contains(&endpoint_uid) {
            return point_xy;
        }

        let Some(&(stock_x, stock_y)) = ctx.elem_positions.get(&endpoint_uid) else {
            return point_xy;
        };
        let dx = f64::from(point_xy.0 - stock_x);
        let dy = f64::from(point_xy.1 - stock_y);
        let on_left_or_right = (dx.abs() - STOCK_WIDTH / 2.0).abs() <= STOCK_EDGE_TOLERANCE
            && dy.abs() <= STOCK_HEIGHT / 2.0 + STOCK_EDGE_TOLERANCE;
        if on_left_or_right {
            return (stock_x, point_xy.1);
        }

        let on_top_or_bottom = (dy.abs() - STOCK_HEIGHT / 2.0).abs() <= STOCK_EDGE_TOLERANCE
            && dx.abs() <= STOCK_WIDTH / 2.0 + STOCK_EDGE_TOLERANCE;
        if on_top_or_bottom {
            return (point_xy.0, stock_y);
        }

        point_xy
    };

    let cut = ctx.uid_remap.cut_ends(flow.uid);
    // An end the segment cannot draw runs into its cut cloud.
    let end_target =
        |point: &view_element::FlowPoint, cloud: Option<i32>, is_sink: bool| match cloud {
            Some(cloud_uid) => (
                cloud_uid,
                cut_cloud_point(flow, point, is_sink, ctx.transform),
            ),
            None => {
                let endpoint_uid = point.attached_to_uid.unwrap_or_default();
                let endpoint_uid = ctx.uid_remap.element_uid(endpoint_uid);
                (endpoint_uid, connector_point(point))
            }
        };

    if flow.points.len() > 1
        && let Some(last) = flow.points.last()
        && last.attached_to_uid.is_some()
    {
        let (endpoint_uid, (x, y)) = end_target(last, cut.sink, true);
        // The last point is the sink/downstream endpoint.
        let direction = 4;
        write_pipe(
            buf,
            !wrote_any,
            *next_connector_uid,
            valve_uid,
            endpoint_uid,
            direction,
            x,
            y,
        );
        wrote_any = true;
        *next_connector_uid += 1;
    }

    let bends = if cut.source.is_some() || cut.sink.is_some() {
        0
    } else {
        flow.points.len().saturating_sub(2)
    };
    for point in flow.points.iter().skip(1).take(bends) {
        let (x, y) = connector_point(point);
        write_pipe(
            buf,
            !wrote_any,
            *next_connector_uid,
            valve_uid,
            valve_uid,
            0,
            x,
            y,
        );
        wrote_any = true;
        *next_connector_uid += 1;
    }

    if let Some(first) = flow.points.first()
        && first.attached_to_uid.is_some()
    {
        let (endpoint_uid, (x, y)) = end_target(first, cut.source, false);
        // The first point is the source/upstream endpoint.
        let direction = 100;
        write_pipe(
            buf,
            !wrote_any,
            *next_connector_uid,
            valve_uid,
            endpoint_uid,
            direction,
            x,
            y,
        );
        wrote_any = true;
        *next_connector_uid += 1;
    }

    wrote_any
}

/// Write a type 12 line for a Cloud element.
#[cfg(test)]
fn write_cloud_element(buf: &mut String, cloud: &view_element::Cloud) {
    let remap = SketchUidRemap::dense_for_segment(&[], &HashMap::new());
    write_cloud_element_with_context(buf, cloud, SketchTransform::identity(), &remap);
}

fn write_cloud_element_with_context(
    buf: &mut String,
    cloud: &view_element::Cloud,
    transform: SketchTransform,
    uid_remap: &SketchUidRemap,
) {
    let (w, h, shape, bits) = match &cloud.compat {
        Some(c) => (c.width as i32, c.height as i32, c.shape, c.bits),
        None => (10, 8, 0, 3),
    };
    let (x, y) = transform.point(cloud.x, cloud.y);
    let name_field = compat_name_field(cloud.compat.as_ref(), "48");
    let tail = compat_tail(cloud.compat.as_ref(), "0,0,-1,0,0,0");
    let uid = uid_remap.element_uid(cloud.uid);
    write!(
        buf,
        "12,{},{},{},{},{},{},{},{},{}",
        uid, name_field, x, y, w, h, shape, bits, tail,
    )
    .unwrap();
}

/// Write a type 10 line for an Alias (ghost) element.
#[cfg(test)]
fn write_alias_element(
    buf: &mut String,
    alias: &view_element::Alias,
    name_map: &HashMap<i32, &str>,
) {
    let element = ViewElement::Alias(alias.clone());
    let remap = SketchUidRemap::dense_for_segment(&[&element], &HashMap::new());
    write_alias_element_with_context(
        buf,
        alias,
        name_map,
        &HashSet::new(),
        SketchTransform::identity(),
        &remap,
    );
}

fn write_alias_element_with_context(
    buf: &mut String,
    alias: &view_element::Alias,
    name_map: &HashMap<i32, &str>,
    stock_uids: &HashSet<i32>,
    transform: SketchTransform,
    uid_remap: &SketchUidRemap,
) {
    let raw_name = name_map.get(&alias.alias_of_uid).copied().unwrap_or("");
    let name = format_sketch_name(raw_name);
    let (w, h, shape, bits) = match &alias.compat {
        Some(c) => (c.width as i32, c.height as i32, c.shape, c.bits),
        // A ghost shows the ghosted variable's name, so it has the same
        // multi-line-name sizing concern as an aux (see `default_aux_size`).
        None => {
            let (w, h) = default_aux_size(raw_name);
            (w, h, 8, 2)
        }
    };
    let (alias_x, alias_y) = if stock_uids.contains(&alias.alias_of_uid) {
        (alias.x + 22.0, alias.y + 17.0)
    } else {
        (alias.x, alias.y)
    };
    let (x, y) = transform.point(alias_x, alias_y);
    let tail = compat_tail(
        alias.compat.as_ref(),
        "0,3,-1,0,0,0,128-128-128,0-0-0,|12||128-128-128",
    );
    let uid = uid_remap.element_uid(alias.uid);
    // shape=8
    write!(
        buf,
        "10,{},{},{},{},{},{},{},{},{}",
        uid, name, x, y, w, h, shape, bits, tail,
    )
    .unwrap();
}

/// Write a type 1 line for a Link (connector) element.
///
/// For arc connectors, we reverse-compute a control point from the stored
/// canvas angle using the endpoints of the connected elements.
#[cfg(test)]
fn write_link_element(
    buf: &mut String,
    link: &view_element::Link,
    elem_positions: &HashMap<i32, (i32, i32)>,
    use_lettered_polarity: bool,
) {
    let element = ViewElement::Link(link.clone());
    let remap = SketchUidRemap::dense_for_segment(&[&element], &HashMap::new());
    write_link_element_with_context(
        buf,
        link,
        elem_positions,
        use_lettered_polarity,
        None,
        SketchTransform::identity(),
        &remap,
    );
}

fn write_link_element_with_context(
    buf: &mut String,
    link: &view_element::Link,
    elem_positions: &HashMap<i32, (i32, i32)>,
    use_lettered_polarity: bool,
    link_compat: Option<&view_element::LinkSketchCompat>,
    transform: SketchTransform,
    uid_remap: &SketchUidRemap,
) {
    let polarity_val = match link.polarity {
        Some(LinkPolarity::Positive) if use_lettered_polarity => 83, // 'S'
        Some(LinkPolarity::Negative) if use_lettered_polarity => 79, // 'O'
        Some(LinkPolarity::Positive) => 43,                          // '+'
        Some(LinkPolarity::Negative) => 45,                          // '-'
        None => 0,
    };

    let from_uid = link.from_uid;
    let to_uid = link.to_uid;
    let from_pos = elem_positions.get(&from_uid).copied().unwrap_or((0, 0));
    let to_pos = elem_positions.get(&to_uid).copied().unwrap_or((0, 0));
    let link_uid = uid_remap.element_uid(link.uid);
    let from_uid = uid_remap.element_uid(from_uid);
    let to_uid = uid_remap.element_uid(to_uid);
    // Field 4 marks whether the connector carries a meaningful control point.
    // Vensim writes 1 on every curved influence connector and 0 on straight
    // ones (see Vensim-authored test/.../active_initial.mdl, pop.mdl,
    // water.mdl); with 0 it ignores any stored control point and re-routes the
    // connector straight, which is why XMILE-exported arcs looked straight in
    // Vensim. Recorded MDL geometry keeps its original flag verbatim.
    let field4 = match link_compat {
        Some(compat) => compat.field4,
        None => match link.shape {
            LinkShape::Straight => 0,
            LinkShape::Arc(_) | LinkShape::MultiPoint(_) => 1,
        },
    };
    let field10 = link_compat.map(|compat| compat.field10).unwrap_or(0);

    // Field 9 = 64 marks influence (causal) connectors in Vensim sketches.
    match &link.shape {
        LinkShape::Straight | LinkShape::Arc(_) => {
            let recorded = link_compat.and_then(|compat| compat.control_point);
            let (ctrl_x, ctrl_y) =
                connector_control_point(&link.shape, recorded, from_pos, to_pos, transform);
            write!(
                buf,
                "1,{},{},{},{},0,{},0,0,64,{},-1--1--1,,1|({},{})|",
                link_uid, from_uid, to_uid, field4, polarity_val, field10, ctrl_x, ctrl_y,
            )
            .unwrap();
        }
        LinkShape::MultiPoint(points) => {
            let npoints = points.len();
            write!(
                buf,
                "1,{},{},{},{},0,{},0,0,64,{},-1--1--1,,{}|",
                link_uid, from_uid, to_uid, field4, polarity_val, field10, npoints,
            )
            .unwrap();
            for pt in points {
                let (x, y) = transform.point(pt.x, pt.y);
                write!(buf, "({},{})|", x, y).unwrap();
            }
        }
    }
}

/// The control point a straight or arc connector is written with, in its
/// segment's coordinates: `from` and `to` are the endpoints it is written
/// between there, and `transform` places the segment in the view, where the
/// importer reads the point (`connector_shape`).
///
/// The point the sketch gave the connector (`recorded`, in the view's
/// coordinates) is written back while it still reads as the link's shape
/// between those endpoints, so an untouched connector keeps its line as the
/// file had it. Otherwise the point is computed from the shape: `(0, 0)` for a
/// straight link, and the arc's bisector point, rounded, for an arc.
///
/// Either way the next import records the point written here and reads the
/// link's shape from it, from the same endpoints in the same frame, so the
/// save after it finds the recorded point still reads as the shape and writes
/// it unchanged: the sketch is a fixed point of saving after one save. (A
/// rounded point can read back as a slightly different arc, or as straight
/// for an arc too flat for a whole point to bend; that reading is what the
/// next save keeps.)
fn connector_control_point(
    shape: &LinkShape,
    recorded: Option<(i32, i32)>,
    from: (i32, i32),
    to: (i32, i32),
    transform: SketchTransform,
) -> (i32, i32) {
    if let Some(point) = recorded {
        let in_view = |p: (i32, i32)| {
            (
                p.0 as f64 + transform.x_offset,
                p.1 as f64 + transform.y_offset,
            )
        };
        let written = transform.point(point.0 as f64, point.1 as f64);
        let reads_as = super::view::processing::connector_shape(in_view(from), in_view(to), point);
        // A written (0, 0) is the straight sentinel, so an arc's recorded point
        // that lands there in the segment cannot be written back.
        let is_sentinel = written == (0, 0) && matches!(shape, LinkShape::Arc(_));
        if reads_as == *shape && !is_sentinel {
            return written;
        }
    }
    match shape {
        LinkShape::Arc(canvas_angle) => compute_control_point(from, to, *canvas_angle),
        LinkShape::Straight | LinkShape::MultiPoint(_) => (0, 0),
    }
}

/// Reverse the `angle_from_points` computation: given two endpoint positions
/// and a canvas-space arc angle, compute the single control point (x, y) that
/// lies on the arc between them.
///
/// For a straight connector the caller should use (0, 0) directly rather
/// than calling this function.
fn compute_control_point(from: (i32, i32), to: (i32, i32), canvas_angle: f64) -> (i32, i32) {
    use std::f64::consts::PI;

    let (fx, fy) = (from.0 as f64, from.1 as f64);
    let (tx, ty) = (to.0 as f64, to.1 as f64);

    // Convert canvas angle to XMILE angle
    let xmile_angle = super::view::processing::canvas_angle_to_xmile(canvas_angle);
    let theta_rad = xmile_angle * PI / 180.0;

    // The tangent direction at the start point
    // XMILE angle is counter-clockwise from x-axis with y-up,
    // but canvas is y-down, so we negate y.
    let tan_x = theta_rad.cos();
    let tan_y = -theta_rad.sin();

    // Vector from start to end
    let dx = tx - fx;
    let dy = ty - fy;
    let dist = (dx * dx + dy * dy).sqrt();
    if dist < 1e-6 {
        return (((fx + tx) / 2.0) as i32, ((fy + ty) / 2.0) as i32);
    }

    // Midpoint of start-end segment
    let mx = (fx + tx) / 2.0;
    let my = (fy + ty) / 2.0;

    // Unit vector along start-end
    let ux = dx / dist;
    let uy = dy / dist;

    // The perpendicular bisector of start-end goes through (mx, my)
    // in direction (-uy, ux).
    //
    // The tangent at start forms an angle with the start-end line.
    // The cross product of (ux,uy) and (tan_x,tan_y) tells us which
    // side the arc bulges toward.
    let cross = ux * tan_y - uy * tan_x;
    let dot = ux * tan_x + uy * tan_y;

    // For nearly-straight lines, return the midpoint
    if cross.abs() < 1e-6 {
        return (mx as i32, my as i32);
    }

    // Half-angle between tangent and chord
    // tan(half_angle) = cross / (1 + dot) (half-angle formula)
    let half_angle = cross.atan2(1.0 + dot);

    // The sagitta (distance from midpoint to the arc along the perpendicular bisector):
    // sagitta = (dist/2) * tan(half_angle)
    let sagitta = (dist / 2.0) * half_angle.tan();

    // Control point along the perpendicular bisector
    let perp_x = -uy;
    let perp_y = ux;
    let cx = mx + sagitta * perp_x;
    let cy = my + sagitta * perp_y;

    (cx.round() as i32, cy.round() as i32)
}

/// Splits a StockFlow's elements into view segments at MDL view-marker
/// Group boundaries.
///
/// When the MDL parser merges multiple named views into a single StockFlow,
/// it inserts a Group element (with `is_mdl_view_marker == true`) at the
/// start of each original view's elements.  This function reverses that
/// merge by splitting on those markers.  Organizational groups from XMILE
/// (where `is_mdl_view_marker == false`) are passed through as regular
/// elements rather than triggering a view split.
///
/// Returns a Vec of (view_name, elements, font). If no marker Groups exist,
/// returns a single segment using the StockFlow's own name (or "View 1").
fn split_view_on_groups<'a>(
    sf: &'a datamodel::StockFlow,
) -> Vec<(String, Vec<&'a ViewElement>, Option<String>)> {
    let has_mdl_markers = sf.elements.iter().any(|e| {
        matches!(
            e,
            ViewElement::Group(g) if g.is_mdl_view_marker
        )
    });

    if !has_mdl_markers {
        let name = sf.name.clone().unwrap_or_else(|| "View 1".to_string());
        let elements: Vec<&ViewElement> = sf
            .elements
            .iter()
            .filter(|e| !matches!(e, ViewElement::Module(_)))
            .collect();
        return vec![(name, elements, sf.font.clone())];
    }

    let mut segments = Vec::new();
    let mut current_name = sf.name.clone().unwrap_or_else(|| "View 1".to_string());
    let mut current_elements: Vec<&'a ViewElement> = Vec::new();

    let mut seen_marker = false;
    for element in &sf.elements {
        if let ViewElement::Group(group) = element
            && group.is_mdl_view_marker
        {
            // Push the previous segment. Skip the initial pre-Group segment
            // only if it has no elements (no content before the first Group).
            if seen_marker || !current_elements.is_empty() {
                segments.push((current_name, current_elements, sf.font.clone()));
                current_elements = Vec::new();
            }
            seen_marker = true;
            current_name = group.name.clone();
            continue;
        }
        if !matches!(element, ViewElement::Module(_)) {
            current_elements.push(element);
        }
    }
    // Push the final segment (may be empty for trailing Groups).
    if seen_marker || !current_elements.is_empty() {
        segments.push((current_name, current_elements, sf.font.clone()));
    }
    segments
}

/// Stateful writer that accumulates the full MDL file text.
pub struct MdlWriter {
    buf: String,
    /// Non-fatal degradations accumulated during emission (#856). Returned
    /// alongside the text from [`MdlWriter::write_project`].
    warnings: Vec<ExportWarning>,
}

impl Default for MdlWriter {
    fn default() -> Self {
        Self::new()
    }
}

impl MdlWriter {
    pub fn new() -> Self {
        MdlWriter {
            buf: String::new(),
            warnings: Vec::new(),
        }
    }

    /// Orchestrate the full MDL file assembly and return the text plus any
    /// [`ExportWarning`]s recorded for lossy constructs.
    ///
    /// Vensim requires CRLF (`\r\n`) line endings, so the final output
    /// is converted from LF to CRLF before returning.
    pub(super) fn write_project(
        mut self,
        project: &datamodel::Project,
    ) -> Result<(String, Vec<ExportWarning>)> {
        self.buf.push_str("{UTF-8}\n");
        // Macro definitions are top-level `:MACRO:` blocks emitted right
        // after `{UTF-8}` and before the dimension defs / main-model
        // variables / `.Control` section -- the position every well-formed
        // `macro_*` fixture uses (Vensim accepts multiple back-to-back
        // blocks here). The single non-macro model is the body.
        self.write_macro_blocks(project);
        let model = super::main_model(project).expect(super::MAIN_MODEL_EXPECT);
        self.write_equations_section(model, project)?;
        self.write_sketch_section(&views_named_as_defined(model));
        self.write_settings_section(project, model);
        warn_dropped_loop_metadata(model, &mut self.warnings);
        // Collapse exact-duplicate warnings while preserving first-seen order,
        // so a construct emitted from more than one path (or an incidental
        // repeat) surfaces once rather than as stderr noise. Per-element
        // warnings name distinct elements and are intentionally not collapsed
        // here; aggregating those per variable is tracked separately.
        let mut seen: HashSet<String> = HashSet::new();
        self.warnings.retain(|w| seen.insert(w.message.clone()));
        Ok((self.buf.replace('\n', "\r\n"), self.warnings))
    }

    /// Emit each macro-marked model as a `:MACRO: ... :END OF MACRO:` block.
    ///
    /// The header is reconstructed from the `MacroSpec` (single-output
    /// `:MACRO: name(p1, p2)`, or the multi-output `:MACRO: name(p1, p2 :
    /// o1, o2)` form when `additional_outputs` is non-empty). The body is
    /// every model variable whose ident is *not* a formal parameter -- the
    /// `MacroSpec.parameters` ports are synthesized on re-import from the
    /// header (with `can_be_module_input`), so emitting them as `<param> =
    /// 0` body equations would lose that flag and make the re-imported
    /// macro treat them as ordinary body variables. Blocks are emitted in
    /// `project.models` order, which is stable across passes (the
    /// round-trip harness's zip-index model pairing relies on this).
    fn write_macro_blocks(&mut self, project: &datamodel::Project) {
        for model in &project.models {
            let Some(spec) = model.macro_spec.as_ref() else {
                continue;
            };

            // The body equations may carry original casing in the macro
            // model's own views; fall back to the underbar->space /
            // quoting helpers (the params/name are canonicalized idents).
            let display_names = build_display_name_map(&views_named_as_defined(model));
            // Context is scoped to the macro model so a body reference to a
            // parameter or a body-local variable resolves against its own
            // variable set (e.g. a wildcard subscript over a body variable).
            let ctx = WriterContext::from_model(model, &project.dimensions)
                .with_macros(project)
                .with_written_names(model, &display_names);

            let params = spec
                .parameters
                .iter()
                .map(|p| display_name_for_ident(p, &display_names))
                .collect::<Vec<_>>()
                .join(", ");
            let macro_name = display_name_for_ident(&model.name, &display_names);
            if spec.additional_outputs.is_empty() {
                writeln!(self.buf, ":MACRO: {macro_name}({params})").unwrap();
            } else {
                let outputs = spec
                    .additional_outputs
                    .iter()
                    .map(|o| display_name_for_ident(o, &display_names))
                    .collect::<Vec<_>>()
                    .join(", ");
                writeln!(self.buf, ":MACRO: {macro_name}({params} : {outputs})").unwrap();
            }

            // Body: every variable that is not a synthesized formal-
            // parameter port (those are reconstructed from the header).
            let param_idents: HashSet<&str> = spec.parameters.iter().map(|p| p.as_str()).collect();
            for var in &model.variables {
                if param_idents.contains(var.get_ident()) {
                    continue;
                }
                write_variable_entry_ctx_warn(
                    &mut self.buf,
                    var,
                    &display_names,
                    &ctx,
                    &mut self.warnings,
                );
                self.buf.push('\n');
            }

            self.buf.push_str("\n:END OF MACRO:\n");
        }
    }

    /// Write sim spec control variables (INITIAL TIME, FINAL TIME, TIME STEP, SAVEPER).
    fn write_sim_specs(&mut self, sim_specs: &datamodel::SimSpecs) {
        // The time unit is a units field like a variable's, so it goes through
        // the same choke point. A model that names no time unit is written
        // with the name the engine checks it under
        // (`units_check::model_time_units`): the reader gives an empty field
        // its own default and reads a bare range as dimensionless, and either
        // would be a different unit on the way back.
        let units = sim_specs
            .time_units
            .as_deref()
            .map(|units| sanitize_free_text(units, FreeTextLineMode::SingleLine, &['~']))
            .filter(|units| !units.trim().is_empty())
            .unwrap_or_else(|| "time".to_owned());
        let units = units.trim();

        // INITIAL TIME
        write!(
            self.buf,
            "\nINITIAL TIME  = \n\t{}\n\t~\t{}\n\t~\tThe initial time for the simulation.\n\t|\n",
            format_f64(sim_specs.start),
            units,
        )
        .unwrap();

        // FINAL TIME
        write!(
            self.buf,
            "\nFINAL TIME  = \n\t{}\n\t~\t{}\n\t~\tThe final time for the simulation.\n\t|\n",
            format_f64(sim_specs.stop),
            units,
        )
        .unwrap();

        // TIME STEP
        let dt_value = match &sim_specs.dt {
            datamodel::Dt::Dt(v) => format_f64(*v),
            datamodel::Dt::Reciprocal(v) => format!("1/{}", format_f64(*v)),
        };
        let units_with_range = format!("{units} [0,?]");
        write!(
            self.buf,
            "\nTIME STEP  = \n\t{}\n\t~\t{}\n\t~\tThe time step for the simulation.\n\t|\n",
            dt_value, units_with_range,
        )
        .unwrap();

        // SAVEPER. No save step means the save step follows the time step,
        // which is `SAVEPER = TIME STEP`, the form the importer reads back as
        // no save step. A save step is written as its number even when it
        // equals the time step's, so a file keeps its meaning when TIME STEP
        // is later changed.
        let saveper_value = match &sim_specs.save_step {
            Some(datamodel::Dt::Dt(v)) => format_f64(*v),
            Some(datamodel::Dt::Reciprocal(v)) => format!("1/{}", format_f64(*v)),
            None => "TIME STEP".to_owned(),
        };
        write!(
            self.buf,
            "\nSAVEPER  = \n\t{}\n\t~\t{}\n\t~\tThe frequency with which output is stored.\n\t|\n",
            saveper_value, units_with_range,
        )
        .unwrap();
    }

    /// Write the full equations section: dimensions, grouped variables, sim specs, terminator.
    fn write_equations_section(
        &mut self,
        model: &datamodel::Model,
        project: &datamodel::Project,
    ) -> Result<()> {
        // 1. Dimension definitions
        for dim in &project.dimensions {
            write_dimension_def_warn(&mut self.buf, dim, &project.dimensions, &mut self.warnings);
        }

        let display_names = build_display_name_map(&views_named_as_defined(model));
        // Model-scoped writer context: the variable set (for builtin/variable
        // shadowing) and per-variable declared dimensions (for wildcard
        // subscript recovery), threaded into every equation's printer.
        let ctx = WriterContext::from_model(model, &project.dimensions)
            .with_macros(project)
            .with_written_names(model, &display_names);

        // Reconstruct each Phase-4-materialized multi-output cluster
        // (`<lhs> = <macro>(<args> : <bindings>)`) and collect the idents
        // that must be suppressed from the normal per-variable emission
        // (the Variable::Module plus its binding auxes). The reconstructed
        // entries are emitted with the ungrouped variables (sorted by LHS
        // ident) so the output ordering stays deterministic across passes.
        let reconstruction = build_multi_output_reconstructions(model, project, &display_names);

        // A macro-backed `Variable::Module` whose materialized cluster is
        // incomplete cannot be reconstructed into `:` call syntax. Emitting
        // the rest of the model anyway would silently drop the invocation
        // and leave the surviving binding auxes referencing a module that
        // is never written -- a corrupt `.mdl`. `project_to_mdl`'s gate
        // already hard-rejects ordinary `Variable::Module`s for the same
        // reason; admitting a macro-backed one is conditional on its
        // cluster being well-formed, which is enforced here (the gate's
        // is-macro check is only a coarse pre-filter -- it cannot see
        // whether a binding aux or argument wiring was later edited away).
        if !reconstruction.failed.is_empty() {
            return Err(Error::new(
                ErrorKind::Import,
                ErrorCode::Generic,
                Some(format!(
                    "MDL export cannot faithfully reconstruct {} multi-output \
                     macro invocation(s): {}",
                    reconstruction.failed.len(),
                    reconstruction.failed.join("; ")
                )),
            ));
        }

        // Where a variable is written says which group it is in: the reader
        // puts each variable in the group whose marker last came before it.
        // So the variables no group holds come first, before any marker, and
        // each group's follow its own, in the model's group order (which is
        // also what the reader derives a group's parent from). The control
        // variables are the sim specs: they are written in the model's
        // Control group when it has one, and with the ungrouped variables
        // otherwise, so a save adds no group the model does not have.
        let control_group = model
            .groups
            .iter()
            .position(|group| group.name.eq_ignore_ascii_case("Control"));
        let grouped_idents: HashSet<&str> = model
            .groups
            .iter()
            .flat_map(|group| group.members.iter().map(String::as_str))
            .collect();
        let sim_specs = model.sim_specs.as_ref().unwrap_or(&project.sim_specs);

        // 2. Ungrouped variables (alphabetical by ident for deterministic
        // output), with the reconstructed multi-output invocations
        // interleaved by LHS ident so the whole section stays sorted across
        // passes (the round-trip harness's zip-index model pairing relies on
        // stable ordering).
        let mut ungrouped: Vec<UngroupedEntry<'_>> = model
            .variables
            .iter()
            .filter(|v| {
                !grouped_idents.contains(v.get_ident())
                    && !reconstruction.suppressed.contains(v.get_ident())
            })
            .map(UngroupedEntry::Variable)
            .collect();
        // A reconstructed call is written where its left-hand side is: in
        // its group, or here.
        let reconstructed: HashMap<&str, &str> = reconstruction
            .entries
            .iter()
            .map(|(lhs, text)| (lhs.as_str(), text.as_str()))
            .collect();
        for (lhs_ident, text) in &reconstruction.entries {
            if !grouped_idents.contains(lhs_ident.as_str()) {
                ungrouped.push(UngroupedEntry::Reconstructed(lhs_ident, text));
            }
        }
        ungrouped.sort_by(|a, b| a.sort_key().cmp(b.sort_key()));

        for entry in ungrouped {
            match entry {
                UngroupedEntry::Variable(var) => {
                    write_variable_entry_ctx_warn(
                        &mut self.buf,
                        var,
                        &display_names,
                        &ctx,
                        &mut self.warnings,
                    );
                }
                UngroupedEntry::Reconstructed(_, text) => {
                    self.buf.push_str(text);
                }
            }
            self.buf.push('\n');
        }
        if control_group.is_none() {
            self.write_sim_specs(sim_specs);
        }

        // 3. Each group's marker and members.
        for (at, group) in model.groups.iter().enumerate() {
            let holds_sim_specs = control_group == Some(at);
            // Both free-text fields route through the sanitization choke
            // point (GH #849): the name is a single banner line
            // (`try_group_star` stops it at whitespace), and the doc is
            // skipped up to `|` -- so a raw `|` in either, an embedded section
            // terminator, or a line break in the name would corrupt the file.
            // The group holding the sim specs is the model's like any other:
            // its name as the model spells it, and its documentation.
            warn_group_lossiness(group, &mut self.warnings);
            let (name, doc) = (
                sanitize_free_text(
                    &underbar_to_space(&group.name),
                    FreeTextLineMode::SingleLine,
                    &[],
                ),
                sanitize_free_text(
                    group.doc.as_deref().unwrap_or(""),
                    FreeTextLineMode::Multiline,
                    &[],
                ),
            );
            write!(
                self.buf,
                "\n********************************************************\n\t.{name}\n********************************************************~\n\t\t{doc}\n\t|\n",
            )
            .unwrap();

            for member_ident in &group.members {
                if let Some(text) = reconstructed.get(member_ident.as_str()) {
                    self.buf.push_str(text);
                    self.buf.push('\n');
                    continue;
                }
                if reconstruction.suppressed.contains(member_ident.as_str()) {
                    continue;
                }
                if let Some(var) = model
                    .variables
                    .iter()
                    .find(|v| v.get_ident() == member_ident)
                {
                    write_variable_entry_ctx_warn(
                        &mut self.buf,
                        var,
                        &display_names,
                        &ctx,
                        &mut self.warnings,
                    );
                    self.buf.push('\n');
                }
            }
            if holds_sim_specs {
                self.write_sim_specs(sim_specs);
            }
        }

        // 4. Section terminator
        self.buf
            .push_str("\\\\\\---/// Sketch information - do not modify anything except names\n");

        Ok(())
    }

    /// Write the sketch/view section of the MDL file.
    ///
    /// Each view gets its own `\\\---///` separator and `V300` header line.
    /// The first view's separator is already emitted by `write_equations_section`.
    /// The final `///---\\\` terminator follows the last view.
    ///
    /// When a StockFlow contains Group elements (from merging multiple MDL
    /// views at parse time), we split on those boundaries to reconstruct
    /// the original multi-view structure.
    fn write_sketch_section(&mut self, views: &[View]) {
        if views.is_empty() {
            // Emit a minimal valid sketch so the output is not malformed.
            self.buf
                .push_str("V300  Do not put anything below this section - it will be ignored\n");
            self.buf.push_str("*View 1\n");
            self.buf
                .push_str("$192-192-192,0,Times New Roman|12||0-0-0|0-0-0|0-0-255|-1--1--1|-1--1--1|96,96,100,0\n");
            self.buf.push_str("///---\\\\\\\n");
            return;
        }

        let mut segment_idx = 0;
        for view in views {
            let View::StockFlow(sf) = view;

            // Build shared maps from ALL elements so that cross-view
            // references (links, aliases) resolve correctly.
            let all_elements = sf.elements.to_vec();
            let name_map = build_name_map(&all_elements);
            let mut link_compat_by_uid: HashMap<i32, &view_element::LinkSketchCompat> =
                HashMap::new();
            let mut stock_uids: HashSet<i32> = HashSet::new();
            if let Some(sketch_compat) = sf.sketch_compat.as_ref() {
                for link in &sketch_compat.links {
                    link_compat_by_uid.insert(link.uid, link);
                }
            }
            for elem in &sf.elements {
                if let ViewElement::Stock(stock) = elem {
                    stock_uids.insert(stock.uid);
                }
            }

            let segments = split_view_on_groups(sf);
            let mut elem_positions = HashMap::new();
            for (segment_ix, (_, elements, _)) in segments.iter().enumerate() {
                let transform = sf
                    .sketch_compat
                    .as_ref()
                    .and_then(|compat| compat.segments.get(segment_ix))
                    .map(|compat| SketchTransform {
                        x_offset: compat.x_offset,
                        y_offset: compat.y_offset,
                    })
                    .unwrap_or_else(SketchTransform::identity);
                elem_positions.extend(build_element_positions_with_transform(
                    elements,
                    transform,
                    &stock_uids,
                ));
            }

            for (segment_ix, (view_name, elements, font)) in segments.iter().enumerate() {
                if segment_idx > 0 {
                    self.buf.push_str(
                        "\\\\\\---/// Sketch information - do not modify anything except names\n",
                    );
                }
                self.buf.push_str(
                    "V300  Do not put anything below this section - it will be ignored\n",
                );
                self.write_view_segment(
                    view_name,
                    &all_elements,
                    elements,
                    font.as_deref(),
                    sf.use_lettered_polarity,
                    sf.sketch_compat
                        .as_ref()
                        .and_then(|compat| compat.segments.get(segment_ix))
                        .map(|compat| SketchTransform {
                            x_offset: compat.x_offset,
                            y_offset: compat.y_offset,
                        })
                        .unwrap_or_else(SketchTransform::identity),
                    &elem_positions,
                    &name_map,
                    &stock_uids,
                    &link_compat_by_uid,
                );
                segment_idx += 1;
            }
        }

        self.buf.push_str("///---\\\\\\\n");
    }

    /// Write a single view segment: title, font line, and all sketch elements.
    #[allow(clippy::too_many_arguments)]
    fn write_view_segment(
        &mut self,
        view_name: &str,
        view_elements: &[ViewElement],
        elements: &[&ViewElement],
        font: Option<&str>,
        use_lettered_polarity: bool,
        transform: SketchTransform,
        elem_positions: &HashMap<i32, (i32, i32)>,
        name_map: &HashMap<i32, &str>,
        stock_uids: &HashSet<i32>,
        link_compat_by_uid: &HashMap<i32, &view_element::LinkSketchCompat>,
    ) {
        // Collect cloud UIDs so flow pipe connectors can set the right
        // direction flag, and build a map from flow_uid -> clouds so each
        // cloud is emitted just before its flow's pipe connectors (Vensim
        // requires this ordering). Built before UID allocation so the remap
        // and the emit loop below agree on the per-flow cloud order.
        //
        // The clouds come from the whole view, not the segment: a cloud the
        // importer placed after the views merged (`routes`) sits at the end of
        // the element list, in the last segment, whichever segment its flow is
        // drawn in, and it is written with its flow.
        let mut flow_clouds: HashMap<i32, Vec<&view_element::Cloud>> = HashMap::new();
        for elem in view_elements {
            if let ViewElement::Cloud(c) = elem {
                flow_clouds.entry(c.flow_uid).or_default().push(c);
            }
        }

        let uid_remap = SketchUidRemap::dense_for_segment(elements, &flow_clouds);
        let view_title = sanitize_view_title_for_mdl(view_name);
        writeln!(self.buf, "*{}", view_title).unwrap();

        if let Some(f) = font {
            writeln!(self.buf, "${}", f).unwrap();
        } else {
            self.buf.push_str(
                "$192-192-192,0,Times New Roman|12||0-0-0|0-0-0|0-0-255|-1--1--1|-1--1--1|96,96,100,0\n",
            );
        }

        for elem in elements {
            match elem {
                ViewElement::Aux(aux) => {
                    write_aux_element_with_context(&mut self.buf, aux, transform, &uid_remap);
                    self.buf.push('\n');
                }
                ViewElement::Stock(stock) => {
                    write_stock_element_with_context(&mut self.buf, stock, transform, &uid_remap);
                    self.buf.push('\n');
                }
                ViewElement::Flow(flow) => {
                    // Emit associated clouds before the flow pipes
                    if let Some(clouds) = flow_clouds.get(&flow.uid) {
                        for cloud in clouds {
                            write_cloud_element_with_context(
                                &mut self.buf,
                                cloud,
                                transform,
                                &uid_remap,
                            );
                            self.buf.push('\n');
                        }
                    }
                    let cut = uid_remap.cut_ends(flow.uid);
                    let cut_clouds = [
                        (cut.source, flow.points.first(), false),
                        (cut.sink, flow.points.last(), true),
                    ];
                    for (uid, end, is_sink) in cut_clouds {
                        if let (Some(uid), Some(end)) = (uid, end) {
                            let (x, y) = cut_cloud_point(flow, end, is_sink, transform);
                            writeln!(self.buf, "12,{uid},48,{x},{y},10,8,0,3,0,0,-1,0,0,0")
                                .unwrap();
                        }
                    }
                    write_flow_element_with_context(
                        &mut self.buf,
                        flow,
                        transform,
                        elem_positions,
                        stock_uids,
                        &uid_remap,
                    );
                    self.buf.push('\n');
                }
                ViewElement::Link(link) => {
                    write_link_element_with_context(
                        &mut self.buf,
                        link,
                        elem_positions,
                        use_lettered_polarity,
                        link_compat_by_uid.get(&link.uid).copied(),
                        transform,
                        &uid_remap,
                    );
                    self.buf.push('\n');
                }
                // Clouds are emitted with their associated flow above
                ViewElement::Cloud(_) => {}
                ViewElement::Alias(alias) => {
                    write_alias_element_with_context(
                        &mut self.buf,
                        alias,
                        name_map,
                        stock_uids,
                        transform,
                        &uid_remap,
                    );
                    self.buf.push('\n');
                }
                ViewElement::Module(_) | ViewElement::Group(_) => {}
            }
        }
    }

    /// Write the settings section of the MDL file.
    ///
    /// The settings section follows the sketch terminator (`///---\\\`) and
    /// starts with the `:L<%^E!@` marker. It contains type-coded setting
    /// lines that Vensim reads to restore UI and simulation state.
    ///
    /// The specs are the main model's, as the control variables are
    /// (`write_equations_section`): the integration method and the display
    /// range follow the run the file defines.
    fn write_settings_section(&mut self, project: &datamodel::Project, model: &datamodel::Model) {
        let sim_specs = model.sim_specs.as_ref().unwrap_or(&project.sim_specs);

        // The ///---\\\ separator is already emitted by write_sketch_section.
        // The 0x7F (DEL) between :L and <%^E!@ is required by Vensim's parser.
        self.buf.push_str(":L\x7F<%^E!@\n");

        // Type 22: Unit equivalences. The name/equation/aliases are one
        // physical `22:` line whose fields split on `,` (see
        // `settings::parse_unit_equivalence`), so each free-text token routes
        // through the sanitization choke point with `,` as its field separator
        // (GH #849) -- a raw comma would otherwise fracture a name into extra
        // aliases and a line break would truncate the line.
        let sanitize_unit_field =
            |s: &str| sanitize_free_text(s, FreeTextLineMode::SingleLine, &[',']);
        for unit in &project.units {
            if unit.disabled {
                continue;
            }
            self.buf.push_str("22:");
            if let Some(eq) = &unit.equation {
                write!(self.buf, "{},", sanitize_unit_field(eq)).unwrap();
            }
            self.buf.push_str(&sanitize_unit_field(&unit.name));
            for alias in &unit.aliases {
                write!(self.buf, ",{}", sanitize_unit_field(alias)).unwrap();
            }
            self.buf.push('\n');
        }

        // Type 15: Integration method
        let method_code = match sim_specs.sim_method {
            datamodel::SimMethod::Euler => 0,
            datamodel::SimMethod::RungeKutta4 => 1,
            datamodel::SimMethod::RungeKutta2 => 3,
        };
        writeln!(self.buf, "15:0,0,0,{},0,0", method_code).unwrap();

        // Type 19: Display settings (Vensim default)
        self.buf.push_str("19:100,0\n");
        // Type 27: Font size (Vensim default)
        self.buf.push_str("27:0,\n");
        // Type 34: Optimization settings (Vensim default)
        self.buf.push_str("34:0,\n");
        // Type 4: Time variable name
        self.buf.push_str("4:Time\n");
        // Type 35: Date format name
        self.buf.push_str("35:Date\n");
        // Type 36: Date format pattern
        self.buf.push_str("36:YYYY-MM-DD\n");
        // Type 37-39: Calendar date origin (2000-01-01)
        self.buf.push_str("37:2000\n");
        self.buf.push_str("38:1\n");
        self.buf.push_str("39:1\n");
        // Type 40: Calendar type
        self.buf.push_str("40:2\n");
        // Type 41-42: Calendar sub-settings
        self.buf.push_str("41:0\n");
        self.buf.push_str("42:0\n");

        // Types 24/25/26: Display time range for the graph/chart output.
        // These control what Vensim shows in its default output graphs,
        // NOT the simulation time range (which comes from the TIME STEP,
        // INITIAL TIME, and FINAL TIME variable definitions).
        // All reference MDL files set 24=start, 25=stop, 26=stop.
        writeln!(self.buf, "24:{}", format_f64(sim_specs.start)).unwrap();
        writeln!(self.buf, "25:{}", format_f64(sim_specs.stop)).unwrap();
        writeln!(self.buf, "26:{}", format_f64(sim_specs.stop)).unwrap();
    }
}

fn build_element_positions_with_transform(
    elements: &[&ViewElement],
    transform: SketchTransform,
    stock_uids: &HashSet<i32>,
) -> HashMap<i32, (i32, i32)> {
    let mut positions = HashMap::new();
    for elem in elements {
        let (uid, x, y) = match elem {
            ViewElement::Aux(a) => {
                let (x, y) = transform.point(a.x, a.y);
                (a.uid, x, y)
            }
            ViewElement::Stock(s) => {
                let (x, y) = transform.point(s.x, s.y);
                (s.uid, x, y)
            }
            ViewElement::Flow(f) => {
                let (label_x, label_y) = default_flow_label_point(f, transform);
                (f.uid, label_x, label_y)
            }
            ViewElement::Cloud(c) => {
                let (x, y) = transform.point(c.x, c.y);
                (c.uid, x, y)
            }
            ViewElement::Alias(a) => {
                let (x, y) = if stock_uids.contains(&a.alias_of_uid) {
                    transform.point(a.x + 22.0, a.y + 17.0)
                } else {
                    transform.point(a.x, a.y)
                };
                (a.uid, x, y)
            }
            ViewElement::Module(m) => {
                let (x, y) = transform.point(m.x, m.y);
                (m.uid, x, y)
            }
            ViewElement::Link(_) | ViewElement::Group(_) => continue,
        };
        positions.insert(uid, (x, y));
    }
    positions
}

/// Build a map from element UID to name for alias (ghost) resolution.
fn build_name_map(elements: &[ViewElement]) -> HashMap<i32, &str> {
    let mut names = HashMap::new();
    for elem in elements {
        match elem {
            ViewElement::Aux(a) => {
                names.insert(a.uid, a.name.as_str());
            }
            ViewElement::Stock(s) => {
                names.insert(s.uid, s.name.as_str());
            }
            ViewElement::Flow(f) => {
                names.insert(f.uid, f.name.as_str());
            }
            _ => {}
        }
    }
    names
}

#[cfg(test)]
#[path = "writer_tests.rs"]
mod tests;

// Split out of writer_tests.rs to stay under the per-file line cap (GH #645);
// reuses the `tests` module's `make_*` fixture helpers.
#[cfg(test)]
#[path = "writer_lossiness_tests.rs"]
mod lossiness_tests;

// Split out of writer_tests.rs for the same per-file line cap; the sketch
// element/connector/section serialization block.
#[cfg(test)]
#[path = "writer_sketch_tests.rs"]
mod sketch_tests;

// Property-based tests (own file per the per-file line cap; see the module's
// header for the generator design and fixpoint conventions).
#[cfg(test)]
#[path = "writer_proptest.rs"]
mod proptest_tests;

// The rules that make a save a fixed point (own file per the per-file line
// cap).
#[cfg(test)]
#[path = "writer_fixpoint_tests.rs"]
mod fixpoint_tests;

// An `Equation::Arrayed`'s equations.
#[path = "writer_arrayed.rs"]
mod arrayed;

// What an XMILE model's save as MDL keeps.
#[cfg(test)]
#[path = "writer_xmile_tests.rs"]
mod xmile_tests;
