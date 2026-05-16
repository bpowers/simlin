# Vensim Macros: Implementation Reference

Status: reference document for designing and implementing Vensim macro support in
`simlin-engine`. The engine currently *parses* `:MACRO:` blocks but discards them
(`src/simlin-engine/src/mdl/convert/mod.rs:248`). This document collects what a
Vensim macro is, how it is written, and -- most importantly -- its precise
runtime semantics, so an implementation design can be grounded in fact rather
than guesswork.

Authoritative sources used (cited inline throughout):

- Vensim documentation, "Macros": https://www.vensim.com/documentation/macros.html
- Vensim documentation, "Defining Macros": https://www.vensim.com/documentation/22145.html
- Vensim documentation, "Using Macros": https://www.vensim.com/documentation/22150.html
- Vensim documentation, "Function and Language Changes": https://www.vensim.com/documentation/function_changes.html
- Vensim documentation, version 5.8 release notes: https://www.vensim.com/documentation/version_5_8___-___.html
- XMILE v1.0 OASIS specification (local copy: `docs/reference/xmile-v1.0.html`,
  saved from https://docs.oasis-open.org/xmile/xmile/v1.0/xmile-v1.0.html) --
  sections 2.2.1, 2.10, 3.6/3.6.1/3.6.2, and 4.8/4.8.1/4.8.2/4.8.3.
- Local repository: the six `test/test-models/tests/macro_*` fixtures, the
  C-LEARN hero model, the engine MDL parser (`src/simlin-engine/src/mdl/`), the
  XMILE serde layer (`src/simlin-engine/src/xmile/mod.rs`), and the bundled
  `xmutil` Vensim-to-XMILE converter (`src/xmutil/third_party/xmutil/`).

A note on terminology: throughout, "the model" or "the whole-model" means the
top-level model that *invokes* a macro, as distinguished from the contents of
the macro definition itself.

---

## 1. What a Vensim macro is

A Vensim macro is a **named, parameterized fragment of model structure** -- one
or more equations, optionally including stocks, that is defined once and then
"called like a function" from anywhere in the model. Per the Vensim "Macros"
overview, macros exist so that modelers can "repeat model structures without
retyping equations"
(https://www.vensim.com/documentation/macros.html).

Conceptually a macro is closer to a **textual / structural template that is
expanded inline at each call site** than to a true runtime function:

- "When the macro is encountered, variables and equations will be made up so
  that causal tracing will continue to function properly"
  (https://www.vensim.com/documentation/22150.html). That is, calling a macro
  *materializes new model variables* -- it does not perform an opaque function
  call.
- "When Vensim expands the macro to create the detailed equations it does the
  expansion recursively" for nested macros
  (https://www.vensim.com/documentation/22145.html).
- The XMILE spec describes the same idea from the encoding side: macros with
  variables "are, in fact, independent models that run to completion each time
  they are invoked" (XMILE 1.0 section 3.6.1).

How a macro differs from a regular variable: a regular variable has exactly one
value (or one arrayed value) in the model namespace. A macro has *no value of
its own*; it is invoked, and each invocation produces a fresh set of expanded
variables that do have values.

How a macro differs from a built-in function: a built-in function (e.g. `MIN`,
`ABS`) is implemented by the engine and is stateless from the model's point of
view. A macro is defined in the model's own language and *can carry state*
(stocks). In fact several Vensim "functions" are themselves macros under the
hood: `DELAY1`, `DELAY1I`, `DELAY3`, `DELAY3I`, `DELAYP`, `FORECAST`, `NPV`,
`NPVE`, `SMOOTH`, `SMOOTH3`, `SMOOTH3I`, and `TREND` are all "defined as Macros"
(https://www.vensim.com/documentation/macros.html). This is the key reason a
macro can introduce a hidden stock: the same mechanism that lets `SMOOTH`
introduce a hidden level is the macro mechanism.

Why modelers use them: reuse of structure, and the ability to package a
behavior (a custom delay, a custom smoothing, a custom NPV variant) as a
callable unit. Why the Vensim docs are ambivalent about them: macros "are also
dangerous in that they can allow dynamics to 'creep into' a model," and the
documentation explicitly recommends using them "sparingly, if at all"
(https://www.vensim.com/documentation/macros.html). The use of *output
arguments* in particular is called out as "discouraged"
(https://www.vensim.com/documentation/22145.html).

Caveat on Vensim editions: defining macros, the `$` model-variable escape, and
nested/recursive macro expansion are Pro/DSS features. PLE and PLE Plus cannot
define macros, and cannot even display the hidden variables that built-in
macro-functions create
(https://www.vensim.com/documentation/macros.html,
https://www.vensim.com/documentation/function_changes.html).

---

## 2. Definition syntax

### 2.1 The `:MACRO:` header

A macro definition is introduced by a `:MACRO:` line and closed by a
`:END OF MACRO:` line. The header has the form
(https://www.vensim.com/documentation/22145.html):

```
:MACRO: macroname(inarg1, inarg2, inarg3 : outarg1, outarg2)
```

- **`macroname`** -- "any valid unquoted Vensim name": it must start with a
  letter and may contain letters, numbers, spaces, underbars (`_`) and dollar
  signs (`$`). Because Vensim names may contain spaces, a macro can be named
  `EXPRESSION MACRO` or `RAMP FROM TO` (both appear in the local fixtures and
  the C-LEARN model). A macro name *may shadow a built-in function name* -- the
  C-LEARN model defines a macro literally named `SSHAPE`, which is also a Vensim
  built-in; the engine's parser already accommodates this by accepting either a
  `Symbol` or a `Function` token for the macro name
  (`src/simlin-engine/src/mdl/parser.rs:521-523`).

- **Input argument list** -- "any number" of arguments, each "any valid
  unquoted name." Arguments are positional. "The arguments cannot be
  subscripted" in the definition, "but you can use subscripted values when
  calling the macro" (https://www.vensim.com/documentation/22145.html). Input
  arguments are separated by commas.

- **The `:` separator and output argument list** -- the `:` character (when
  present) separates the *input* argument list (before the colon) from the
  *output* argument list (after the colon). Output arguments "are variables
  that are defined inside of the macro, in addition to the output of the macro
  itself. These are optional and must be separated from input arguments by a
  colon. Their use is discouraged. When no outputs are specified, omit the
  colon" (https://www.vensim.com/documentation/22145.html).

  Concretely, the `add3` example from the Vensim "Using Macros" page declares
  two extra outputs:

  ```
  :MACRO: add3(val1, val2, val3 : minval, maxval)
  add3 = val1 + val2 + val3
      ~ val1
      ~ |
  minval = min(val1, min(val2, val3))
      ~ val1
      ~ |
  maxval = max(val1, max(val2, val3))
      ~ val1
      ~ |
  :END OF MACRO:
  ```

  (https://www.vensim.com/documentation/22150.html -- the page's own example
  has typos, repeating `val2`; the corrected intent is shown above.)

### 2.2 The macro's own output

Even with no output list, every macro has exactly one "output of the macro
itself": **the variable whose name equals the macro name.** "The macroname and
output arguments each must have a single equation inside of the macro
description" (https://www.vensim.com/documentation/22145.html).

So in:

```
:MACRO: EXPRESSION MACRO(input, parameter)
EXPRESSION MACRO = input * parameter
    ~ input
    ~ tests basic macro containing no stocks and having no output
    |
:END OF MACRO:
```

(from `test/test-models/tests/macro_expression/test_macro_expression.mdl`) the
equation `EXPRESSION MACRO = input * parameter` defines the macro's return
value. When there is no output list, the macro name *is* the output -- that is
what "When no outputs are specified, omit the colon" means: there is still an
output, it is just the macro-named variable and nothing else.

When there *is* an output list, the macro still has its macro-named output
*plus* each named output variable. In the `add3` example above, `add3` is the
primary output and `minval`/`maxval` are additional outputs that the caller can
also retrieve (see section 4.5).

### 2.3 Argument naming rules

- Input and output argument names are "any valid unquoted name"
  (https://www.vensim.com/documentation/22145.html).
- Argument names are **local to the macro** (see section 5.4). The Vensim docs
  note you "can use, for example, `L1` in as many macros as you like"
  (https://www.vensim.com/documentation/22150.html).
- Arguments are bound positionally at the call site (section 5.2).
- Argument names cannot carry subscripts in the header
  (https://www.vensim.com/documentation/22145.html).

### 2.4 Placement, ordering, and scope in a `.mdl` file

- A macro definition is a top-level block in the `.mdl` file's equation
  section, peer to ordinary variable equations and group (`****`) markers. The
  engine models this directly: the MDL reader produces `MdlItem::Macro` as a
  sibling of `MdlItem::Equation` and `MdlItem::Group`
  (`src/simlin-engine/src/mdl/ast.rs:498-506`).

- **Ordering rule.** "The macro definition must occur before the macro is
  referenced. This is the only equation ordering rule in the Vensim modeling
  language. If you do not define a macro before you use it, you will receive a
  syntax error message" (https://www.vensim.com/documentation/22145.html, and
  restated at https://www.vensim.com/documentation/macros.html). This is
  significant: Vensim equations are otherwise order-independent, but macro
  *definitions* must lexically precede their *uses*.

  The local fixture `test/test-models/tests/macro_trailing_definition/` is a
  deliberate counter-example: it places the `:MACRO:` block *after* the
  `macro output = EXPRESSION MACRO(...)` call that uses it. Its README still
  calls it a valid test and it has an `output.tab`, which means the
  *third-party* tools that produced the fixture (and the SD community's
  expectations) tolerate a trailing definition even though the Vensim manual
  forbids it. Treat the "definition before use" rule as Vensim's stated
  contract, but be aware real-world files and other tools may not enforce it,
  and the engine should decide deliberately whether to be strict or lenient.

- **Multiple macros.** A file may define any number of macros. The fixtures
  `macro_multi_macros/` (two independent macros) and `macro_cross_reference/`
  (one macro calls another) both exercise this. Each `:MACRO: ... :END OF
  MACRO:` is a separate block.

- **Scope.** A macro definition introduces its own private namespace. Names
  defined inside one macro do not leak into the model or into other macros (see
  section 5.4). The macro *name* itself, however, becomes visible in the model
  namespace so it can be called.

### 2.5 The `:END OF MACRO:` terminator

`:END OF MACRO:` closes the most recently opened `:MACRO:` block. There is no
nesting of `:MACRO: ... :END OF MACRO:` blocks themselves -- nesting of macros
is achieved by *calling* one macro from inside another (section 5.5), not by
lexically embedding one definition inside another. The engine treats an
unmatched `:END OF MACRO:` and an end-of-file inside an open macro as errors
(`ReaderError::UnmatchedMacroEnd`, `ReaderError::EofInsideMacro` in
`src/simlin-engine/src/mdl/reader.rs:27-30`).

---

## 3. The macro body

Between `:MACRO:` and `:END OF MACRO:` the body is a sequence of ordinary
Vensim equations, written in the normal `name = rhs ~ units ~ comment |`
format. From the Vensim docs and the local fixtures, the body may contain:

- **Auxiliaries / intermediate variables.** "Variables can also be defined
  internal to the macro description" beyond the required macro-named and
  output-named equations (https://www.vensim.com/documentation/22145.html). The
  XMILE spec adds that auxiliaries inside a macro are "useful to simplify the
  equation into smaller, meaningful pieces" (XMILE 1.0 section 4.8.2). The
  `macro_multi_expression/` fixture has a body with an `intermediate` variable;
  C-LEARN's `RAMP FROM TO` macro body has six internal auxiliaries (`linear`,
  `linear ramp`, `exp ramp`, `slope`, `rate`, `interval`).

- **Constants.** A body equation can be a literal constant.

- **Stocks (`INTEG`).** A macro body may contain `INTEG(...)` equations. The
  `macro_stock/` fixture's macro is `EXPRESSION MACRO = INTEG(input,
  parameter)`. C-LEARN's `SAMPLE UNTIL` macro contains
  `SAMPLE UNTIL = INTEG(..., initval)`. The XMILE spec's SMOOTH1 macro example
  is built from a stock and a flow (XMILE 1.0 section 4.8.2). Per-invocation
  state of these stocks is the single most important semantic point and is
  covered in section 5.1.

- **Lookups / table functions.** The XMILE spec does not prohibit them, and the
  body is "all of the syntax of XMILE" (XMILE 1.0 section 3.6.1) / Vensim. None
  of the local fixtures exercises a lookup inside a macro, so there is no local
  ground-truth output for this case -- flag it as something to test against
  Vensim directly if the implementation needs to support it.

- **Subscripted / arrayed variables.** Vensim is explicit and restrictive
  here: "There is no support for Subscripts within Macro definitions. When a
  Macro is called, the variables should be used with the same subscripts they
  appear with normally. When the macro equation is expanded, the created
  variables inherit the Subscripts from the left hand side of the equation.
  Each equation created during the expansion of the Macro has the same
  subscripts on the left hand side" (https://www.vensim.com/documentation/macros.html).
  In other words, the macro body is written *as if scalar*; subscripts are
  supplied entirely from the call-site LHS and propagated onto every expanded
  equation. XMILE, by contrast, permits arrays inside macros and even shows a
  recursive `FIND` macro indexing into a 1-D array argument with `A[i]` (XMILE
  1.0 section 4.8.1) -- so the two formats differ on array handling inside
  macros. See section 5.6.

- **References to simulation control.** The macro body cannot name
  `TIME STEP`, `INITIAL TIME`, `FINAL TIME`, or `TIME` directly as ordinary
  identifiers (those would be treated as local, undefined names). It reaches
  them via the `$` escape (section 3.1). C-LEARN's `SAMPLE UNTIL` macro uses
  `TIME STEP$`; `RAMP FROM TO` uses `Time$` and `TIME STEP$`; `INIT` uses
  `INITIAL(x)`.

### 3.1 Can a macro body variable reference things outside the macro?

By default, **no** -- the macro body is a "self-contained world that can be
accessed only through the macro call itself"
(https://www.vensim.com/documentation/22145.html). There is exactly one
documented exception:

> "The one exception to the local nature of macro variables is that you may
> specify a model variable by following its name with a dollar sign `$`. For
> example you can use `TIME STEP$` inside of a macro to refer to the model
> variable `TIME STEP`. Only unsubscripted variables may be referred to in this
> manner." (https://www.vensim.com/documentation/22145.html)

So `Time$`, `TIME STEP$`, `INITIAL TIME$`, etc. inside a macro body refer to
the *model's* (the caller's) corresponding variable. This is a Pro/DSS-only
feature (https://www.vensim.com/documentation/function_changes.html). The `$`
escape is restricted to *unsubscripted* model variables.

This `$`-suffixed reference is a real, load-bearing piece of syntax that the
engine's MDL lexer/parser must handle for the C-LEARN hero model -- e.g.
`SAMPLE UNTIL = INTEG( (1-STEP(1,lastTime))*(input-SAMPLE UNTIL)/TIME STEP$,
initval)`. The implementation must decide how a macro-body `$` reference maps
onto whatever representation a macro becomes (e.g. an implicit module input
wired to the caller's `time_step`).

### 3.2 Units inside a macro body

Units in a macro body equation may be either ordinary model units (`People`,
`$`, etc.) *or the name of an input argument*. "If the units are the name of an
input variable, the units from the input will be substituted wherever the macro
is invoked" (https://www.vensim.com/documentation/22145.html). A name like
`Time$` may also be used as a units specifier. Note in every local fixture the
macro-named equation's units are written as the *name of the first input
argument* (`~ input`), which is exactly this "units = input argument name"
convention -- the macro's output unit is whatever unit the actual first
argument had at the call site.

---

## 4. Call-site / invocation syntax

### 4.1 Basic call

Once defined, a macro "can be used any number of times. To use it, call it just
like a function" (https://www.vensim.com/documentation/22150.html). The call
appears on the right-hand side of an ordinary equation:

```
macro output = EXPRESSION MACRO(macro input, macro parameter)
```

(from `test/test-models/tests/macro_expression/test_macro_expression.mdl`). The
syntax is `MACRONAME(arg1, arg2, ...)` -- syntactically indistinguishable from a
function call or a lookup invocation. The engine's MDL parser does not even
recognize it as a macro call at parse time: a multi-argument call on a `Symbol`
token is parsed as `Expr::App` with `CallKind::Symbol` and described in the AST
as "unknown function (could be macro or error)"
(`src/simlin-engine/src/mdl/ast.rs:97,106,127`). Resolution of "is this name a
macro?" therefore has to happen *after* parsing, against the table of collected
macro definitions.

### 4.2 Can arguments be arbitrary expressions?

Per the Vensim docs the call is "just like a function" and the local + C-LEARN
fixtures show:

- **Simple identifiers** -- `EXPRESSION MACRO(macro input, macro parameter)`
  (all `macro_*` fixtures).
- **Subscripted variables** -- `SMOOTH(tadum[house], 20)` in the Vensim "Using
  Macros" page (https://www.vensim.com/documentation/22150.html). Vensim
  explicitly says "you can use subscripted values when calling the macro"
  (https://www.vensim.com/documentation/22145.html).
- **Numeric literals** -- the `20` in `SMOOTH(tadum[house], 20)` and the
  literals passed to C-LEARN's built-in `DELAY`/`STEP`/`RAMP` macro-functions.
- **Arbitrary sub-expressions** -- C-LEARN's macro-function calls receive
  compound expressions (e.g. `STEP(1, lastTime)` is itself nested inside the
  macro body, but at the model level functions-implemented-as-macros such as
  `SMOOTH`/`DELAY3` routinely receive full expressions as arguments). The MDL
  grammar both in `xmutil` and in the engine parses macro-call arguments as a
  general `exprlist` (`src/xmutil/.../Vensim/VYacc.y:99`,
  `src/simlin-engine/src/mdl/parser.rs:528`), so *syntactically* any expression
  is accepted.

The engine's AST documentation flags the tension: the grammar "parses macro
arguments as `exprlist`, which allows arbitrary expressions. In valid macros,
these should be simple variable references"
(`src/simlin-engine/src/mdl/ast.rs:478-482`). That comment is about the
*definition header* (where args must be plain names); at the *call site*,
expression-valued arguments are normal and expected (think `SMOOTH(a + b, t)`).
This is an important asymmetry: header argument list = plain names only;
call-site argument list = arbitrary expressions.

### 4.3 Can a macro call be nested inside a larger expression?

Yes. The Vensim "Using Macros" page's first example is
`diddle[house] = daddle + SMOOTH(tadum[house], 20)` -- the `SMOOTH` macro call
is one operand of a `+` (https://www.vensim.com/documentation/22150.html). Since
`SMOOTH` is itself a macro, this is direct evidence that a macro invocation can
appear as a sub-expression of a larger RHS, not only as the entire RHS.

The local fixtures, by contrast, only ever use a macro call as the *entire*
RHS of `macro output = MACRONAME(...)`. So the engine's existing fixtures do
not cover the nested-in-expression case, even though Vensim clearly supports it.

### 4.4 Can the same macro be invoked multiple times?

Yes -- "you can use it any number of times"
(https://www.vensim.com/documentation/22150.html). Each use is an independent
expansion (section 5.1). The local fixtures invoke each macro only once;
multiple-invocation behavior is the headline correctness concern for any
implementation and is not directly pinned by a local fixture.

### 4.5 How are multiple outputs accessed at a call site?

When a macro declares output arguments, the call site uses the *same* `:`
separator to bind them. From the Vensim "Using Macros" page
(https://www.vensim.com/documentation/22150.html):

```
total reserve = add3(water reserve lakes, water reserve rivers,
                     water reserve reservoir : min reserve, max reserve)
    ~ m*m*m
    ~ |
is water low flag = if then else(min reserve < LOW WARNING LEVEL, 1, 0)
    ~ DMNL
    ~ |
```

Here:
- The LHS variable `total reserve` receives the macro's *primary* output
  (`add3`).
- The names after the `:` in the call (`min reserve`, `max reserve`) are
  *bound by the caller* to the macro's output arguments (`minval`, `maxval`).
  They become real model variables and can be referenced in subsequent
  equations (`is water low flag = ... min reserve ...`).

So the output-list mechanism at the call site is effectively *multiple-value
return by named binding*: the input list yields the macro-named output assigned
to the LHS, and the colon-separated output list yields additional model
variables named by the caller.

### 4.6 Important parser gap: the `:` separator is not handled today

Both `xmutil` and the engine's current MDL parser handle only the *no-output*
form of the macro header and call. Specifically:

- `xmutil`'s grammar rule for a macro header is
  `VPTT_macro VPTT_symbol '(' exprlist ')'`
  (`src/xmutil/third_party/xmutil/Vensim/VYacc.y:99`), and `exprlist` only
  permits `,` and `;` as separators (`VYacc.y:203-207`). There is no production
  that consumes a `:` between an input list and an output list. `xmutil`
  therefore supports only macros whose sole output is the macro-named variable.

- The engine's MDL parser does the same: `parse_expr_list`
  (`src/simlin-engine/src/mdl/parser.rs:1361-1399`) continues only on `Comma`
  or `Semicolon` and stops at a `Colon`. The macro-header parse then calls
  `expect(TokenKind::RParen, "')'")`
  (`src/simlin-engine/src/mdl/parser.rs:530`), so a header like
  `:MACRO: add3(a, b : c)` would fail with an "expected ')'" error at the `:`.
  (The reader's raw-token scan tracks parentheses and *would* read through a
  `:` -- `src/simlin-engine/src/mdl/reader.rs:211-227` -- but the structured
  parser sub-step rejects it.)

  The engine's lexer *does* have a `Colon` token
  (`src/simlin-engine/src/mdl/lexer.rs:44`), so adding output-list support is a
  parser change, not a lexer change.

Implication: macros with explicit output lists are (a) discouraged by Vensim,
(b) absent from every local fixture, and (c) unparseable by both converters
today. An implementation must decide whether to support them at all; if it
does, parser work is required on both the header and the call-site grammar.

---

## 5. Semantics

This is the section that most directly governs the implementation design.

### 5.1 Per-invocation state (the critical question)

**Each invocation of a macro that contains a stock gets its own independent
stock state, and that state persists for the whole simulation.** This is stated
unambiguously by the XMILE spec and demonstrated by the local `macro_stock`
fixture.

XMILE 1.0 section 4.8.2, verbatim:

> "Note that any stocks that are defined within a macro MUST have their own
> instances for each use of that macro and these instances MUST persist across
> the length of the simulation. That is to say, if this SMOOTH1 function is used
> five times in a model, there must also be five copies of the stock
> Smooth_of_Input, one for each use. Use of flows and auxiliaries do not require
> this..."

And XMILE 1.0 section 3.6.1: macros with variables "are, in fact, independent
models that run to completion each time they are invoked."

The Vensim side corroborates this: the whole point of `SMOOTH`, `DELAY3`, etc.
being macros is that every call site of `SMOOTH(...)` gets its own hidden level
-- two `SMOOTH` calls in a model do not share a smoothing stock. The
hash-delimited expanded variable names (section 6 / section 7) exist precisely
so that each call site's expanded stock is a distinct, uniquely-named model
variable.

The local fixture `test/test-models/tests/macro_stock/` pins the runtime
behavior of a single stock-bearing macro invocation. Its macro is:

```
:MACRO: EXPRESSION MACRO(input, parameter)
EXPRESSION MACRO = INTEG(input, parameter)
:END OF MACRO:
```

invoked as `macro output = EXPRESSION MACRO(macro input, macro parameter)` with
`macro input = 5`, `macro parameter = 1.1`, `TIME STEP = 1`, time 0..10. The
expected `output.tab` shows `macro output` taking values
`1.1, 6.1, 11.1, 16.1, 21.1, ...` -- i.e. a stock initialized to `parameter`
(= 1.1) and integrating `input` (= 5) per unit time. So the macro's
`INTEG(input, parameter)` becomes, at that call site, a genuine stock with
initial value = the bound `parameter` argument and inflow = the bound `input`
argument, and `macro output` reads that stock. (Note for implementers: the
fixture's expected `macro output` at `t=1` is `6.1`, i.e. the Euler update
`1.1 + 5*1` -- the value of the macro at time `t` is the stock value at time
`t`, with the first integration step already applied at `t=1`.)

Flows and auxiliaries inside a macro do *not* need per-instance persistence in
the same sense -- they are recomputed each step like any auxiliary -- but they
*are* still materialized per call site (each call site gets its own copy of
every internal variable; see section 7).

### 5.2 How input arguments bind to the macro's input parameters

- **By position.** The Nth actual argument at the call site binds to the Nth
  formal parameter in the header. Vensim and XMILE both treat the parameter
  list as ordered: XMILE requires "one `<parm>` property for each macro
  parameter and they must appear in the expected calling order (i.e., the order
  of the actual parameters)" (XMILE 1.0 section 4.8).

- **Evaluated in the caller's context.** Because macro expansion materializes
  the macro body as real model variables wired to the call-site arguments, an
  argument expression is evaluated in the *caller's* namespace, on the caller's
  schedule. The XMILE framing -- the macro is "an independent model" with the
  arguments as its inputs -- means each argument is, in effect, an input port
  fed by the caller's expression. For a stockless macro this is just inline
  substitution; for a stock-bearing macro the argument feeds the macro's
  internal structure each time step.

- **Argument arity is fixed per definition.** "Variable numbers of arguments
  are not supported, but the same macro MAY be defined multiple times with a
  different number of arguments" (XMILE 1.0 section 3.6.1). XMILE additionally
  allows `<parm default="...">` so trailing parameters can be omitted at the
  call site (XMILE 1.0 section 4.8); Vensim's `:MACRO:` syntax as documented
  has no default-value mechanism.

### 5.3 How the output value is computed and returned

- The macro's primary output is the value of the variable whose name equals the
  macro name. At a call site `lhs = MACRONAME(args)`, `lhs` takes that value.
  In `xmutil`'s XMILE output the macro's `<eqn>` is literally just the macro
  name (`src/xmutil/.../Xmile/XMILEGenerator.cpp:60-63`), and the macro's
  `<variables>` block contains the equation that actually defines that
  macro-named variable -- the `<eqn>` says "the output is the variable called
  `<macroname>`," and `<variables>` says how that variable is computed.
- For stockless macros the output is a pure function of the (possibly
  expression-valued) arguments at the current time step (`macro_expression`,
  `macro_multi_expression` fixtures).
- For stock-bearing macros the output is whatever the macro-named equation
  computes -- which may itself be the stock (`macro_stock`: the macro-named
  variable *is* the `INTEG`), or may be an auxiliary that reads an internal
  stock (the XMILE `SMOOTH1` example: `<eqn>Smooth_of_Input</eqn>` returns the
  internal stock).
- Additional named outputs (section 4.5) are returned by the caller binding
  them to caller-named variables via the `:` list.

### 5.4 Namespace / locality

Every name defined inside a macro -- parameters, the macro-named variable,
internal auxiliaries, internal stocks, named outputs -- is **local to that
macro**. "The variable names inside of a macro relate to nothing outside the
macro or in other macros"
(https://www.vensim.com/documentation/22150.html). A model variable named
`population` and a macro-internal variable named `population` are unrelated
(https://www.vensim.com/documentation/22150.html). XMILE states the same: "the
names of any variables (including parameter identifiers) defined within a macro
are local to that macro alone and will not conflict with any names within
either the whole-model or other macros" (XMILE 1.0 section 3.6.1).

`xmutil` implements this with an explicit separate symbol table per macro: on
`:MACRO:` it pushes a fresh `SymbolNameSpace` and on `:END OF MACRO:` it pops
back to the model's namespace; the macro *name* alone is registered into the
model's (main) namespace so it can be called
(`src/xmutil/third_party/xmutil/Vensim/VensimParse.cpp:679-694`). The only
escape from the local namespace is the `$`-suffixed model-variable reference
(section 3.1).

### 5.5 Evaluation / initialization order, and nesting

- Because a macro expands into real model variables, those expanded variables
  participate in the model's normal dependency graph and topological sort. They
  are initialized and stepped exactly like any other variable -- the macro
  boundary is not a scheduling barrier once expanded. (The one definition-time
  ordering rule -- "definition before use" -- is a *parse-time* rule, not a
  runtime one; see section 2.4.)

- **Nested macros.** "Macro definitions can be nested, that is the definition
  of a macro can involve another macro. In this case when Vensim expands the
  macro to create the detailed equations it does the expansion recursively"
  (https://www.vensim.com/documentation/22145.html). The local fixture
  `test/test-models/tests/macro_cross_reference/` exercises exactly this:
  `EXPRESSION MACRO`'s body is `EXPRESSION MACRO = SECOND MACRO(input,
  parameter)`, i.e. one macro calls another. Its `output.tab` expects
  `macro output = 4.54545` for `macro input=5, macro parameter=1.1`
  (= 5/1.1, since `SECOND MACRO` divides) -- confirming the nested call expands
  and evaluates correctly. The version-5.8 release notes give the canonical
  use: "a DELAY9 macro can be constructed from 3 DELAY3 calls"
  (https://www.vensim.com/documentation/version_5_8___-___.html). Because the
  body-equation calls another macro, definition order matters here too:
  `SECOND MACRO` is defined before `EXPRESSION MACRO` in the cross-reference
  fixture.

### 5.6 Subscript / array behavior across the macro boundary

Vensim and XMILE diverge here, and the implementation should be explicit about
which model it follows.

- **Vensim.** No subscripts in the macro *definition*. At the call site you
  pass normally-subscripted variables; on expansion, "the created variables
  inherit the Subscripts from the left hand side of the equation" and "each
  equation created during the expansion of the Macro has the same subscripts on
  the left hand side" (https://www.vensim.com/documentation/macros.html). So a
  call `diddle[house] = ... SMOOTH(tadum[house], 20)` expands to an internal
  smoothing stock that is *also* subscripted by `house` -- the array dimension
  rides in from the call-site LHS and is stamped onto every expanded equation.
  The macro body is authored as if scalar. The `$`-escape, by contrast, only
  works for *unsubscripted* model variables
  (https://www.vensim.com/documentation/22145.html).

- **XMILE.** Macros may declare and use arrays internally. The spec's recursive
  `FIND` macro takes a 1-D array parameter `A` and indexes it as `A[i]`, uses
  `SIZE(A)`, etc. (XMILE 1.0 section 4.8.1). So XMILE macros are not restricted
  to scalar bodies.

None of the local `macro_*` fixtures exercises subscripts at all -- they are
all scalar. There is therefore no local ground-truth `output.tab` for arrayed
macro expansion; this case must be validated against Vensim itself if needed.

### 5.7 Recursion

- **Vensim.** The Vensim `:MACRO:` documentation describes *nesting* (one macro
  calling another) and recursive *expansion* of that nesting
  (https://www.vensim.com/documentation/22145.html), and "macro definitions can
  contain calls to other macros previously defined, or built in"
  (https://www.vensim.com/documentation/macros.html). The phrase "previously
  defined" plus the strict "definition before use" rule means *self-reference*
  (a macro calling itself by name) is not expressible in standard Vensim
  `.mdl`, because the macro's own name is not yet defined within its own body.
  The documented Vensim feature is mutual reference among *distinct*,
  previously-defined macros, not classical recursion. (No local fixture or
  Vensim doc page demonstrates a self-recursive `:MACRO:`.)

- **XMILE.** XMILE explicitly supports recursive macros. "Macros can be
  recursive, so a slightly more complicate[d] macro would call itself:
  `FACT(x): IF x <= 1 THEN 1 ELSE x*FACT(x - 1)`" (XMILE 1.0 section 3.6.1).
  Recursion can even be mutual/nested: a recursive macro can call another
  recursive macro (the `FACTSUM` example, XMILE 1.0 section 4.8.1). The spec
  notes its recursion examples all use *tail* recursion deliberately, "because
  implementers are free to map macros into any internal form, especially for
  the sake of efficiency. The use of tail recursion whenever possible allows
  implementers to convert the recursion into a simple loop" (XMILE 1.0 section
  4.8.1).

- **The `recursive_macros` flag.** XMILE requires a model that uses recursive
  macros to declare it. The `<uses_macros>` tag (which itself must be listed
  under `<options>` when the file uses macros at all) has *two REQUIRED
  attributes*: `recursive_macros="true|false"` ("Has macros which are recursive
  (directly or indirectly)") and `option_filters="true|false"` ("Defines option
  filters") (XMILE 1.0 section 2.2.1, and section 4.8: "OPTIONALLY, they can
  also be recursive... In this case, the recursive_macros option must be set to
  true in the `<uses_macros>` tag"). This is the `recursive_macros` field that
  the engine's XMILE serde already models: `Feature::UsesMacros {
  recursive_macros: Option<bool>, option_filters: Option<bool> }`
  (`src/simlin-engine/src/xmile/mod.rs:510-513`). It is purely a declaration /
  capability flag in the file header -- it tells a reader "this file contains
  macros that recurse, so don't assume macro expansion terminates by simple
  inlining." It does not itself change macro semantics; it is a hint that naive
  finite inline-expansion is unsafe.

### 5.8 Units behavior

See section 3.2 for the in-body rule (units may name an input argument, whose
actual unit is substituted at each call). At the call site, the LHS variable
that receives the macro output carries its own units declaration like any
equation (e.g. `total reserve = add3(...) ~ m*m*m ~ |` in the Vensim "Using
Macros" example). XMILE keeps `<units>` on each variable inside the macro's
`<variables>` block (the `macro_stock.xmile` fixture shows `<units>input</units>`
on the macro's stock -- again the "units = name of an input parameter"
convention, carried through verbatim by `xmutil`).

---

## 6. XMILE representation

XMILE v1.0 *does* specify macros (despite a common belief that it does not).
The local copy of the OASIS spec covers them in sections 2.10 (pointer), 3.6 /
3.6.1 / 3.6.2 (rationale), and 4.8 / 4.8.1 / 4.8.2 / 4.8.3 (the encoding).

### 6.1 Placement and the `<macro>` element

> "Macros live outside of all other blocks, at the same level as the `<model>`
> tag, and MAY be the only thing in a file other than its header." (XMILE 1.0
> section 4.8)

A macro is a `<macro>` element. Its **REQUIRED** properties/attributes:

- `name="..."` -- the macro name, a valid XMILE identifier.
- `<eqn>` -- a valid XMILE expression (in a `CDATA` section if needed). For a
  stockless macro this is the macro's actual formula (e.g.
  `<eqn>LN(x)/LN(base)</eqn>`); for a macro with variables this is typically
  just the *name of the variable to return* (e.g. `<eqn>Smooth_of_Input</eqn>`).

**OPTIONAL** properties/attributes (XMILE 1.0 section 4.8):

- `<parm>` -- one per formal parameter, in calling order. Holds the parameter's
  local name (a valid XMILE identifier). May carry a `default="..."` attribute
  (a valid XMILE expression that can refer to earlier parameters); once one
  parameter has a default, every later parameter must too. The spec strongly
  recommends `<parm>` tags appear before `<eqn>`.
- `<format>` -- text describing the proper call format (e.g.
  `LOG(<value>, <base>)`).
- `<doc>` -- text describing the macro's purpose, optionally HTML.
- `<sim_specs>` -- the macro may run with *its own* simulation specs (only
  `<start>`, `<stop>`, `<dt>`, and `method`; all but `method` may be
  expressions referring to parameters). Must only appear together with a
  `<variables>` block. When `<sim_specs>` is present the macro's default DT is
  1 and default integration method is Euler. Absent `<sim_specs>`, the macro
  uses the *invoking model's* DT and method.
- `<variables>` -- a variable block, exactly as for `<model>` (stocks, flows,
  auxes, etc.). Absent for a pure-expression macro.
- `<views>` containing exactly one `<view>` -- only with `<variables>`,
  "exists only to facilitate editing macros."
- `namespace="..."` -- a single XMILE namespace for the macro.

XMILE also says macros MAY include submodels, and MAY be recursive (with the
`recursive_macros` option flag set; section 5.7).

### 6.2 XMILE examples (from the spec)

Stockless / expression macro (XMILE 1.0 section 4.8.1):

```xml
<macro name="LOG">
   <parm>x</parm>
   <parm>base</parm>
   <eqn>LN(x)/LN(base)</eqn>
   <format><![CDATA[LOG(<value>, <base>)]]></format>
   <doc><![CDATA[Finds the base-<base> logarithm of <value>.]]></doc>
</macro>
```

Recursive macro (XMILE 1.0 section 4.8.1):

```xml
<macro name="FACT">
   <parm>x</parm>
   <eqn>IF x <= 1 THEN 1 ELSE FACT(x - 1)</eqn>
</macro>
```

Macro with variables -- a hand-rolled first-order smooth (XMILE 1.0 section
4.8.2):

```xml
<macro name="SMOOTH1">
   <parm>input</parm>
   <parm>averaging_time</parm>
   <parm default="input">initial</parm>
   <eqn>Smooth_of_Input</eqn>
   <variables>
      <stock name="Smooth_of_Input">
         <eqn>initial</eqn>
         <inflow>change_in_smooth</inflow>
      </stock>
      <flow name="change_in_smooth">
         <eqn>(input - Smooth_of_Input)/averaging_time</eqn>
      </flow>
   </variables>
</macro>
```

Macro with variables *and* simulation specs -- an iterative factorial (XMILE
1.0 section 4.8.3):

```xml
<macro name="FACT">
   <parm>x</parm>
   <eqn>Fact</eqn>
   <sim_specs>
     <start>1</start>
     <stop>x</stop>            <!-- stop time is parameterized -->
   </sim_specs>                <!-- DT defaults to 1 -->
   <variables>
      <stock name="Fact">
        <eqn>1</eqn>
        <inflow>change_in_fact</inflow>
      </stock>
      <flow name="change_in_fact">
        <eqn>Fact*TIME</eqn>
      </flow>
   </variables>
</macro>
```

### 6.3 How a macro is invoked in XMILE equations

"In equations, the macro name is used as a function" (XMILE 1.0 section 3.6.1)
-- i.e. `LOG(256, 2)`, `SMOOTH1(demand, 5)`. This matches the Vensim call
syntax and matches what `xmutil` emits: the `macro_expression.xmile` fixture's
model variable is `<eqn>EXPRESSION_MACRO(macro_input, macro_parameter)</eqn>`.
The XMILE spec does not define a `:`-style multiple-output call syntax; XMILE's
optional-parameter story is `<parm default="...">`, not Vensim's output list.

### 6.4 The `<options>` / `<uses_macros>` declaration

A file that uses any macros must list `<uses_macros/>` under the header's
`<options>` tag, and `<uses_macros>` carries the two REQUIRED attributes
`recursive_macros` and `option_filters` (XMILE 1.0 sections 2.2.1, 4.8; see
section 5.7). XMILE also defines a standard macro *library* distribution
mechanism: vendors/organizations can publish versioned macro files (e.g.
`http://systemdynamics.org/xmile/macros/standard-1.0.xml`) and a model pulls
them in via `<includes><include resource="..."/></includes>` inside `<header>`
(XMILE 1.0 sections 2.10, 2.11). "A vendor may create a common library of
macros with specific functionality used by all whole-models produced by that
vendor's software" (XMILE 1.0 section 2.10).

### 6.5 What the engine's XMILE serde currently models

- `xmile::Project` has `#[serde(rename = "macro")] pub macros: Vec<Macro>`
  (`src/simlin-engine/src/xmile/mod.rs:75-76,244`) -- so the `<macro>` elements
  are collected at the project level, as the spec requires.
- `xmile::Macro` is a **stub**: the struct body is literally `// TODO`
  (`src/simlin-engine/src/xmile/mod.rs:334-338`). No `name`, no `<parm>`, no
  `<eqn>`, no `<variables>`. So XMILE macro contents are currently *parsed into
  an empty struct and effectively dropped*, even though the count of macros is
  preserved.
- `Feature::UsesMacros { recursive_macros: Option<bool>, option_filters:
  Option<bool> }` (`src/simlin-engine/src/xmile/mod.rs:510-513`) models the
  header `<uses_macros>` declaration faithfully.

So on the XMILE side, implementing macros means (at minimum) fleshing out
`xmile::Macro` to carry `name`, `parm`s, `eqn`, `variables`, and optionally
`sim_specs`/`views`/`namespace`, and then giving it meaning in the
datamodel/compiler.

---

## 7. xmutil's mapping (Vensim `:MACRO:` -> XMILE `<macro>`)

`xmutil` is the bundled C++ Vensim-to-XMILE converter
(`src/xmutil/third_party/xmutil/`). It is the tool that produced the `.xmile`
and `.stmx` files in the local fixtures, so its mapping is the de facto
"expected" Vensim->XMILE translation the engine's own MDL pipeline is measured
against. Reading its source, the mapping is:

### 7.1 Parse-time (Vensim side)

- The Vensim lexer recognizes `:MACRO:` and `:END OF MACRO:` as dedicated
  tokens `VPTT_macro` / `VPTT_end_of_macro`
  (`src/xmutil/third_party/xmutil/Vensim/VensimLex.cpp:427-438`).
- The grammar rule is
  `macrostart: VPTT_macro { vpyy_macro_start(); } VPTT_symbol '(' exprlist ')'
  { vpyy_macro_expression($3, $5); }` and
  `macroend: VPTT_end_of_macro { vpyy_macro_end(); }`
  (`src/xmutil/third_party/xmutil/Vensim/VYacc.y:98-104`). Note again: the
  header parses `(' exprlist ')'` with **no `:` output-list production** -- so
  `xmutil` supports only the macro-name-is-the-output form.
- `VensimParse::MacroStart()` pushes a *new local `SymbolNameSpace`* for the
  macro's interior; `VensimParse::MacroExpression(name, margs)` constructs a
  `MacroFunction` registered against the *main* (model) namespace -- so the
  macro name is callable from the model -- while its body variables live in the
  local namespace; `VensimParse::MacroEnd()` restores the model namespace
  (`src/xmutil/third_party/xmutil/Vensim/VensimParse.cpp:679-694`).
- A `MacroFunction` (subclass of `Function`) holds: the macro name, an
  `ExpressionList* mArgs` (the formal parameters), a private
  `SymbolNameSpace* pSymbolNameSpace` (the local namespace), and a vector of
  `EqUnitPair` (each body equation + its units)
  (`src/xmutil/third_party/xmutil/Function/Function.h:106-135`). Body equations
  encountered between `:MACRO:` and `:END OF MACRO:` are added to the active
  macro rather than to a model group -- `AddFullEq` checks `!mInMacro` before
  assigning a variable to a group
  (`src/xmutil/third_party/xmutil/Vensim/VensimParse.cpp:181`). The completed
  list of `MacroFunction`s is handed to the `Model`
  (`src/xmutil/third_party/xmutil/Vensim/VensimParse.cpp:387`,
  `Model.h:58-62,127`).
- Because a `MacroFunction` is a `Function`, a call to it inside an equation is
  parsed like any function call -- there is no special call-site syntax.

### 7.2 Generate-time (XMILE side)

The whole-model translation runs first, then "macros are presented as separate
models." For each `MacroFunction`
(`src/xmutil/third_party/xmutil/Xmile/XMILEGenerator.cpp:55-78`):

1. Create a `<macro>` element with `name="<macro name>"` (original Vensim
   casing, spaces preserved -- e.g. `name="EXPRESSION MACRO"`).
2. Emit an `<eqn>` whose text is **just the macro name**. The source comment is
   explicit: "in vensim the equation is always just the name of the macro"
   (`XMILEGenerator.cpp:60-63`). This is XMILE's "return the variable named
   `<macroname>`" convention -- and it works because the macro body always
   contains an equation that defines a variable with the macro's name.
3. For each formal parameter in `mArgs`, emit a `<parm>` whose text is the
   parameter name (`XMILEGenerator.cpp:64-74`). Parameter order is preserved.
4. Emit the macro body by calling `generateModelAsSectors(macro, ..., mf->
   NameSpace(), false)` -- i.e. the macro's local namespace is rendered as a
   `<variables>` block (plus a `<views>`) *inside* the `<macro>` element, using
   the same machinery that renders the main model
   (`XMILEGenerator.cpp:76`). The comment notes it is "not really a sector --
   only a root module here."
5. `<macro>` elements are appended to the document root, as siblings of
   `<model>` (`XMILEGenerator.cpp:77`).

`MacroFunction::ComputableName()` returns `SpaceToUnderBar(name)`
(`src/xmutil/third_party/xmutil/Function/Function.cpp:21-23`), so when the macro
is *called* in a model equation, `xmutil` emits the underscored name -- e.g.
the Vensim call `EXPRESSION MACRO(macro input, macro parameter)` becomes the
XMILE equation `EXPRESSION_MACRO(macro_input, macro_parameter)` (exactly what
`test_macro_expression.xmile` contains). The `<macro name="...">` attribute
keeps the spaced name, but call sites use the underscored form.

### 7.3 Concretely: the local fixtures' Vensim -> XMILE mapping

`test_macro_expression.mdl` ->
`test_macro_expression.xmile` (verified by reading both files):

```
:MACRO: EXPRESSION MACRO(input, parameter)
EXPRESSION MACRO = input * parameter
    ~ input
    ~ tests basic macro containing no stocks and having no output
    |
:END OF MACRO:
```

becomes

```xml
<macro name="EXPRESSION MACRO">
    <eqn>EXPRESSION MACRO</eqn>
    <parm>input</parm>
    <parm>parameter</parm>
    <variables>
        <aux name="EXPRESSION MACRO">
            <doc>	tests basic macro containing no stocks and having no output</doc>
            <eqn>input*parameter</eqn>
            <units>input</units>
        </aux>
    </variables>
    <views>...</views>
</macro>
```

and the model-level call `macro output = EXPRESSION MACRO(macro input, macro
parameter)` becomes `<aux name="macro output"><eqn>EXPRESSION_MACRO(macro_input,
macro_parameter)</eqn></aux>`.

For the stock-bearing `test_macro_stock.mdl`, the macro body
`EXPRESSION MACRO = INTEG(input, parameter)` becomes a `<stock>` plus a
synthetic `<flow>` inside the macro's `<variables>`:

```xml
<macro name="EXPRESSION MACRO">
    <eqn>EXPRESSION MACRO</eqn>
    <parm>input</parm>
    <parm>parameter</parm>
    <variables>
        <flow name="input"/>
        <stock name="EXPRESSION MACRO">
            <doc>	tests basic macro containing a stock but no output</doc>
            <inflow>input</inflow>
            <eqn>parameter</eqn>
            <units>input</units>
        </stock>
    </variables>
    <views>...</views>
</macro>
```

Note `xmutil`'s `INTEG(input, parameter)` -> stock translation reuses the *first
INTEG argument's name* (`input`) as the inflow name -- here `input` is also a
parameter name, so the macro ends up with a `<flow name="input"/>` shadowing the
`<parm>input</parm>`. That is a quirk of `xmutil`'s INTEG handling, not a
property of macros per se, but it is what the fixture contains and an
implementation reading these `.xmile` files must cope with it.

Multiple macros (`test_macro_multi_macros.mdl`) produce multiple sibling
`<macro>` elements. Cross-referencing macros (`macro_cross_reference`) produce
two `<macro>` elements where one macro's `<variables>` equation calls the other
by its underscored name.

### 7.4 What `xmutil` does *not* do

- It does **not** expand/inline macros into the model. It carries the macro
  through to XMILE as a `<macro>` element and leaves call sites as
  `MACRONAME(args)` function calls. Expansion/instantiation is left to whatever
  consumes the XMILE.
- It does **not** support the `:`-separated output list (section 4.6 / 7.1).
- It does **not** carry `<sim_specs>` on the macro (Vensim `:MACRO:` syntax has
  no per-macro sim specs to begin with; that is an XMILE-only feature).

### 7.5 What the engine's *native* MDL parser does today

The engine's pure-Rust MDL parser (`src/simlin-engine/src/mdl/`) -- which is
replacing `xmutil` -- already *parses* macros into a `MacroDef { name, args:
Vec<Expr>, equations: Vec<FullEquation>, loc }`
(`src/simlin-engine/src/mdl/ast.rs:483-493`), assembled by the reader's
`MacroState` machine (`src/simlin-engine/src/mdl/reader.rs:59-70,300-330`). But
the AST-to-datamodel conversion **discards** it: `MdlItem::Macro(_)` is matched
and ignored (`src/simlin-engine/src/mdl/convert/mod.rs:248`, with a test
`test_mdl_group_variables_after_macro` at `convert/mod.rs:616` that only checks
the macro doesn't corrupt *group* membership). The engine's own MDL parser
CLAUDE.md lists this as a known gap: "Macro expansion/inlining (parsing
complete, conversion not implemented). C-LEARN model requires this to
simulate" (`src/simlin-engine/src/mdl/CLAUDE.md`). So the implementation task,
on the Vensim-import side, is *conversion*, not parsing.

---

## 8. Implications for implementation

This section lays out -- *neutrally* -- the evidence bearing on one candidate
representation: **modeling a Vensim macro as a reusable sub-model (module),
instantiated once per call site, with the macro's input arguments wired as
module inputs and its output(s) read by auxiliary variables.** It does not pick
a design; it catalogs which facts make that mapping clean and which make it
hard.

### 8.1 Facts that fit a "module instantiated per call site" mapping well

- **The macro body already *is* a little model.** XMILE says so directly:
  macros with variables "are, in fact, independent models that run to
  completion each time they are invoked" (XMILE 1.0 section 3.6.1), and
  `xmutil` literally renders the macro body with the *same code path* it uses
  for the main model (`generateModelAsSectors`,
  `XMILEGenerator.cpp:76`). A module/sub-model is the natural home for "a
  variable block that looks like a model."

- **Per-invocation state is exactly module-instance semantics.** The XMILE
  requirement that "any stocks that are defined within a macro MUST have their
  own instances for each use of that macro and these instances MUST persist
  across the length of the simulation" (XMILE 1.0 section 4.8.2) is precisely
  what a module instance gives you: each instantiation has its own stock
  storage that persists for the run. The `macro_stock` fixture's expected
  trajectory confirms the runtime behavior a module instance would have to
  reproduce.

- **Locality maps onto a module namespace.** Macro-internal names are local and
  cannot conflict with the model or other macros
  (https://www.vensim.com/documentation/22150.html, XMILE 1.0 section 3.6.1) --
  the same isolation a module/sub-model namespace provides. `xmutil` already
  enforces this with a per-macro symbol table
  (`VensimParse.cpp:679-694`).

- **Positional input binding maps onto module inputs.** Arguments bind by
  position (XMILE 1.0 section 4.8); a module with an ordered input port list is
  the same shape. The macro's primary output (the macro-named variable) is a
  designated output of the module that the call-site LHS reads.

- **Definition/parsing is done.** Both `xmutil` and the engine's native parser
  already capture the macro's name, ordered parameter list, and body equations
  (`MacroDef` / `MacroFunction`). The XMILE serde already collects `<macro>`
  elements into `project.macros`. The hard part left is *instantiation +
  wiring*, not lexing.

- **The `recursive_macros` flag already has a home.** The engine's XMILE serde
  models `Feature::UsesMacros { recursive_macros, option_filters }`
  (`xmile/mod.rs:510-513`), so a design can detect "this file claims recursive
  macros" up front and choose a strategy.

### 8.2 Facts that make the mapping hard or that need explicit decisions

- **Multiple call sites of the same macro.** A module typically has a single
  declared instance with a name; a macro can be called arbitrarily many times
  (https://www.vensim.com/documentation/22150.html), each needing its *own*
  instance with its *own* stock storage and its *own* expanded variable names.
  Vensim's own answer is name-mangling: expanded variables are wrapped in
  `#...#` and prefixed by the call-site LHS variable name -- the post-5.8 form
  is `#lhs>macroname>macrovar#` (e.g. `#smoothed income>smooth#`)
  (https://www.vensim.com/documentation/macros.html,
  https://www.vensim.com/documentation/version_5_8___-___.html). Any
  module-instantiation design has to synthesize one instance (and a unique
  instance name) per call site, and decide how those instances are named and
  surfaced.

- **Macros nested inside larger expressions.** Vensim allows
  `diddle = daddle + SMOOTH(tadum, 20)` -- the macro call is a *sub-expression*
  (https://www.vensim.com/documentation/22150.html). A module instance produces
  a *variable*, not an expression value, so a nested call has to be lifted: the
  call site must be rewritten to introduce a synthesized variable that reads
  the instance's output, and the original expression must reference that
  synthesized variable. (This is the same shape as the engine's existing
  builtins handling, e.g. `PREVIOUS`/`INIT` desugaring through synthesized
  helper auxes -- see `simlin-engine`'s `builtins_visitor.rs` -- so there is
  precedent, but it is non-trivial work.) The local fixtures never exercise
  the nested case, so there is no ground-truth output for it.

- **Expression-valued arguments.** Call-site arguments can be arbitrary
  expressions (`SMOOTH(a + b, t)`; section 4.2). A module input port is fed by
  *a variable* or an expression-input; the design must decide whether to (a)
  pass an expression directly as a module input, or (b) synthesize a helper aux
  per non-trivial argument and wire that. Either is workable; it must be
  chosen. Note the asymmetry: *definition*-header parameters are plain names,
  but *call-site* arguments are full expressions.

- **Multiple outputs (the `:` output list).** Vensim's discouraged-but-legal
  output list (`add3(... : minval, maxval)`) returns extra named variables to
  the caller (section 4.5). Neither converter parses it today (section 4.6). A
  module can have multiple outputs, so the *representation* is fine, but: the
  parser must be extended (header *and* call site), and the call-site binding
  semantics ("the caller names these; they become model variables") must be
  implemented. Given Vensim discourages it and no fixture uses it, an
  implementation could legitimately defer or reject this -- but that is a
  decision to make explicitly.

- **Per-instance stock state vs. how the engine instantiates modules.** The
  design has to confirm that the engine's module mechanism actually gives each
  instance independent, persistent stock storage (the XMILE MUST in section
  4.8.2) and independent initialization. If module instances currently share
  any storage or are deduplicated, that breaks macros with stocks.

- **The `$` model-variable escape.** Macro bodies reach `Time$`, `TIME STEP$`,
  etc. -- references *out* of the macro to the caller's control variables
  (section 3.1), and this is load-bearing for the C-LEARN hero model. A module
  is normally a closed namespace fed only through its input ports. The design
  must decide how a `$` reference becomes an (implicit) module input wired to
  the caller's `time`/`time_step`/etc. -- and remember it is restricted to
  *unsubscripted* model variables.

- **Subscript inheritance from the call-site LHS.** Vensim macro bodies are
  authored as scalar; on expansion every created variable inherits the
  subscripts of the call-site LHS
  (https://www.vensim.com/documentation/macros.html). A module instantiation
  design must replicate "stamp the call-site LHS's dimensions onto the whole
  instance" -- which is a different operation from the usual "module has its
  own fixed dimensions." (XMILE diverges here -- it allows arrays *in* the
  macro body -- so the design must also decide which semantics to honor for
  XMILE-sourced vs. Vensim-sourced macros.) No local fixture covers arrayed
  macros, so this path is unverified by the test corpus.

- **Recursion.** XMILE permits self-recursive and mutually-recursive macros
  (section 5.7); a per-call-site *inline instantiation* strategy does not
  terminate for a genuinely recursive macro. The `recursive_macros` flag is the
  warning sign. Vensim's standard `:MACRO:` syntax cannot express direct
  self-recursion ("definition before use" + the macro name not being defined in
  its own body), so for the *Vensim-import* path this may be a non-issue in
  practice -- but an XMILE-import path, or a strict reading, has to decide what
  to do (support bounded recursion, reject, etc.). XMILE's own hint is that
  *tail*-recursive macros can be turned into loops (section 4.8.1).

- **Definition-order rule.** Vensim's "definition must precede use" is the only
  ordering rule in the language (https://www.vensim.com/documentation/22145.html),
  yet the local `macro_trailing_definition` fixture deliberately violates it and
  is still treated as a valid, simulating model. The implementation must decide
  whether to enforce Vensim's stated rule (and error on trailing definitions)
  or be lenient (collect all macro definitions before resolving any call,
  regardless of order). The engine's parser already collects all top-level
  items before conversion, so leniency is the lower-effort path -- but it is a
  conscious deviation from Vensim's documented contract.

- **Mismatch between XMILE `<macro>` and a module.** An XMILE `<macro>` can
  carry its *own* `<sim_specs>` (its own `start`/`stop`/`dt`/`method`, possibly
  parameterized by arguments) and "run to completion each time it is invoked"
  (XMILE 1.0 sections 3.6.1, 4.8, 4.8.3). That is *not* how a normal sub-model/
  module behaves -- a module steps in lockstep with its parent. A macro with
  its own sim specs is closer to "call a whole nested simulation per time
  step." The local Vensim fixtures never hit this (Vensim `:MACRO:` has no
  per-macro sim specs), but an XMILE-import path or a faithful XMILE-export
  would have to confront it. For the *Vensim* hero use case this can likely be
  ignored; for full XMILE-macro support it cannot.

### 8.3 Summary of where the corpus does and does not give ground truth

| Macro feature | Pinned by a local fixture? |
| --- | --- |
| Stockless macro, single call, scalar | Yes -- `macro_expression`, `macro_multi_expression` |
| Stock-bearing macro, single call, scalar | Yes -- `macro_stock` (full 11-step trajectory) |
| Multiple distinct macros in one file | Yes -- `macro_multi_macros`, `macro_cross_reference` |
| One macro calling another (nesting) | Yes -- `macro_cross_reference` |
| Macro defined *after* use | Yes -- `macro_trailing_definition` (and treated as valid) |
| Same macro invoked at multiple call sites | No |
| Macro call nested inside a larger expression | No |
| Expression-valued call arguments | No (fixtures pass only plain identifiers) |
| `:`-separated output list (multiple outputs) | No (and unparseable today) |
| Subscripted / arrayed macros | No |
| `$` model-variable escape (`Time$`, `TIME STEP$`) | Not in `macro_*` fixtures, but used by the C-LEARN hero model |
| Recursive macro | No |
| Per-macro `<sim_specs>` (XMILE) | No |

The C-LEARN hero model (`test/xmutil_test_models/C-LEARN v77 for Vensim.mdl`)
defines four macros -- `SAMPLE UNTIL` (contains a stock and uses `TIME STEP$`),
`SSHAPE` (stockless, two body equations, *shadows a built-in name*),
`RAMP FROM TO` (stockless, five parameters, six internal auxes, uses `Time$`
and `TIME STEP$`), and `INIT` (stockless, wraps `INITIAL`) -- so it stresses
multi-equation bodies, the `$` escape, a stock-in-macro, and a builtin-shadowing
name, but (as written) it invokes each macro in a relatively simple way. It is
the realistic target; the table above shows which of its features have
*isolated* fixture coverage and which do not.

---

## 9. Open questions / uncertainties not resolved from authoritative sources

- **Lookups inside a macro body.** Neither the Vensim macro pages nor the local
  fixtures explicitly demonstrate a graphical-function/lookup defined inside a
  `:MACRO:` body. XMILE's "all of the syntax of XMILE" implies it is allowed,
  but there is no ground-truth output. Should be tested against Vensim directly
  if needed.

- **Exact Vensim behavior on a trailing macro definition.** The Vensim manual
  says it is a syntax error; the `macro_trailing_definition` fixture (produced
  with "Vensim DSS 6.3E for Mac") nonetheless ships an expected `output.tab`.
  Whether that fixture's `.mdl` actually loaded cleanly in that Vensim build, or
  was hand-constructed, is not determinable from the files alone.

- **Whether direct self-recursion is *ever* accepted by Vensim DSS.** The docs
  describe nesting of *previously defined* macros and recursive *expansion of
  that nesting*, and the strict ordering rule makes literal self-reference
  inexpressible -- but the Vensim documentation never says "self-recursive
  macros are rejected" in so many words. XMILE explicitly allows recursion;
  Vensim's position is, at best, implied.

- **How Vensim resolves a macro name that shadows a built-in at a *call site*.**
  C-LEARN names a macro `SSHAPE`, which is also a Vensim built-in. The engine's
  parser disambiguates the *definition header* (accepts a `Function` token as
  the macro name). What is not pinned by an authoritative source is the
  precedence rule at the *call site* once such a macro is in scope -- does the
  user macro shadow the built-in for the rest of the file? The "definition
  before use" rule and the per-file macro table strongly imply yes, but it is
  not stated outright.

- **Argument-evaluation timing for stock-bearing macros across DT.** The
  XMILE/Vensim model ("the macro is an independent model fed by the
  arguments") and the `macro_stock` fixture together imply the argument
  expressions are evaluated in the caller's context every step and fed into the
  macro's internal integration. The exact ordering relative to the caller's own
  stock updates within a single Euler/RK step is not spelled out in the macro
  documentation pages; it follows from the general XMILE/Vensim integration
  semantics, but a careful implementation should verify it against the
  `macro_stock` numbers and, ideally, a multi-step RK fixture.

- **XMILE `<macro>` with its own `<sim_specs>` -- runtime semantics in
  practice.** The spec defines the *encoding* (sections 4.8, 4.8.3) and says
  such a macro "runs to completion each time it is invoked," but real-world
  XMILE files exercising this are scarce, and the local corpus has none.
