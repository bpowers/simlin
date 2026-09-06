// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Vensim MDL file parser.
//!
//! This module provides a pure Rust implementation for parsing Vensim MDL files
//! directly into `crate::datamodel::Project` structures, replacing the
//! C++ xmutil dependency.
//!
//! See `CLAUDE.md` in this directory for implementation context and goals.

pub mod ast;
mod builtins;
mod convert;
mod lexer;
mod normalizer;
mod parser;
mod reader;
mod settings;
pub mod view;
pub mod writer;
mod xmile_compat;

// Public re-exports. Everything else in this module is reached through its
// own submodule path (`crate::mdl::reader::...`), so only the names with an
// out-of-module consumer are re-exported here.
pub use writer::ExportWarning;

use crate::common::{Error, ErrorCode, ErrorKind, Result};
use crate::datamodel::{Project, Variable};

use convert::convert_mdl_with_data;
use writer::MdlWriter;

/// Sentinel equation `"0+0"` used by the MDL converter for a variable with an
/// empty RHS (`MdlEquation::EmptyRhs`) -- a defined-but-unspecified variable
/// that should evaluate to 0 rather than error.
///
/// A bare lookup definition (`MdlEquation::Lookup`) now imports as the canonical
/// lookup-only form -- an EMPTY equation + a graphical function -- NOT this
/// sentinel (issue #606). The sentinel is still ACCEPTED on read as a
/// lookup-only marker (`variable::is_empty_or_sentinel`) for models already
/// serialized with it. The writer recognises both the empty and sentinel forms
/// to emit native Vensim `name(body)` syntax instead of
/// `name = WITH LOOKUP(input, body)`.
pub(crate) const LOOKUP_SENTINEL: &str = "0+0";

/// Convert a Project to Vensim MDL text.
///
/// Thin wrapper over [`project_to_mdl_with_warnings`] that discards the
/// lossiness warnings for the many callers that only need the text. Callers
/// that want to surface degraded exports to the user should use the
/// warnings-returning entry point instead.
pub fn project_to_mdl(project: &Project) -> Result<String> {
    project_to_mdl_with_warnings(project).map(|(text, _warnings)| text)
}

/// Convert a Project to Vensim MDL text, returning any [`ExportWarning`]s for
/// constructs the MDL surface could not represent losslessly (#856).
///
/// # Lossiness contract
///
/// - **Hard errors** (`Err`): structural impossibilities -- a project with more
///   than one non-macro model, an ordinary (non-macro) `Module` variable, or a
///   macro-invocation cluster whose wiring was edited into an unreconstructable
///   state. These would produce corrupt or meaningless `.mdl` and so fail
///   loudly rather than warn.
/// - **Warnings** (non-empty `Vec<ExportWarning>`): the export succeeded but a
///   construct was degraded to the closest representable form. The arms are
///   the `ExportWarning::new` sites in `writer.rs`; by construct:
///   - equations: one that could not be parsed (written as raw text, builtin
///     renames not applied), one using the transpose operator, one calling
///     ROUND (a Simlin extension Vensim does not define) -- each written
///     through as-is with a warning that it will not re-import as meant;
///   - stocks/flows: a dropped `compat.non_negative` flag (changes Vensim sim
///     semantics); a conveyor or queue stock, a conveyor leakage flow, a
///     conveyor inflow placement (spreadflow), a queue overflow outflow (no
///     Vensim construct -- emitted as plain stock/flow);
///   - graphical functions: Discrete interpolation (emitted continuous); an
///     Extrapolate table on an inline `WITH LOOKUP`, one with no `LOOKUP` call
///     site to rewrite as `TABXL`, or one on a per-element arrayed GF (each
///     emitted clamped);
///   - arrays: a one-to-many dimension element mapping (MDL positional
///     notation cannot express it -- emitted as a plain name mapping); an
///     EXCEPT default that could not be reconstructed (dimension membership
///     unavailable, or the default references its own dimensions);
///   - groups: a multi-word group name (the reader truncates the banner at the
///     first whitespace) and a group's documentation (dropped on re-import);
///   - `loop_metadata`: every entry (named loop, described unnamed loop,
///     hidden-loop marker, unnamed LTM pin) -- MDL has no construct for any of
///     it (`warn_dropped_loop_metadata`).
///
///   Each names the affected variable, dimension, group, or loop.
/// - **Silently lossless**: everything else, including a *standalone*
///   Extrapolate lookup table (its call sites are rewritten to `TABXL`, so the
///   kind round-trips) and an EXCEPT default that IS reconstructed (its covered
///   elements are materialized explicitly).
///
/// Warnings are a side channel: they never change the emitted text, so they do
/// not affect the corpus round-trip ratchets.
pub fn project_to_mdl_with_warnings(project: &Project) -> Result<(String, Vec<ExportWarning>)> {
    // MDL has no general multi-model representation, but a macro-marked model
    // is emitted as a `:MACRO:` block (not a separate model), so only the
    // *non-macro* models are subject to the single-model rule. An ordinary
    // multi-model XMILE project is still rejected; a macro-bearing project
    // (one main model plus one or more macro-marked models) is accepted.
    if project
        .models
        .iter()
        .filter(|m| m.macro_spec.is_none())
        .count()
        != 1
    {
        return Err(Error::new(
            ErrorKind::Import,
            ErrorCode::Generic,
            Some("MDL format supports only a single model".to_owned()),
        ));
    }

    let model = main_model(project).expect(MAIN_MODEL_EXPECT);
    for var in &model.variables {
        if let Variable::Module(m) = var {
            // A macro-module instance (Phase 4's materialized multi-output
            // cluster) is reconstructed into the `:` call syntax by the
            // writer, so it passes this coarse gate. This is only a
            // pre-filter on `model_name`: it cannot see whether the
            // cluster's binding auxes / argument wiring are intact (a
            // post-import MCP patch can break them), so the writer itself
            // re-validates and hard-errors on an unreconstructable cluster
            // rather than silently dropping the invocation. An ordinary
            // submodule instance is rejected here outright (a general MDL
            // module-export overhaul is out of scope).
            let is_macro_module = project
                .models
                .iter()
                .any(|candidate| candidate.macro_spec.is_some() && candidate.name == m.model_name);
            if !is_macro_module {
                return Err(Error::new(
                    ErrorKind::Import,
                    ErrorCode::Generic,
                    Some("MDL format does not support Module variables".to_owned()),
                ));
            }
        }
    }

    let writer = MdlWriter::new();
    writer.write_project(project)
}

/// The single non-macro ("main") model of a macro-bearing project, or
/// `None` if there is no non-macro model.
///
/// **Invariant:** every caller must run *after* [`project_to_mdl`]'s reject
/// gate, which rejects any project whose non-macro model count is not
/// exactly 1 (the empty-project `0 != 1` case included). Post-gate the
/// `.find(...)` always matches, so callers `.expect(...)` the result -- a
/// loud, non-indexing assertion of that invariant rather than a panicking
/// index. This is the shared lookup so the gate, `write_project`, and
/// `write_equations_section` all agree on which model is the body.
pub(crate) fn main_model(project: &Project) -> Option<&crate::datamodel::Model> {
    project.models.iter().find(|m| m.macro_spec.is_none())
}

/// `.expect` message for [`main_model`]'s post-reject-gate invariant.
pub(crate) const MAIN_MODEL_EXPECT: &str = "main_model: callers must run after the project_to_mdl reject gate, \
     which guarantees exactly one non-macro model";

/// Parse a Vensim MDL file into a Project.
///
/// This is the main entry point for MDL parsing. It takes the MDL source as a
/// string and converts it to the internal datamodel representation.
pub fn parse_mdl(source: &str) -> Result<Project> {
    parse_mdl_with_data(source, None)
}

/// Parse a Vensim MDL file into a Project with an optional DataProvider
/// for resolving GET DIRECT external data references.
pub fn parse_mdl_with_data(
    source: &str,
    data_provider: Option<&dyn crate::data_provider::DataProvider>,
) -> Result<Project> {
    convert_mdl_with_data(source, data_provider).map_err(|e| {
        Error::new(
            ErrorKind::Import,
            ErrorCode::Generic,
            Some(format!("Failed to parse MDL: {}", e)),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_mdl_simple() {
        let mdl = "x = 5
~ Units
~ A constant |
\\\\\\---///
";
        let result = parse_mdl(mdl);
        assert!(result.is_ok(), "parse_mdl should succeed: {:?}", result);
        let project = result.unwrap();
        assert_eq!(project.models.len(), 1);
    }

    #[test]
    fn test_parse_mdl_stock() {
        let mdl = "Stock = INTEG(inflow - outflow, 100)
~ Units
~ A stock |
inflow = 10
~ Units/Time
~ Inflow rate |
outflow = 5
~ Units/Time
~ Outflow rate |
\\\\\\---///
";
        let result = parse_mdl(mdl);
        assert!(result.is_ok(), "parse_mdl should succeed: {:?}", result);
        let project = result.unwrap();
        assert_eq!(project.models.len(), 1);
        assert!(!project.models[0].variables.is_empty());

        // Verify stock has inflows/outflows
        use crate::datamodel::Variable;
        let stock = project.models[0]
            .variables
            .iter()
            .find(|v| matches!(v, Variable::Stock(_)));
        assert!(stock.is_some(), "Should have a stock variable");
        if let Some(Variable::Stock(s)) = stock {
            assert_eq!(s.inflows, vec!["inflow"]);
            assert_eq!(s.outflows, vec!["outflow"]);
        }
    }

    /// Vensim accepts a space-separated stacked unary minus (`x = - -3`), and
    /// the importer's `xmile_compat` formatter renders a nested negation
    /// unparenthesized -- so the stored datamodel equation is `--3`. The
    /// engine's own equation parser must accept what its importer produces,
    /// otherwise the variable never compiles AND the MDL writer's
    /// unparseable-equation fallback leaks raw XMILE back into the `.mdl`
    /// (#912). Confirms the whole chain: import -> re-parse -> compile ->
    /// simulate to 3.
    #[test]
    fn stacked_unary_minus_imports_reparses_compiles_and_simulates() {
        use crate::datamodel::{Equation, Variable};
        use crate::db::{compile_project_incremental, sync_from_datamodel_incremental};
        use crate::lexer::LexerType;
        use crate::vm::Vm;

        let mdl = "x = - -3
~ Dmnl
~ A stacked unary minus |
\\\\\\---///
";
        let project = parse_mdl(mdl).expect("MDL with `- -3` must import");
        let x = project.models[0]
            .variables
            .iter()
            .find(|v| v.get_ident() == "x")
            .expect("x should be imported");
        let Variable::Aux(aux) = x else {
            panic!("x should be an aux, got {x:?}")
        };
        let Equation::Scalar(eqn) = &aux.equation else {
            panic!("x should have a scalar equation")
        };
        assert_eq!(eqn, "--3", "the importer stores the collapsed form");

        // The invariant every stored datamodel equation must satisfy.
        assert!(
            matches!(
                crate::ast::Expr0::new(eqn, LexerType::Equation),
                Ok(Some(_))
            ),
            "the importer's own output must re-parse: {eqn:?}"
        );

        let mut db = crate::db::SimlinDb::default();
        let sync = sync_from_datamodel_incremental(&mut db, &project, None);
        let model_name = project.models[0].name.clone();
        let compiled =
            compile_project_incremental(&db, sync.project, &model_name, crate::db::LtmOverlay::Off)
                .expect("the imported project must compile");
        let mut vm = Vm::new(compiled).expect("VM creation");
        vm.run_to_end().expect("VM run");
        let series = crate::test_common::collect_results(&vm.into_results());
        let x_series = series.get("x").expect("x should have a saved series");
        assert!(
            x_series.iter().all(|v| (*v - 3.0).abs() < 1e-12),
            "`- -3` must evaluate to 3, got {:?}",
            &x_series[..x_series.len().min(4)]
        );
    }

    /// `mdl::xmile_compat` re-emits an MDL AST *without* parentheses and leans on
    /// the XMILE grammar to re-establish Vensim's grouping (see the long note on
    /// `format_binary_ctx` and GH #914). That laundering is only sound where the
    /// two grammars agree.
    ///
    /// `:AND:` binds tighter than `:OR:` in Vensim and in `mdl::parser`, so
    /// `1 :OR: 0 :AND: 0` is `Or(1, And(0, 0))` = 1. When the XMILE parser gave
    /// `and`/`or` a single shared left-associative level, the flat re-emission
    /// `1 or 0 and 0` re-parsed as `And(Or(1, 0), 0)` = 0 -- the laundering
    /// silently DESTROYED a correct MDL AST. Pins the whole chain end to end.
    #[test]
    fn logical_precedence_survives_the_mdl_to_xmile_laundering() {
        use crate::db::{compile_project_incremental, sync_from_datamodel_incremental};
        use crate::vm::Vm;

        let mdl = "x = 1 :OR: 0 :AND: 0
~ Dmnl
~ |
y = 1 :OR: 1 :AND: 0
~ Dmnl
~ |
z = 0 :OR: 1 :AND: 0
~ Dmnl
~ |
\\\\\\---///
";
        let project = parse_mdl(mdl).expect("MDL with :OR:/:AND: must import");

        let mut db = crate::db::SimlinDb::default();
        let sync = sync_from_datamodel_incremental(&mut db, &project, None);
        let model_name = project.models[0].name.clone();
        let compiled =
            compile_project_incremental(&db, sync.project, &model_name, crate::db::LtmOverlay::Off)
                .expect("the imported project must compile");
        let mut vm = Vm::new(compiled).expect("VM creation");
        vm.run_to_end().expect("VM run");
        let series = crate::test_common::collect_results(&vm.into_results());

        // Vensim's answers. `x`/`y` discriminate (a shared left-associative level
        // yields 0 for both); `z` guards against a grammar that just always says 1.
        for (name, want) in [("x", 1.0), ("y", 1.0), ("z", 0.0)] {
            let got = series.get(name).expect("saved series");
            assert!(
                got.iter().all(|v| (*v - want).abs() < 1e-12),
                "{name} must be {want}, got {:?}",
                &got[..got.len().min(4)]
            );
        }
    }
}
