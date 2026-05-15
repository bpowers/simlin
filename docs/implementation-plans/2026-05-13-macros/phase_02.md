# Vensim Macro Support — Phase 2: MDL parsing and import

**Goal:** Parse the full `:MACRO:` syntax — including the `:` multi-output separator no parser handles today — and import every macro definition from a `.mdl` file as a macro-marked `datamodel::Model`.

**Architecture:** Two halves. (a) **Parser**: the MDL lexer/reader/parser already recognize `:MACRO:` ... `:END OF MACRO:` and build a `MacroDef` AST node, but they have no notion of the `:` multi-output separator. Phase 2 adds an output list to `MacroDef`, an output-binding list to `Expr::App`, and the parser logic to populate both. (b) **Convert**: `MacroDef`s are currently parsed and then silently discarded by `collect_symbols` in `convert/mod.rs`. Phase 2 makes `mdl/convert` turn each `MacroDef` into a macro-marked `datamodel::Model` (carrying the `MacroSpec` from Phase 1) by reusing the existing `ConversionContext` conversion pipeline scoped to the macro body, and translates `$`-suffixed time references in macro bodies to canonical engine time idents.

**Tech Stack:** Rust — the native MDL parser (`src/simlin-engine/src/mdl/`). No external dependencies. The Vensim `:MACRO:` syntax reference is in-repo at `docs/reference/vensim-macros.md` and `docs/reference/xmile-v1.0.html`.

**Scope:** 7 phases from the original design (`docs/design-plans/2026-05-13-macros.md`); this is phase 2 of 7.

**Codebase verified:** 2026-05-14

---

## Acceptance Criteria Coverage

This phase implements and tests:

### macros.AC1: Macro definitions parse and represent faithfully
- **macros.AC1.1 Success:** A `.mdl` `:MACRO:` block imports as a macro-marked `Model` whose `MacroSpec.parameters` matches the header's input list in order, with body variables matching the body equations.
- **macros.AC1.2 Success:** A `:MACRO:` header with a `:` output list (`add3(a,b,c : minval, maxval)`) imports with `MacroSpec.additional_outputs` populated in order.
- **macros.AC1.5 Success:** `$`-suffixed time references in a macro body (`TIME STEP$`, `Time$`) import as the canonical time identifiers.
- **macros.AC1.6 Failure:** A `:MACRO:` block with no `:END OF MACRO:` reports a clear parse error.
- **macros.AC1.7 Edge:** A macro defined but never invoked (C-LEARN's `INIT`) imports as a valid macro-marked model and is preserved.

(`macros.AC1.3` is XMILE import — Phase 5. `macros.AC1.4` is round-trip — completed in Phase 1.)

---

## Current state (verified 2026-05-14)

**Already works** — the lexer, reader, and parser fully handle the *single-output* `:MACRO:` form:
- `src/simlin-engine/src/mdl/lexer.rs` — `RawToken::Macro` (`:MACRO:`), `RawToken::EndOfMacro` (`:END OF MACRO:`), and `RawToken::Colon` (bare `:`) are all tokenized (`colon_keyword()`, lexer.rs:518-570). `$` is a symbol char (`is_symbol_char`, lexer.rs:343-351) and `symbol()` (lexer.rs:355-399) does **not** strip a trailing `$`, so `Time$` lexes as one `Symbol("Time$")` and `TIME STEP$` as `Symbol("TIME STEP$")`.
- `src/simlin-engine/src/mdl/parser.rs` — the macro header is parsed in `parse_full_eq_with_units` (parser.rs:518-545), producing `SectionEnd::MacroStart(name, args, loc)`; `:END OF MACRO:` → `SectionEnd::MacroEnd(loc)`. Argument lists go through `parse_expr_list` (parser.rs:1361-1399), which separates only on `Comma` and `Semicolon` — **there is no `Colon` arm anywhere in expression parsing.** `Token::Colon` is meaningful only as the subscript-definition operator immediately after an LHS symbol.
- `src/simlin-engine/src/mdl/reader.rs` — `MacroState` (reader.rs:60-70) accumulates body equations; `handle_parse_result` (reader.rs:273-332) builds `MacroDef` and emits `MdlItem::Macro(Box<MacroDef>)`. **An unterminated `:MACRO:` block already produces `ReaderError::EofInsideMacro`** ("unexpected end of file inside macro"); a stray `:END OF MACRO:` produces `ReaderError::UnmatchedMacroEnd`. Nested `:MACRO:` blocks are rejected.
- `src/simlin-engine/src/mdl/ast.rs` — `MacroDef` (ast.rs:483-493): `{ name: Cow<str>, args: Vec<Expr>, equations: Vec<FullEquation>, loc: Loc }`. `Expr::App(Cow<str>, Vec<Subscript>, Vec<Expr>, CallKind, Loc)` (ast.rs:128-134). `CallKind` is `Builtin | Symbol` (ast.rs:98-108).

**Does not work / missing:**
- No `:` multi-output separator support — not in `MacroDef` (no input/output split on `args`), not in `Expr::App`, not in the parser.
- Macros are **discarded**: `collect_symbols` in `convert/mod.rs:248` has `MdlItem::Macro(_) | MdlItem::EqEnd(_) => {}` — every parsed `MacroDef` is dropped and never reaches the datamodel. (Note: this line declines to register macros as *main-model symbols*, which is correct; the `MdlItem::Macro` entries remain in `self.items`. Phase 2 adds a new conversion step that consumes them — it does not need to change line 248.)
- No `$`-time translation anywhere — `Time$` survives as an `Expr::Var` named `"Time$"` that nothing maps to a canonical ident.

**Key types and entry points:**
- `ConversionContext<'input>` (`convert/mod.rs:45-88`) — holds `items: Vec<MdlItem>`, `symbols: HashMap<String, SymbolInfo>`, `dimensions`, `sim_specs`, `formatter: XmileFormatter`, `data_provider`. Built by `new_with_data` (`convert/mod.rs:98-141`, drains an `EquationReader` into `self.items`).
- `convert()` (`convert/mod.rs:144-197`) runs these steps in order: `collect_symbols`, `build_dimensions`, set-subrange-names, `mark_variable_types`, `scan_for_extrapolate_lookups`, `link_stocks_and_flows`, `build_project` — six numbered passes (the in-code comments label them Pass 1 through Pass 6) plus the set-subrange-names half-pass (Pass 2.5).
- `build_project` (`convert/variables.rs:28-106`) iterates `self.symbols`, builds variables via `build_variable` / `build_variable_with_elements` / `build_equation`, and constructs exactly one `Model { name: "main", ... }` wrapped in a `Project`. **After Phase 1, this construction site already includes `macro_spec: None`.**
- `build_variable` (`convert/variables.rs:798-854`) — `SymbolInfo` + `FullEquation` → `datamodel::Variable` (Stock/Flow/Aux). `build_equation` (`convert/variables.rs:859-939`) — `MdlEquation` → `(datamodel::Equation, Compat, Option<GraphicalFunction>)`, RHS formatted via `self.formatter.format_expr(expr)`.
- `ConvertError` (`convert/types.rs:12-26`): `Reader | View | InvalidRange | CyclicDimensionDefinition | Import | Other`. New macro-conversion errors use `ConvertError::Other(String)` or a new variant.
- Test-only entry point `convert_mdl(source: &str) -> Result<Project, ConvertError>` (`convert/mod.rs:315-318`); production `open_vensim(&str) -> common::Result<Project>` (`compat.rs:62-74`).
- Canonical engine time idents (`builtins.rs:146-149`, `354-367`): `time`, `time_step` (alias `dt`), `initial_time` (alias `starttime`), `final_time` (alias `stoptime`).
- Parser test idiom (`parser.rs:2343-2359`, `test_parse_macro_start`): build a `TokenNormalizer` from `&str`, `collect_tokens`, call public `parse(&tokens)`, assert on the `(Equation, Option<Units>, SectionEnd)` tuple. Reader test idiom (`reader.rs:1304-1320`, `test_macro_followed_by_equation`): `EquationReader::new(input).next_item()`.

**The 6 fixtures** (`test/test-models/tests/macro_*/`) — all single-output, all `.mdl` + `output.tab`; none uses the `:` separator or `$`-time, so those two features need synthetic test inputs:
- `macro_expression` — `:MACRO: EXPRESSION MACRO(input, parameter)` / body `EXPRESSION MACRO = input * parameter` (one aux). Invoked `macro output = EXPRESSION MACRO(macro input,macro parameter)`.
- `macro_multi_expression` — same header; 2-equation body (`EXPRESSION MACRO = input * intermediate` + helper `intermediate = parameter * 3`).
- `macro_stock` — body is a stock: `EXPRESSION MACRO = INTEG(input, parameter)`.
- `macro_cross_reference` — two macros; `EXPRESSION MACRO` body calls `SECOND MACRO(input,parameter)` (a macro call inside a macro body — stays as equation text in Phase 2).
- `macro_multi_macros` — two independent macros.
- `macro_trailing_definition` — macro defined *after* its call site.

---

<!-- START_SUBCOMPONENT_A (tasks 1-3) -->
## Subcomponent A: Parser support for the `:` multi-output syntax

<!-- START_TASK_1 -->
### Task 1: AST fields for macro outputs and call-site output bindings

**Verifies:** None (AST structural change; the `:`-form ACs are verified in Tasks 5-7).

**Files:**
- Modify: `src/simlin-engine/src/mdl/ast.rs` (`MacroDef` ~lines 483-493; `Expr::App` ~lines 128-134; the `Expr` `loc()` / walk helpers that `match` on `App`)
- Modify: every other site in `src/simlin-engine/` that constructs or non-wildcard-matches `Expr::App` — discovered by the compiler. Known sites: `parser.rs` (3 `App` construction sites at `parser.rs:1283/1306/1348`, plus non-wildcard `match` arms in its `#[cfg(test)]` module), `reader.rs` (4 non-wildcard `Expr::App` match arms at `reader.rs:747/768/787/810`), `xmile_compat.rs`, `convert/helpers.rs:66` (`is_top_level_integ`), `convert/stocks.rs`, `convert/variables.rs`, `writer.rs`. Known `MacroDef` construction sites: `reader.rs` `handle_parse_result`, the inline tests `ast.rs:1026` (`test_macro_def`) and `reader.rs:1304` (`test_macro_followed_by_equation`).

**Implementation:**

Step 1 — Add an `outputs` field to `MacroDef`, after `args`:

```rust
pub struct MacroDef<'input> {
    pub name: Cow<'input, str>,
    /// Formal input parameters (the comma-separated list before any `:`),
    /// parsed as expressions (typically just `Var` nodes).
    pub args: Vec<Expr<'input>>,
    /// Additional named outputs from the `:`-list in the header
    /// (`:MACRO: add3(a,b,c : minval, maxval)`). Empty for ordinary
    /// single-output macros.
    pub outputs: Vec<Expr<'input>>,
    /// Equations within the macro body.
    pub equations: Vec<FullEquation<'input>>,
    pub loc: Loc,
}
```

Step 2 — Add a 6th field to `Expr::App` for call-site output bindings. Update its doc comment:

```rust
    /// Function/macro application: name, subscripts, args, kind, output bindings.
    ///
    /// `output_bindings` holds the post-`:` output-binding expressions of a
    /// Vensim multi-output macro call (`add3(a, b, c : minv, maxv)`); it is
    /// empty for every ordinary call. The bindings are parsed as expressions
    /// (expected to be `Var` nodes naming caller variables).
    App(
        Cow<'input, str>,
        Vec<Subscript<'input>>,
        Vec<Expr<'input>>,
        CallKind,
        Vec<Expr<'input>>,
        Loc,
    ),
```

Step 3 — Make `src/simlin-engine` compile again. Run `cargo build -p simlin-engine --all-targets`. For every error:
- `Expr::App` construction sites: add `vec![]` for the new `output_bindings` field (the parser will populate it in Task 3; everything else leaves it empty).
- `Expr::App` non-wildcard `match` arms: add a binding (or `_`) for the new field. In AST traversal helpers (`loc()`, `walk`), the new `Vec<Expr>` should be walked like `args` so later passes see nested expressions — but for Task 1, matching it as `_` is acceptable where the helper does not recurse into `args` either; match the surrounding code's behavior.
- The MDL-writer formatter `format_call_ctx` (`src/simlin-engine/src/mdl/xmile_compat.rs`, the `Expr::App` arm ~lines 219-498): a multi-output `App` cannot be represented as plain equation text. Add `debug_assert!(output_bindings.is_empty(), "multi-output macro invocations must be materialized by the converter before formatting -- see Phase 4")` at the top of that arm. Phase 4 materializes multi-output invocations in the converter *before* anything formats them, so this assertion never trips at runtime; it exists to catch a regression where a multi-output invocation reaches the text formatter and its output bindings would be silently dropped.
- `MacroDef` construction sites: add `outputs: vec![]`.

Do not change `SectionEnd::MacroStart` or `MacroState` in this task — they keep their current shape, and `reader.rs` constructs `MacroDef { ..., outputs: vec![] }`. Tasks 2 and 3 populate the new fields.

**Testing:** None. Structural change only; the compiler is the verifier. The existing `test_macro_def` (ast.rs) and `test_macro_followed_by_equation` (reader.rs) must still pass after their literals are updated.

**Verification:**
Run: `cargo build -p simlin-engine --all-targets`
Expected: compiles with no errors.
Run: `cargo test -p simlin-engine mdl::`
Expected: existing MDL tests still pass (behavior unchanged).

**Commit:** `engine: add AST fields for macro multi-output syntax`
<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: Parse the `:` separator in `:MACRO:` headers

**Verifies:** None directly (parser support; `macros.AC1.2` is completed at import level in Task 5). Required by the design's "Done when: parser unit tests cover the `:` header ... forms".

**Files:**
- Modify: `src/simlin-engine/src/mdl/parser.rs` (the macro-header path in `parse_full_eq_with_units` ~lines 518-545; the `SectionEnd::MacroStart` variant — find its definition and add the `outputs` field)
- Modify: `src/simlin-engine/src/mdl/reader.rs` (`MacroState::InMacro` ~lines 60-70; `handle_parse_result` ~lines 273-332)
- Test: `src/simlin-engine/src/mdl/parser.rs` (inline `#[cfg(test)]` module, near `test_parse_macro_start` ~line 2343); `src/simlin-engine/src/mdl/reader.rs` (inline tests near `test_macro_followed_by_equation` ~line 1304)

**Implementation:**

Step 1 — Extend `SectionEnd::MacroStart` to carry the output list. Change `MacroStart(name, args, loc)` to `MacroStart(name, args, outputs, loc)` (or add an `outputs: Vec<Expr>` field). Update its construction in the parser and its consumption in `reader.rs`.

Step 2 — In the macro-header path (`parse_full_eq_with_units`, parser.rs:518-545): after parsing the input `args` via `parse_expr_list()?.into_exprs()` and **before** `expect(TokenKind::RParen, ...)`, check for an optional `:` separator. `parse_expr_list` naturally stops at `:` (it separates only on `Comma`/`Semicolon`), so the token after the input list is either `RParen` or `Colon`:

```rust
let outputs = if self.peek_kind() == Some(TokenKind::Colon) {
    self.advance_pos(); // consume ':'
    self.parse_expr_list()?.into_exprs()
} else {
    vec![]
};
self.expect(TokenKind::RParen, "')' to close macro arguments")?;
```

Pass `outputs` into `SectionEnd::MacroStart`.

Step 3 — Thread `outputs` through the reader. Add `outputs: Vec<Expr<'input>>` to `MacroState::InMacro`; set it from `MacroStart` in `handle_parse_result`; include it in the `MacroDef { ... }` built on `MacroEnd`.

**Testing:**
Parser/reader unit tests (parser-structural, following the `test_parse_macro_start` idiom):
- `:MACRO: add3(a, b, c : minval, maxval)` → `SectionEnd::MacroStart` with `args.len() == 3`, `outputs.len() == 2`, output names `minval`/`maxval` in order.
- `:MACRO: MYFUNC(arg1, arg2)` (no `:`) → `outputs` is empty (regression: the single-output form is unchanged).
- Reader-level: a full `:MACRO: add3(a,b,c : minval, maxval) ... :END OF MACRO:` block → `MdlItem::Macro` whose `MacroDef.outputs` has the two names in order.
- A `:` header with an empty output list (`add3(a : )`) — `parse_expr_list` calls `parse_expr` unconditionally first (it has no leading-emptiness tolerance), so a `:` immediately followed by `)` produces a **clean parse error** on the `)`. Assert that error is returned (not a panic, not a silently-empty `outputs`), and lock the behavior in with the test.

**Verification:**
Run: `cargo test -p simlin-engine mdl::parser`
Run: `cargo test -p simlin-engine mdl::reader`
Expected: all pass, including the new `:`-header tests.

**Commit:** `engine: parse the ':' separator in :MACRO: headers`
<!-- END_TASK_2 -->

<!-- START_TASK_3 -->
### Task 3: Parse the `:` separator in macro call-site argument lists

**Verifies:** None directly (parser support; multi-output *invocation* is materialized in Phase 4). Required by the design's "Done when: parser unit tests cover the ... call forms".

**Files:**
- Modify: `src/simlin-engine/src/mdl/parser.rs` (the `parse_atom` Symbol-call branch ~lines 1264-1294)
- Test: `src/simlin-engine/src/mdl/parser.rs` (inline `#[cfg(test)]` module)

**Implementation:**

A multi-output macro invocation (`total = add3(a, b, c : minv, maxv)`) lands in the `parse_atom` Symbol branch, because a macro name used in call position is tokenized as a `Symbol` and produces `Expr::App(..., CallKind::Symbol, ...)`. (A `:` in a `CallKind::Builtin` call is not valid Vensim, so only the Symbol branch needs this; add a brief comment in the code stating that.)

In the Symbol branch (parser.rs:1264-1294), after `let args = self.parse_expr_list()?.into_exprs();` and **before** `self.expect(TokenKind::RParen, "')' to close call")?`, parse the optional `:` output bindings exactly as in Task 2:

```rust
let output_bindings = if self.peek_kind() == Some(TokenKind::Colon) {
    self.advance_pos(); // consume ':'
    self.parse_expr_list()?.into_exprs()
} else {
    vec![]
};
let close = self.expect(TokenKind::RParen, "')' to close call")?;
```

Pass `output_bindings` as the new 6th field of `Expr::App`. The existing "symbol call requires at least one argument" check stays as-is (it guards `args`, not the output list).

**Testing:**
Parser unit tests (following the `test_parse_macro_start` idiom — parse a full equation and inspect the RHS `Expr`):
- `total = add3(a, b, c : minv, maxv)` → RHS is `Expr::App` with `args.len() == 3`, `output_bindings.len() == 2`, names `minv`/`maxv` in order, `CallKind::Symbol`.
- `y = MYMACRO(a, b)` (no `:`) → `Expr::App` with `output_bindings` empty (regression).
- A `:` call nested in a larger expression (`y = c + add3(a, b, c : minv, maxv)`) → the `App` inside the `Op2` carries the `output_bindings`.

**Verification:**
Run: `cargo test -p simlin-engine mdl::parser`
Expected: all pass, including the new `:`-call tests.

**Commit:** `engine: parse the ':' separator in macro call sites`
<!-- END_TASK_3 -->
<!-- END_SUBCOMPONENT_A -->

<!-- START_SUBCOMPONENT_B (tasks 4-6) -->
## Subcomponent B: Convert each `MacroDef` into a macro-marked `Model`

<!-- START_TASK_4 -->
### Task 4: Make the conversion pipeline reusable for a scoped sub-model

**Verifies:** None (behavior-preserving refactor; verified by existing tests still passing).

**Files:**
- Modify: `src/simlin-engine/src/mdl/convert/mod.rs` (`new_with_data` ~lines 98-141; `convert()` ~lines 144-197)
- Modify: `src/simlin-engine/src/mdl/convert/variables.rs` (`build_project` ~lines 28-106)

**Background — why this refactor:** A macro body is a little model with its own variable namespace. `macro_cross_reference` defines two macros that both have body references to `input`/`parameter`; `macro_stock` has a body stock (`INTEG`) needing the stock/flow-linking pass. The body must therefore be converted with a *scoped* symbol table, not the global one. The existing `ConversionContext` pipeline already converts "a list of `MdlItem`s into a `Model`" — it is only hardwired to (a) drain its items from an `EquationReader` and (b) emit exactly one model named `"main"`. This task removes those two hardwirings without changing any observable behavior.

**Implementation:**

Investigate `convert/mod.rs` and `convert/variables.rs` in full, then make these two behavior-preserving changes:

1. **Injectable item list.** Factor the reader-draining out of `new_with_data` so a `ConversionContext` can be constructed over a caller-supplied `Vec<MdlItem>` (e.g. add `ConversionContext::new_from_items(items, dimensions, data_provider, formatter, ...)` and have `new_with_data` call it after draining the reader). The macro path (Task 5) builds a sub-context from a macro's body equations this way. **The sub-context shares the parent's already-built `dimensions`, `data_provider`, and `formatter` — it does not rebuild or independently re-derive them** (the macro body defines no dimensions of its own, and the formatter's subrange-name state must match the parent's). Whether "shares" is a borrow, an `Rc`, or a cheap clone is at the implementor's discretion as long as the sub-context uses the *same* dimensions/data_provider/formatter the parent built — Task 5 relies on this.

2. **Parameterized model name.** Extract from `build_project` a method `build_model(&self, name: &str) -> Result<datamodel::Model, ConvertError>` that builds `variables` / `views` / `groups` into a `Model` (with `sim_specs: None`, `macro_spec: None`). `build_project` becomes `build_model("main")` followed by the `Project` assembly. This lets the macro path produce a `Model` named after the macro.

The exact mechanics (helper signatures, which fields move where) are at the implementor's discretion **as long as**: the `"main"` model conversion is byte-for-byte unchanged, the public `convert_mdl` / `convert_mdl_with_data` signatures are unchanged, and a sub-context can be built from a `Vec<MdlItem>` plus the parent's shared `dimensions`, `data_provider`, and `formatter`, and have `collect_symbols` / `mark_variable_types` / `scan_for_extrapolate_lookups` / `link_stocks_and_flows` / `build_model` run on it.

**Testing:** None new. This is a refactor — its correctness is "every existing test still passes."

**Verification:**
Run: `cargo test -p simlin-engine mdl::`
Run: `cargo test -p simlin-engine --test mdl_equivalence`
Run: `cargo test -p simlin-engine --test simulate simulates_except_basic_mdl`
Expected: all pass unchanged — the refactor introduced no behavior change.

**Commit:** `engine: make MDL conversion pipeline reusable for sub-models`
<!-- END_TASK_4 -->

<!-- START_TASK_5 -->
### Task 5: Convert each `MacroDef` into a macro-marked `Model`

**Verifies:** macros.AC1.1, macros.AC1.2, macros.AC1.7.

**Files:**
- Modify: `src/simlin-engine/src/mdl/convert/mod.rs` (add a macro-conversion step in `convert()`, after `build_project`) and/or `src/simlin-engine/src/mdl/convert/variables.rs`
- Create or modify: a `convert/` module file for the macro-conversion routine (e.g. a new `src/simlin-engine/src/mdl/convert/macros.rs`, declared `mod macros;` in `convert/mod.rs`, following the file-per-concern layout of `stocks.rs` / `variables.rs` / `dimensions.rs`)
- Modify: `src/simlin-engine/src/mdl/convert/helpers.rs` (a helper to extract a canonical ident from a `MacroDef` arg `Expr::Var`, if one does not already exist)
- Test: the inline `#[cfg(test)] mod tests` in `convert/mod.rs` (near `test_mdl_group_variables_after_macro` ~line 616)

**Implementation:**

Add a conversion step that runs after the `"main"` model is built (the `MdlItem::Macro` entries are still in `self.items` — `collect_symbols` leaves them there). For each `MdlItem::Macro(macro_def)`:

1. **Build the macro's body model.** Wrap `macro_def.equations` (a `Vec<FullEquation>`) as `MdlItem::Equation` entries, build a scoped `ConversionContext` over them (Task 4's `new_from_items`, sharing the parent's `dimensions` and `data_provider`), run `collect_symbols` / `mark_variable_types` / `scan_for_extrapolate_lookups` / `link_stocks_and_flows`, then `build_model(&macro_name)`. This handles single-aux bodies (`macro_expression`), multi-equation bodies with helpers (`macro_multi_expression`'s `intermediate`), and stock bodies (`macro_stock`'s `INTEG`) for free, because those are exactly what the existing passes do.

2. **Synthesize port variables for the formal parameters.** A macro's formal parameters (`input`, `parameter`) are referenced by the body equations but are not themselves defined by body equations — yet the engine's module machinery requires the sub-model to contain an actual `Variable` for every input port. This is exactly how stdlib models work: `stdlib⁚smth1` defines `input`, `delay_time`, `initial_value` as ordinary variables with placeholder-constant equations (`"0"`, `"1"`, `"NAN"`), and `stdlib⁚delay1` defines its `input` port as a `Flow`. So, after `build_model`, append a synthesized port `Variable` to the macro `Model.variables` for each name in `MacroSpec.parameters` that is not already a variable in the model:
   - **Kind:** if the parameter name appears in any body stock's `inflows` or `outflows` list, synthesize a `Variable::Flow`; otherwise synthesize a `Variable::Aux`. (`macro_stock`'s `input` is the `INTEG` rate → `Flow`, mirroring `stdlib⁚delay1`; its `parameter` is the initial value → `Aux`, mirroring `stdlib⁚smth1`'s `input`.)
   - **Equation:** a placeholder constant `Equation::Scalar("0")`. The placeholder only ever runs if the port is left unwired; a macro invocation always overrides it with the call-site argument.
   - **`compat`:** `Compat { can_be_module_input: true, ..Compat::default() }`. This flag is **required**: it is how `collect_module_idents` recognizes the port as a module-input slot (so `PREVIOUS(port)` inside the body compiles correctly). It is the same flag an ordinary XMILE submodel sets via `access="input"`. Without it, a macro model — which is registered as an ordinary, non-`stdlib⁚`-prefixed sub-model — can mis-compile.
   - This makes the macro `Model` a complete, self-contained, compilable sub-model — structurally identical to an ordinary XMILE submodel — and the design's intent that `MacroSpec.parameters` name body variables becomes literally true.
   - If running the scoped conversion on the body alone causes `link_stocks_and_flows` to error on the undefined parameter references (rather than leaving them as resolved-at-compile-time names), the alternative is to prepend a synthetic `<param> = 0` `MdlItem::Equation` for each parameter *before* the scoped conversion so the pipeline assigns kinds itself, then set `can_be_module_input = true` on the results. Investigate `link_stocks_and_flows` and pick whichever yields a correct, compilable macro `Model`; the verification below is the safety net.

3. **Build the `MacroSpec`.** On the resulting `Model`, set `macro_spec: Some(MacroSpec { parameters, primary_output, additional_outputs })`:
   - `parameters` — one canonical ident per `Expr` in `macro_def.args`, in order. Extract the name from each `Expr::Var` and canonicalize it with the **same** function the body-equation formatter uses for `Var` identifiers (so `MacroSpec.parameters` entries are byte-identical to how the body equations reference them, and to the synthesized port-variable idents — verify this with the test below). If an `args` entry is not a `Var`, that is a malformed macro header — return `ConvertError::Other` naming the macro.
   - `primary_output` — the macro's own name, canonicalized the same way as a variable ident (e.g. `quoted_space_to_underbar`). Vensim requires the body to contain an equation whose LHS is the macro name; Phase 2 sets `primary_output` to the canonicalized macro name without validating that the equation exists (a missing primary-output equation surfaces later, in Phase 3's compiler).
   - `additional_outputs` — one canonical ident per `Expr` in `macro_def.outputs`, in order (empty for the 6 fixtures; populated for the `:`-header form from Task 2). Unlike `parameters`, additional outputs are computed by the body and so already have body equations — do **not** synthesize placeholders for them.

4. **Push the macro `Model` into `project.models`**, alongside `"main"`.

**Steps 2 and 3 must be a single named, reusable function** — Phase 5's XMILE reader reuses exactly this port-synthesis + `MacroSpec`-construction logic. Implement it as a `pub(crate)` function, e.g. `Model::new_macro(macro_name: &str, parameters: &[String], additional_outputs: &[String], body_variables: Vec<Variable>) -> Model`, placed in `datamodel.rs` (as an associated function on `Model`) or a small shared module — somewhere callable from **both** `mdl/convert/` and `xmile/`. It takes an already-built body variable list, synthesizes the port variables (step 2), builds and attaches the `MacroSpec` (step 3), and returns the complete macro-marked `Model`. The format-specific part — step 1, building `body_variables` from `MacroDef.equations` via the scoped `ConversionContext` — stays in `mdl/convert/`; it produces the `body_variables` that get passed to the shared function. (Phase 5 Task 1 calls this same function with `body_variables` built from XMILE `<variables>`/`<eqn>`.)

Single-output macro *invocations* (`macro output = EXPRESSION MACRO(macro input,macro parameter)`, and the body-level `EXPRESSION MACRO = SECOND MACRO(input,parameter)` in `macro_cross_reference`) are **not** materialized here — they remain ordinary equation text in their `Aux`/`Stock`/`Flow`, exactly as a `SMTH1(...)` call is today. They are resolved and expanded in Phase 3.

Definition order does not matter: `macro_trailing_definition` defines the macro after its call site, but the reader collects all `MdlItem`s before `convert()` runs, so the macro `Model` is produced regardless of where the `:MACRO:` block appears (this is the design's deliberate "definition-order leniency").

**Testing:**
Tests in `convert/mod.rs`'s inline test module, using `convert_mdl(source)`:
- **macros.AC1.1:** convert a small `.mdl` with `:MACRO: EXPRESSION MACRO(input, parameter)` / `EXPRESSION MACRO = input * parameter` plus a `main`-model invocation. Assert `project.models` contains a model whose `macro_spec` is `Some` with `parameters == ["input", "parameter"]` and `primary_output == "expression_macro"`; assert the macro model's `variables` contains the body aux `expression_macro` and that its equation text references `input` and `parameter` byte-identically to the `parameters` entries; assert the macro model also contains synthesized port variables `input` and `parameter`, each with `compat.can_be_module_input == true`. Assert the `"main"` model still has `macro_spec: None` and that the invocation `macro output = EXPRESSION MACRO(...)` is preserved as equation text.
- **macros.AC1.2:** convert a `.mdl` with `:MACRO: add3(a, b, c : minval, maxval)` and a small body. Assert the macro model's `MacroSpec.additional_outputs == ["minval", "maxval"]` in order, and `parameters == ["a", "b", "c"]`; assert port variables `a`, `b`, `c` were synthesized (with `can_be_module_input == true`) but `minval`/`maxval` were **not** synthesized as ports (they are body-computed outputs).
- **macros.AC1.7:** convert a `.mdl` that defines a macro but never invokes it. Assert the macro-marked `Model` is still present in `project.models` with the correct `MacroSpec` and synthesized port variables.
- A stock-bodied macro (`EXPRESSION MACRO = INTEG(input, parameter)`): assert the macro model's body variable `expression_macro` is a `Variable::Stock`, that the synthesized `input` port is a `Variable::Flow` and `parameter` a `Variable::Aux` (both with `can_be_module_input == true`), and that the stock's inflow resolves to `input` (directly, or via a synthetic flow whose equation is `input`).

**Verification:**
Run: `cargo test -p simlin-engine convert`
Expected: all pass, including the new macro-conversion tests.

**Commit:** `engine: import MDL macro definitions as macro-marked models`
<!-- END_TASK_5 -->

<!-- START_TASK_6 -->
### Task 6: Translate `$`-suffixed time references in macro bodies

**Verifies:** macros.AC1.5.

**Files:**
- Modify: `src/simlin-engine/src/mdl/convert/helpers.rs` (add a `pub(super)` `$`-time rewrite helper)
- Modify: the macro-conversion routine from Task 5 (apply the rewrite to each macro body equation's expression tree before it is formatted)
- Test: `convert/helpers.rs` inline tests (helper-level) and `convert/mod.rs` inline tests (import-level)

**Implementation:**

A `$`-suffixed time reference in a macro body (`Time$`, `TIME STEP$`, `INITIAL TIME$`, `FINAL TIME$`) is Vensim's escape to reach the caller's global time variables. After lexing, such a reference is an `Expr::Var` whose name *includes* the trailing `$` (e.g. `"Time$"` or `"TIME STEP$"`). The engine already resolves the bare canonical time idents inside a module body at any nesting depth (they are zero-arity builtins — `builtins.rs:354-367`), so the only work is a front-end name translation.

Add a helper in `convert/helpers.rs` that recursively walks an `Expr` tree and rewrites every `Expr::Var` whose name — lowercased and space-normalized, with the trailing `$` stripped — matches a Vensim time variable, replacing it with the canonical engine ident:
- `time$` → `time`
- `time step$` → `time_step`
- `initial time$` → `initial_time`
- `final time$` → `final_time`
- (`dt$` → `dt` may be included for completeness; the corpus only exercises `Time$` and `TIME STEP$`.)

The rewrite must recurse through `Op1`, `Op2`, `Paren`, and the `args` (and `output_bindings`) of `App` so a `$`-time reference nested in a larger expression is caught. Apply it in the Task 5 macro-conversion routine to each body equation's expression **before** formatting. Scope it to macro bodies only — do not touch the global `XmileFormatter` or non-macro equations (a `$`-time reference outside a macro body is meaningless and out of scope).

This is the design's "non-time `$` escape ... deprioritized" boundary: only `$`-*time* is translated; a `$`-suffixed reference to a non-time variable is out of Phase 2 scope.

**Testing:**
- Helper-level (in `convert/helpers.rs` inline tests): build small `Expr` trees containing `Var("Time$")`, `Var("TIME STEP$")`, `Var("Initial Time$")`, and a nested `c + MYMACRO(Time$, x)` shape; assert the rewrite produces `time`, `time_step`, `initial_time`, and rewrites the nested occurrence; assert a non-time `Var` (`"foo$"`, `"input"`) is left untouched.
- **macros.AC1.5** (import-level, in `convert/mod.rs` tests): `convert_mdl` a `.mdl` whose macro body uses `Time$` and `TIME STEP$` (e.g. `MYMACRO = input * Time$ / TIME STEP$`); assert the imported macro model's body equation text references `time` and `time_step`, not `Time$` / `TIME STEP$`.

**Verification:**
Run: `cargo test -p simlin-engine convert`
Expected: all pass, including the `$`-time helper and import tests.

**Commit:** `engine: translate $-time references in macro bodies`
<!-- END_TASK_6 -->
<!-- END_SUBCOMPONENT_B -->

<!-- START_SUBCOMPONENT_C (task 7) -->
## Subcomponent C: Fixture import verification

<!-- START_TASK_7 -->
### Task 7: Import-verify the 6 macro fixtures and the unterminated-macro error

**Verifies:** macros.AC1.1, macros.AC1.6, macros.AC1.7.

**Files:**
- Test: `src/simlin-engine/src/mdl/convert/mod.rs` inline `#[cfg(test)] mod tests` (or a new `src/simlin-engine/src/mdl/convert/macro_tests.rs` declared `#[cfg(test)] mod macro_tests;`, following the project's `*_tests.rs` separate-file convention for larger test sets)

**Implementation:** Tests only — no production code. Embed each fixture with `include_str!` so a missing fixture is a compile error (the project's stated preference: "if a required test file is missing, fail loudly"). From `src/simlin-engine/src/mdl/convert/mod.rs`, the path to a fixture is `../../../../../test/test-models/tests/<name>/<file>.mdl` (five `../` to the repo root).

**Testing:**
For each of the 6 `test/test-models/tests/macro_*` `.mdl` fixtures, `convert_mdl(include_str!(...))` and assert (**macros.AC1.1**):
- `project.models` contains the expected macro-marked model(s) — `macro_cross_reference` and `macro_multi_macros` produce two macro models each; the rest produce one.
- Each macro model's `macro_spec` is `Some` with `parameters` matching the header input list in order (e.g. `["input", "parameter"]`) and `primary_output` equal to the canonicalized macro name.
- Each macro model's `variables` match the body equations **plus** a synthesized port variable per formal parameter (each with `compat.can_be_module_input == true`): `macro_expression` → body aux `expression_macro` + port vars `input`, `parameter`; `macro_multi_expression` → body auxes `expression_macro` and the `intermediate` helper + port vars `input`, `parameter`; `macro_stock` → body stock `expression_macro` + port `Flow` `input` + port `Aux` `parameter`; `macro_cross_reference`'s `EXPRESSION MACRO` body → body aux `expression_macro` whose equation references `SECOND MACRO` as a call (equation text, not expanded) + port vars `input`, `parameter`.
- The `"main"` model has `macro_spec: None` and preserves each invocation as equation text.
- **macros.AC1.7:** `macro_trailing_definition` — the macro is defined after its call site; assert the macro model is still present and correct, and `main`'s `macro output` invocation is preserved.

For **macros.AC1.6**: assert that `open_vensim` (or `convert_mdl`) on a `.mdl` containing `:MACRO: BAD(x)` ... with **no** `:END OF MACRO:` returns `Err`; the error must be `ConvertError::Reader(ReaderError::EofInsideMacro)` and its `Display` must be the clear message `"reader error: unexpected end of file inside macro"` (through `open_vensim`, the message is `"Failed to parse MDL: reader error: unexpected end of file inside macro"`). Also assert a stray `:END OF MACRO:` with no opening `:MACRO:` yields `ReaderError::UnmatchedMacroEnd`.

**Verification:**
Run: `cargo test -p simlin-engine convert`
Expected: all 6 fixture-import tests and both error-case tests pass.

**Commit:** `engine: import-verify the macro test fixtures`
<!-- END_TASK_7 -->
<!-- END_SUBCOMPONENT_C -->

---

## Phase 2 completion check

When all seven tasks are committed:
- The MDL parser handles the full `:MACRO:` syntax, including the `:` multi-output separator in both headers and call sites — parser unit tests cover both forms (the design's "Done when").
- Every `.mdl` `:MACRO:` block imports as a macro-marked `datamodel::Model` carrying a correct `MacroSpec` and synthesized port variables for its formal parameters (so the macro `Model` is a complete, compilable sub-model); the 6 `test/test-models/tests/macro_*` `.mdl` fixtures import into datamodels with correct `MacroSpec`s and macro bodies (the design's "Done when").
- `$`-suffixed time references in macro bodies translate to canonical engine time idents (the design's "Done when").
- An unterminated `:MACRO:` block reports a clear parse error.
- `macros.AC1.1`, `macros.AC1.2`, `macros.AC1.5`, `macros.AC1.6`, `macros.AC1.7` are verified.

Macro *invocations* are still plain equation text — resolution and expansion (so macros actually simulate) is Phase 3; multi-output invocation materialization is Phase 4.
