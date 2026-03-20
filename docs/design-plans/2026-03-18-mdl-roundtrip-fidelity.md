# MDL Roundtrip Fidelity Design

## Summary

This design plan improves the fidelity of Simlin's MDL (Vensim model definition) writer so that a Vensim `.mdl` file can be parsed by Simlin and written back out with its format largely preserved -- a "roundtrip." Today the writer loses significant information: it collapses multiple diagram views into one, discards element sizing and font metadata, rewrites lookup call syntax into a different form, lowercases variable names in equations, and formats all equations identically. The result is an MDL file that, while functionally equivalent, looks substantially different from the original and can confuse Vensim users who open it.

The approach is to thread Vensim-specific metadata through two layers. First, the MDL parser is extended to capture and store format details it currently discards -- element dimensions, font specifications, explicit lookup range bounds -- into new optional "compat" structs on the existing datamodel types. Second, the MDL writer is enhanced to consume that metadata: splitting merged views back into named sections using group boundaries, emitting native lookup call syntax, preserving variable name casing from view elements, and formatting equations with Vensim conventions (inline short equations, backslash continuations for long ones). A dedicated integration test roundtrips a real multi-view model (mark2.mdl) and asserts structural equivalence between input and output.

## Definition of Done

1. **Multi-view MDL output**: `simlin convert --to mdl` produces separate named views from groups/sectors in the datamodel. The MDL writer splits merged views on `ViewElement::Group` boundaries, emitting named views like `*1 housing` and `*2 investments`.

2. **MDL format fidelity**: Written MDL files preserve element sizes/bits, view fonts, lookup range bounds, lookup call syntax (`name(Time)` not `LOOKUP()`), variable name casing in equation definitions, equation ordering by sector, inline formatting for short equations, and backslash line continuations for long lines.

3. **Explicit MDL format test**: A new integration test (registered in Cargo.toml) roundtrips mark2.mdl through `parse_mdl` -> `project_to_mdl` and asserts: 2 named views, correct element counts per view, correct equation formatting patterns, and preserved metadata.

4. **Vensim compatibility**: Roundtripped mark2.mdl is openable by Vensim with diagrams that look the same as the original.

**Out of scope**: Graph definitions (`:GRAPH` blocks), VDF file references and other non-essential settings entries, web app multi-view support.

## Acceptance Criteria

### mdl-roundtrip-fidelity.AC1: Multi-view MDL output
- **mdl-roundtrip-fidelity.AC1.1 Success:** mark2.mdl roundtrip produces exactly 2 views with names `*1 housing` and `*2 investments`
- **mdl-roundtrip-fidelity.AC1.2 Success:** Each view contains the correct elements — every element line from the original view appears in the corresponding output view (unordered set comparison)
- **mdl-roundtrip-fidelity.AC1.3 Success:** Single-view models (no ViewElement::Group markers) produce a single view as before
- **mdl-roundtrip-fidelity.AC1.4 Success:** Each view has its own font specification line matching the original

### mdl-roundtrip-fidelity.AC2: Element metadata preservation
- **mdl-roundtrip-fidelity.AC2.1 Success:** Stock elements preserve original width/height/bits (e.g. `53,32,3,131` not hardcoded `40,20,3,3`)
- **mdl-roundtrip-fidelity.AC2.2 Success:** Aux, flow, cloud, and alias elements preserve original dimensions and bits
- **mdl-roundtrip-fidelity.AC2.3 Success:** Elements without compat data (e.g. from XMILE imports) use hardcoded defaults without error

### mdl-roundtrip-fidelity.AC3: Lookup fidelity
- **mdl-roundtrip-fidelity.AC3.1 Success:** Lookup invocations emit as `table_name ( input )` not `LOOKUP(table_name, input)`
- **mdl-roundtrip-fidelity.AC3.2 Success:** Explicit lookup range bounds are preserved (e.g. `[(0,0)-(300,10)]` not computed `[(0,0.98)-(300,8.29)]`)
- **mdl-roundtrip-fidelity.AC3.3 Success:** Lookups without explicit bounds still compute bounds from data (existing behavior for XMILE-sourced models)

### mdl-roundtrip-fidelity.AC4: Equation formatting
- **mdl-roundtrip-fidelity.AC4.1 Success:** Short equations use inline format with spaces around `=` (e.g. `average repayment rate = 0.03`)
- **mdl-roundtrip-fidelity.AC4.2 Success:** Long equations use multiline format with backslash line continuations
- **mdl-roundtrip-fidelity.AC4.3 Success:** Variable name casing on equation LHS matches original (e.g. `Endogenous Federal Funds Rate=`)
- **mdl-roundtrip-fidelity.AC4.4 Success:** Ungrouped variables are ordered deterministically (alphabetically by ident)
- **mdl-roundtrip-fidelity.AC4.5 Success:** Grouped variables retain sector-based ordering

### mdl-roundtrip-fidelity.AC5: Test coverage
- **mdl-roundtrip-fidelity.AC5.1 Success:** `mdl_roundtrip` test is registered in Cargo.toml and runs with `cargo test`
- **mdl-roundtrip-fidelity.AC5.2 Success:** Format test roundtrips mark2.mdl and asserts per-view element lines match as unordered sets (with only documented normalizations)
- **mdl-roundtrip-fidelity.AC5.3 Success:** Existing roundtrip and simulation tests continue to pass

## Glossary

- **MDL**: Vensim's native plain-text model file format. Contains equations (variable definitions with units and comments) and sketch (diagram layout) sections. Uses `|` as a variable delimiter and `~` to separate equation, units, and comment fields.
- **Roundtrip**: Reading a file, converting through an internal representation, and writing back to the same format. "Roundtrip fidelity" measures how closely the output matches the original.
- **Vensim**: Commercial system dynamics modeling software by Ventana Systems. Defines the MDL format that Simlin interoperates with.
- **Compat struct**: A Simlin pattern where format-specific metadata (no XMILE equivalent) is stored in an optional struct on the relevant datamodel type. Existing example: `datamodel::Compat` on Variable types.
- **ViewElement::Group**: A datamodel variant representing a Vensim "sector" or named group of diagram elements. During MDL parsing, multiple views are merged into one view with Group markers at boundaries; the writer reverses this.
- **Lookup / Graphical function**: A table-defined function mapping input to output via interpolation. Vensim invokes as `table_name(input)`. Simlin normalizes to `LOOKUP(table_name, input)` internally.
- **Bits**: An integer bitmask in MDL sketch element lines encoding Vensim display flags (label visibility, variable type hints). Opaque to Simlin but must be preserved for compatibility.
- **MdlPrintVisitor**: The AST visitor in `writer.rs` that walks equation syntax trees and serializes them to MDL text.
- **Canonical ident**: Simlin's internal identifier form: lowercase with underscores (e.g., `federal_funds_rate`). MDL uses space-separated mixed-case names.
- **mark2.mdl**: A multi-view Vensim test model (views: "housing" and "investments") used as the primary roundtrip fidelity test fixture.
- **Backslash line continuation**: Vensim's convention for wrapping long equation lines: backslash at end of line followed by tab-indented continuation.

## Architecture

The MDL writer (`src/simlin-engine/src/mdl/writer.rs`) is the primary change surface. It currently emits a single view, hardcodes element dimensions and fonts, uses alphabetical equation ordering, and always formats equations as multiline. The parser (`src/simlin-engine/src/mdl/`) captures some metadata that is lost before reaching the datamodel.

The fix has two layers:

**Datamodel enrichment (parse-time):** Extend view element types in `src/simlin-engine/src/datamodel.rs` with optional compat structs carrying Vensim-specific metadata (width, height, bits). Add an optional font field to `StockFlow`. Fix `build_graphical_function` in `src/simlin-engine/src/mdl/convert/variables.rs` to preserve explicit y-range bounds from lookup definitions instead of recomputing from data.

**Writer improvements (write-time):** The writer splits a merged single-view model back into multiple named views using `ViewElement::Group` boundaries. It uses compat metadata when present (falling back to current defaults). It pattern-matches `LOOKUP(name, input)` calls in the equation AST and emits Vensim-native `name(input)` syntax. It uses view element names for equation LHS casing. It formats short equations inline and wraps long ones with backslash continuations.

## Existing Patterns

**Compat struct pattern:** `datamodel::Compat` (line 241) already stores Vensim-specific variable metadata (active_initial, non_negative, visibility, data_source) as an optional struct on Stock, Flow, Aux, and Module. View element compat follows the same pattern.

**View element name for display:** View elements already store original-casing names distinct from canonical idents. The TypeScript drawing code (`src/diagram/drawing/common.ts:76`) uses `displayName(element.name)` for rendering labels. The MDL writer will use this same name field for equation LHS casing.

**AST walking in writer:** `MdlPrintVisitor` in `writer.rs` already walks the equation AST for serialization. The lookup call detection adds a special case to the existing `walk()` method.

**Equation group ordering:** The writer already iterates `model.groups` to emit sector markers and grouped variables. The fix extends this to sort ungrouped variables deterministically.

## Implementation Phases

<!-- START_PHASE_1 -->
### Phase 1: Datamodel Extensions
**Goal:** Add compat metadata fields to view element types and StockFlow so the parser can store Vensim-specific display information.

**Components:**
- `ViewElementCompat` struct in `src/simlin-engine/src/datamodel.rs` with optional `width: f64`, `height: f64`, `bits: u32`
- Optional `compat` field on `view_element::Aux`, `Stock`, `Flow`, `Cloud`, `Alias`
- Optional `font: String` on `StockFlow` (or a `StockFlowCompat` wrapper)
- Protobuf schema updates in `src/simlin-engine/src/project_io.proto` for new fields
- Regenerate protobuf bindings (`pnpm build:gen-protobufs`)

**Dependencies:** None

**Done when:** Project builds, existing tests pass, new types are available for use by parser and writer
<!-- END_PHASE_1 -->

<!-- START_PHASE_2 -->
### Phase 2: Parser Metadata Capture
**Goal:** Thread Vensim-specific metadata from the MDL parser through to the datamodel types added in Phase 1.

**Components:**
- MDL view converter in `src/simlin-engine/src/mdl/view/convert.rs` — pass width/height/bits from parsed `VensimVariable` into `view_element::Aux`, `Stock`, etc. compat fields
- MDL view parser in `src/simlin-engine/src/mdl/view/mod.rs` — capture the `$...` font specification line instead of skipping it, store on `StockFlow.font`
- Lookup bounds fix in `src/simlin-engine/src/mdl/convert/variables.rs` — add `if let Some(y_range) = table.y_range` guard in `build_graphical_function` to preserve explicit y bounds, mirroring existing x_range handling
- Update `test_graphical_function_y_scale_computed_from_data` unit test

**Dependencies:** Phase 1

**Done when:** Parsing mark2.mdl produces a datamodel with: correct per-element width/height/bits in compat, font spec on StockFlow, preserved y-range bounds on lookups. Tests verify these values.
<!-- END_PHASE_2 -->

<!-- START_PHASE_3 -->
### Phase 3: Multi-View Split in Writer
**Goal:** The MDL writer splits a merged single-view model into multiple named views based on `ViewElement::Group` boundaries.

**Components:**
- `write_sketch_section` in `src/simlin-engine/src/mdl/writer.rs` — detect `ViewElement::Group` elements, partition elements into per-view segments, emit each as a separate named view with its own separator and header
- View naming — use Group name directly (e.g. `*1 housing`) since the parser preserves the original Vensim `N name` format
- Font line — emit from StockFlow compat font when present, fall back to hardcoded default
- Element writing (`write_aux_element`, `write_stock_element`, etc.) — use compat width/height/bits when present, fall back to current defaults

**Dependencies:** Phase 2

**Done when:** `project_to_mdl` on a parsed mark2.mdl produces MDL with 2 named views (`*1 housing`, `*2 investments`), correct element dimensions, and correct font lines. Tests verify view structure.
<!-- END_PHASE_3 -->

<!-- START_PHASE_4 -->
### Phase 4: Lookup Syntax and Equation Casing
**Goal:** Fix lookup call syntax and variable name casing in the MDL writer output.

**Components:**
- Lookup call detection in `MdlPrintVisitor::walk()` in `src/simlin-engine/src/mdl/writer.rs` — when visiting a function call where name is `LOOKUP`, 2 args, first arg is a variable reference: emit as `first_arg ( second_arg )` instead of `LOOKUP(first_arg, second_arg)`
- Equation LHS casing — in `write_variable_entry` / `write_single_entry`, look up the view element with matching ident and use its `name` field (original casing) for the equation LHS. Fall back to canonical ident when no view element match exists.

**Dependencies:** Phase 3

**Done when:** Roundtripped mark2.mdl output contains `federal funds rate lookup ( Time )` (not `LOOKUP(...)`) and `Endogenous Federal Funds Rate=` (not lowercased). Tests verify both patterns.
<!-- END_PHASE_4 -->

<!-- START_PHASE_5 -->
### Phase 5: Equation Formatting
**Goal:** Match Vensim equation formatting conventions: inline short equations, backslash line continuations, deterministic ordering.

**Components:**
- Inline formatting in `write_single_entry` — when the full `name = equation` line is under ~80 chars and contains no line breaks, emit on one line with spaces around `=`. Otherwise use current multiline format (no spaces around `=`).
- Line continuation — for multiline equations exceeding ~80 chars, wrap with `\\\n\t\t` at reasonable break points (after commas, before operators)
- Ungrouped variable ordering — sort ungrouped variables alphabetically by ident in `write_equations_section` for deterministic output

**Dependencies:** Phase 4

**Done when:** Roundtripped mark2.mdl output has inline format for short equations (e.g. `average repayment rate = 0.03`), backslash continuations for long equations, and deterministic ordering. Tests verify formatting patterns.
<!-- END_PHASE_5 -->

<!-- START_PHASE_6 -->
### Phase 6: MDL Format Test
**Goal:** Comprehensive integration test that validates MDL output format against the original mark2.mdl.

**Components:**
- Register `mdl_roundtrip` test in `src/simlin-engine/Cargo.toml` — add `[[test]] name = "mdl_roundtrip"` entry
- New test function in `src/simlin-engine/tests/mdl_roundtrip.rs` that roundtrips mark2.mdl through `parse_mdl` -> `project_to_mdl` and asserts:
  - View structure: exactly 2 views named `*1 housing` and `*2 investments`
  - Per-view element matching: extract element lines from each view, compare as unordered sets against original (after CRLF/whitespace normalization). Every element line (arrows, valves, clouds, variables) must match field-by-field. Any normalization applied is documented with a comment explaining why.
  - Font lines match original `Verdana|10` spec
  - Equation section: lookup syntax, lookup bounds, variable casing, inline formatting spot-checks

**Dependencies:** Phase 5

**Done when:** `cargo test -p simlin-engine --test mdl_roundtrip` passes with all assertions. The test validates that the roundtripped MDL output is structurally identical to the original (modulo documented normalizations).
<!-- END_PHASE_6 -->

## Additional Considerations

**Protobuf backwards compatibility:** The datamodel extensions add new optional fields to protobuf messages. Since all new fields are optional with default values, existing serialized instances remain valid. No migration needed.

**Web app impact:** None. The web app reads `views[0]` and ignores compat fields it doesn't know about. The merge behavior during parsing is unchanged — the writer-side split only affects MDL output.
