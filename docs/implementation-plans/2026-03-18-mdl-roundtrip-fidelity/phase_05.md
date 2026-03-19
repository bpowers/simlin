# MDL Roundtrip Fidelity Implementation Plan

**Goal:** Improve MDL writer fidelity so Vensim .mdl files roundtrip through Simlin with format preserved.

**Architecture:** Two-layer approach: (1) enrich datamodel with Vensim-specific metadata at parse time, (2) enhance writer to consume that metadata. Changes span datamodel.rs, protobuf schema, serde.rs, MDL parser, and MDL writer.

**Tech Stack:** Rust, protobuf (prost), cargo

**Scope:** 6 phases from original design (phases 1-6)

**Codebase verified:** 2026-03-18

---

## Acceptance Criteria Coverage

This phase implements equation formatting conventions:

### mdl-roundtrip-fidelity.AC4: Equation formatting
- **mdl-roundtrip-fidelity.AC4.1 Success:** Short equations use inline format with spaces around `=` (e.g. `average repayment rate = 0.03`)
- **mdl-roundtrip-fidelity.AC4.2 Success:** Long equations use multiline format with backslash line continuations
- **mdl-roundtrip-fidelity.AC4.4 Success:** Ungrouped variables are ordered deterministically (alphabetically by ident)
- **mdl-roundtrip-fidelity.AC4.5 Success:** Grouped variables retain sector-based ordering

---

<!-- START_SUBCOMPONENT_A (tasks 1-2) -->

<!-- START_TASK_1 -->
### Task 1: Inline format for short equations

**Verifies:** mdl-roundtrip-fidelity.AC4.1

**Files:**
- Modify: `src/simlin-engine/src/mdl/writer.rs`

**Implementation:**

Currently `write_single_entry()` (lines 978-1008) always writes equations in multiline format:
```
name=
\teqn
\t~\tunits
\t~\tcomment
\t|
```

Add an inline path for short equations. After computing the full equation text, check if the combined `name = equation` fits inline (under ~80 chars, no embedded newlines):

```rust
fn write_single_entry(
    buf: &mut String,
    display_name: &str,
    ident: &str,
    eqn: &str,
    dims: &[&str],
    units: &Option<String>,
    doc: &str,
    gf: Option<&GraphicalFunction>,
) {
    let assign_op = if is_data_equation(eqn) { ":=" } else { "=" };

    if gf.is_some() {
        // Lookup tables always use multiline format
        write_multiline_entry(buf, display_name, assign_op, dims, eqn, units, doc, gf);
        return;
    }

    let mdl_eqn = equation_to_mdl(eqn);
    let dim_suffix = if dims.is_empty() {
        String::new()
    } else {
        let dim_strs: Vec<String> = dims.iter().map(|d| format_mdl_ident(d)).collect();
        format!("[{}]", dim_strs.join(","))
    };

    let inline_line = format!("{display_name}{dim_suffix} {assign_op} {mdl_eqn}");

    if inline_line.len() <= 80 && !mdl_eqn.contains('\n') {
        // Inline format: name = equation
        buf.push_str(&inline_line);
        buf.push_str("\r\n");
        write_units_and_comment(buf, units, doc);
    } else {
        // Multiline format: name=\n\tequation (existing behavior)
        write_multiline_entry(buf, display_name, assign_op, dims, &mdl_eqn, units, doc, None);
    }
}
```

Extract the current multiline writing into a helper `write_multiline_entry()` to avoid duplication.

The inline format uses spaces around `=`: `average repayment rate = 0.03`. The multiline format uses no spaces: `Complex Variable Name=`.

**Testing:**
- mdl-roundtrip-fidelity.AC4.1: A variable with equation `0.03` and name `average repayment rate` emits as `average repayment rate = 0.03\r\n\t~\t...\r\n\t~\t...\r\n\t|`

**Verification:**
Run `cargo test -p simlin-engine` -- tests pass.

**Commit:** `engine: inline format for short MDL equations`

<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: Tests for inline formatting

**Verifies:** mdl-roundtrip-fidelity.AC4.1

**Files:**
- Modify: `src/simlin-engine/src/mdl/writer.rs` (test module)

**Testing:**
- Short equation (e.g. `0.03`, name `average repayment rate`): assert inline format `average repayment rate = 0.03`
- Long equation (>80 chars): assert multiline format (name=\n\tequation)
- Equation with graphical function (lookup): assert always multiline
- Data equation: assert uses `:=` operator

**Verification:**
Run `cargo test -p simlin-engine` -- all tests pass.

<!-- END_TASK_2 -->

<!-- END_SUBCOMPONENT_A -->

<!-- START_SUBCOMPONENT_B (tasks 3-4) -->

<!-- START_TASK_3 -->
### Task 3: Backslash line continuations for long equations

**Verifies:** mdl-roundtrip-fidelity.AC4.2

**Files:**
- Modify: `src/simlin-engine/src/mdl/writer.rs`

**Implementation:**

For multiline equations exceeding ~80 characters per line, Vensim uses backslash continuation: `\\\r\n\t\t` (backslash, CRLF, two tabs for continuation indent).

In the multiline equation writing path, after computing the MDL equation text, wrap long lines:

```rust
fn wrap_equation_with_continuations(eqn: &str, max_line_len: usize) -> String {
    // If the equation fits in one line, return as-is
    if eqn.len() <= max_line_len {
        return eqn.to_string();
    }

    let mut result = String::new();
    let mut current_line = String::new();

    // Break at reasonable points: after commas, before operators
    for token in tokenize_for_wrapping(eqn) {
        if current_line.len() + token.len() > max_line_len && !current_line.is_empty() {
            result.push_str(current_line.trim_end());
            result.push_str("\\\r\n\t\t");
            current_line.clear();
        }
        current_line.push_str(&token);
    }
    if !current_line.is_empty() {
        result.push_str(&current_line);
    }
    result
}
```

The `tokenize_for_wrapping` function should split the equation into tokens at natural break points while preserving the full text. Tokens should be: identifiers/numbers (contiguous alphanumeric + spaces), operators (`+`, `-`, `*`, `/`, `^`), parenthesized groups or individual parens, commas with trailing space, and whitespace. Break preferentially after commas and before binary operators. The exact tokenization depends on the equation text format produced by `equation_to_mdl`.

Apply this wrapping in `write_multiline_entry()` when emitting the equation body.

**Testing:**
- mdl-roundtrip-fidelity.AC4.2: A long equation (>80 chars) is wrapped with `\\\r\n\t\t` at reasonable break points
- Short equations are NOT wrapped (no spurious continuations)

**Verification:**
Run `cargo test -p simlin-engine` -- tests pass.

**Commit:** `engine: backslash line continuations for long MDL equations`

<!-- END_TASK_3 -->

<!-- START_TASK_4 -->
### Task 4: Tests for backslash continuations

**Verifies:** mdl-roundtrip-fidelity.AC4.2

**Files:**
- Modify: `src/simlin-engine/src/mdl/writer.rs` (test module)

**Testing:**
- Long equation with multiple terms: assert output contains `\\\r\n\t\t` continuation
- Short equation: assert output does NOT contain backslash continuation
- Equation with commas (e.g., function call with many args): assert break happens after comma

**Verification:**
Run `cargo test -p simlin-engine` -- all tests pass.

<!-- END_TASK_4 -->

<!-- END_SUBCOMPONENT_B -->

<!-- START_SUBCOMPONENT_C (tasks 5-6) -->

<!-- START_TASK_5 -->
### Task 5: Sort ungrouped variables alphabetically

**Verifies:** mdl-roundtrip-fidelity.AC4.4, mdl-roundtrip-fidelity.AC4.5

**Files:**
- Modify: `src/simlin-engine/src/mdl/writer.rs`

**Implementation:**

In `write_equations_section()` (lines 1595-1648), ungrouped variables are currently emitted in `model.variables` iteration order (parser insertion order, lines 1633-1639):

```rust
for var in &model.variables {
    if !grouped_idents.contains(var.get_ident()) {
        write_variable_entry(&mut self.buf, var);
        self.buf.push('\n');
    }
}
```

Change to sort ungrouped variables alphabetically by canonical ident:

```rust
let mut ungrouped: Vec<&Variable> = model.variables.iter()
    .filter(|v| !grouped_idents.contains(v.get_ident()))
    .collect();
ungrouped.sort_by_key(|v| v.get_ident());

for var in ungrouped {
    write_variable_entry(&mut self.buf, var, &display_names);
    self.buf.push_str("\r\n");
}
```

Grouped variables (emitted via `model.groups` iteration, lines 1621-1632) already follow sector-based ordering from the parser, satisfying AC4.5. No change needed for grouped variable ordering.

**Testing:**
- mdl-roundtrip-fidelity.AC4.4: A model with ungrouped variables [c, a, b] emits them in order [a, b, c]
- mdl-roundtrip-fidelity.AC4.5: Grouped variables retain their sector ordering (existing behavior, verify not broken)

**Verification:**
Run `cargo test -p simlin-engine` -- tests pass.

**Commit:** `engine: sort ungrouped MDL variables alphabetically`

<!-- END_TASK_5 -->

<!-- START_TASK_6 -->
### Task 6: Tests for variable ordering

**Verifies:** mdl-roundtrip-fidelity.AC4.4, mdl-roundtrip-fidelity.AC4.5

**Files:**
- Modify: `src/simlin-engine/src/mdl/writer.rs` (test module)

**Testing:**
- Create a model with 3 ungrouped variables with idents [c, a, b]. Write to MDL. Assert they appear in alphabetical order [a, b, c] in the output.
- Create a model with grouped variables in a specific group order. Write to MDL. Assert grouped variables appear in group order, not alphabetical.

**Verification:**
Run `cargo test -p simlin-engine` -- all tests pass.

<!-- END_TASK_6 -->

<!-- END_SUBCOMPONENT_C -->
