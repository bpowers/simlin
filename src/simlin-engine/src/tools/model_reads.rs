// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! What each variable of a model reads, and what reads it.

use crate::analysis::model_links;
use crate::db::SourceProject;

/// One read of a model, as [`model_reads`] reports it: `to` reads `from`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelRead {
    pub from: String,
    pub to: String,
    /// The static polarity of the causal link from `from` to `to`
    /// ([`model_links`]), and unknown for a read that is no causal link: a
    /// read made only when the model starts, or a table a `LOOKUP` names.
    pub polarity: crate::ltm::LinkPolarity,
    /// Whether `to` reads `from` only when the model starts: in a stock's
    /// initial value, in an initial-only equation, or as an `INIT` argument.
    /// Such a read sets where `to` starts and never moves it afterwards, so
    /// it is no causal link and no part of a feedback loop.
    pub start_only: bool,
    /// Whether `from` is a table `to` looks up (`LOOKUP(from, x)`) and never
    /// reads as a value: the table shapes `to` and is no causal link.
    pub table: bool,
    /// The stock's or flow's option `to` reads `from` in, when every read of
    /// the pair is in one (a conveyor's transit time, a leak's fraction):
    /// the engine reads it in the special-stock build, outside `to`'s
    /// equation.
    pub option: Option<simlin_base::datamodel::StockOption>,
    /// Whether every read of the pair is in the text of an equation the
    /// compiler cannot read (an unknown builtin, a wrong argument count): the
    /// name is written there, and the equation reads it once it compiles.
    pub unchecked: bool,
}

/// How one read of a pair was made, which the pair's [`ModelRead`] sums.
#[derive(Clone, Copy)]
struct ReadMade {
    at_start: bool,
    table: bool,
    option: Option<simlin_base::datamodel::StockOption>,
    unchecked: bool,
}

impl ReadMade {
    const EQUATION: ReadMade = ReadMade {
        at_start: false,
        table: false,
        option: None,
        unchecked: false,
    };
}

/// Whether a read in `phase` under `lag` is made only when the model starts:
/// every read of an initial-value equation, and a per-step equation's read of
/// the frozen initial snapshot.
fn read_at_start(phase: crate::db::DepPhase, lag: crate::variable::DepLag) -> bool {
    use crate::db::DepPhase;
    use crate::variable::DepLag;
    match (phase, lag) {
        (DepPhase::Init, DepLag::Current | DepLag::Previous | DepLag::Initial) => true,
        (DepPhase::Dt, DepLag::Initial) => true,
        (DepPhase::Dt, DepLag::Current | DepLag::Previous) => false,
    }
}

/// What each variable of a model reads: the one owner of "what does this
/// variable read, and what reads it", for the agent tools' variable records,
/// the `readers` and `reads` citations and the validation battery's reach.
///
/// [`model_links`] is the loops' graph and answers a different question: it
/// holds only what moves a variable from step to step, so a constant a stock
/// starts from, or one read inside `INIT`, has no link out of it. A model
/// still depends on both, and "nothing reads this constant" is false of them.
///
/// The reads come from `variable_direct_dependencies`, in both phases and
/// under every lag:
///
/// - A helper a variable's parse synthesized (a hoisted argument, a
///   `PREVIOUS`/`INIT` capture, a `SMTH1`/`DELAY` instance) is no variable of
///   the model, so what it reads is read by the variable it was made for.
/// - A read through a module instance (`m·x`) is a read of the instance, as
///   a causal link through one is.
/// - A stock reads its flows, and the names in its initial value at the
///   start only.
/// - A table a `LOOKUP` names is read by the caller: the dependency set
///   leaves it out, since a table orders nothing, but the caller does not
///   compile without it.
///
/// A pair is `start_only` when every read of it is made at the start
/// ([`read_at_start`], through an `INIT`-only capture, or as a module
/// instance's input source that only the sub-model's start reads, which the
/// causal edges leave out for that reason). Sorted by reader,
/// then by what is read.
pub fn model_reads(
    db: &dyn crate::db::Db,
    model: crate::db::SourceModel,
    project: SourceProject,
) -> Vec<ModelRead> {
    use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

    use crate::db::{DepRefs, ImplicitVarDeps, SourceVariable};
    use crate::variable::SourceVariableKind;
    use simlin_base::common::{Canonical, Ident};

    /// The reads found so far, for one reader at a time.
    struct Reads<'a> {
        /// (reader, read) -> how every read of the pair was made, summed:
        /// whether all are at the start, all table lookups, all in one
        /// option, all in an unread equation's text.
        pairs: BTreeMap<(String, String), ReadMade>,
        declared: &'a HashMap<String, SourceVariable>,
    }

    impl Reads<'_> {
        /// A read of `from` by `reader`. A name the model does not declare
        /// (an equation's misspelling, a flow a stock lists that is gone) is
        /// the reader's error, which its diagnostics report, not a read of
        /// anything.
        fn note(&mut self, from: &str, reader: &str, at_start: bool, table: bool) {
            self.made(
                from,
                reader,
                ReadMade {
                    at_start,
                    table,
                    ..ReadMade::EQUATION
                },
            );
        }

        fn made(&mut self, from: &str, reader: &str, made: ReadMade) {
            if !self.declared.contains_key(from) {
                return;
            }
            self.pairs
                .entry((reader.to_string(), from.to_string()))
                .and_modify(|sum| {
                    sum.at_start &= made.at_start;
                    sum.table &= made.table;
                    if sum.option != made.option {
                        sum.option = None;
                    }
                    sum.unchecked &= made.unchecked;
                })
                .or_insert(made);
        }

        /// The reads `text` writes, by its references
        /// ([`crate::patch::text_references`], the walk a rename makes), each
        /// made as `made` says. A name alone in a subscript's brackets is a
        /// read only when no dimension has an element of that name, as the
        /// dependency walk reads it; a read through a module instance is a
        /// read of the instance. `false` for a text that does not parse.
        fn text(
            &mut self,
            dimensions: &simlin_base::dimensions::DimensionsContext,
            reader: &str,
            text: &str,
            made: ReadMade,
        ) -> bool {
            let Some(references) = crate::patch::text_references(text) else {
                return false;
            };
            for (site, reference) in references {
                let name = reference.as_str();
                let name = name.strip_prefix("self\u{00B7}").unwrap_or(name);
                let head = name.split('\u{00B7}').next().unwrap_or(name);
                let element = simlin_base::common::CanonicalElementName::from_raw(head);
                if site == crate::patch::Site::Index
                    && dimensions.is_element_of_any_dimension(&element)
                {
                    continue;
                }
                if head != reader {
                    self.made(head, reader, made);
                }
            }
            true
        }

        /// The tables `reader` looks up (a call of a name the model does not
        /// declare is parsed as one too, and [`Reads::note`] drops it).
        fn tables(&mut self, reader: &str, tables: &BTreeSet<Ident<Canonical>>, at_start: bool) {
            for table in tables {
                self.note(table.as_str(), reader, at_start, true);
            }
        }

        /// Every read of `deps`, the reads of the node `node` (`reader`
        /// itself or one of its helpers), as a read by `reader`, descending
        /// into the reader's own helpers, each once (`visited`). `at_start`
        /// says the reads are reached only through one made at the start; a
        /// helper an `INIT` captures is read through `INIT`, whose lag
        /// already says so, and a module instance's input source that only
        /// its sub-model's start reads is in `start_only` (the causal edges'
        /// `start_only_inputs`).
        #[allow(clippy::too_many_arguments)]
        fn record<'h>(
            &mut self,
            helpers: &HashMap<&'h str, &'h ImplicitVarDeps>,
            visited: &mut HashSet<&'h str>,
            start_only: &BTreeSet<(String, String)>,
            reader: &str,
            node: &str,
            deps: &DepRefs,
            at_start: bool,
        ) {
            for dep in deps.iter() {
                let head = dep.target.head().as_str();
                let at_start = at_start
                    || read_at_start(dep.phase, dep.lag)
                    || start_only.contains(&(head.to_string(), node.to_string()));
                // A module instance's read of its own output binds nothing (a
                // Stella import wires those as inputs), as in the causal
                // graph.
                if !dep.target.is_local() && head == reader {
                    continue;
                }
                let Some((&name, &helper)) = helpers.get_key_value(head) else {
                    self.note(head, reader, at_start, false);
                    continue;
                };
                if visited.insert(name) {
                    self.tables(reader, &helper.referenced_tables, at_start);
                    self.record(
                        helpers,
                        visited,
                        start_only,
                        reader,
                        name,
                        &helper.deps,
                        at_start,
                    );
                }
            }
        }
    }

    let empty_inputs = crate::db::ModuleInputSet::empty(db);
    let dimensions = crate::db::project_dimensions_context(db, project);
    let start_only = &crate::db::model_causal_edges(db, model, project).start_only_inputs;
    let mut reads = Reads {
        pairs: BTreeMap::new(),
        declared: model.variables(db),
    };
    for (name, source_var) in model.variables(db).iter() {
        // A stock's or a flow's options are expressions the special-stock
        // build reads, outside the dependency set.
        for (role, text) in source_var.compat(db).expression_texts() {
            if let simlin_base::datamodel::ExpressionRole::Option(option) = role {
                reads.text(
                    dimensions,
                    name,
                    text,
                    ReadMade {
                        option: Some(option),
                        ..ReadMade::EQUATION
                    },
                );
            }
        }
        // An equation the compiler cannot read has no dependency set; the
        // names its text writes are what it reads once it compiles.
        if !equation_is_read(db, *source_var, project, dimensions) {
            for (_, text) in source_var.equation(db).expression_texts() {
                reads.text(
                    dimensions,
                    name,
                    text,
                    ReadMade {
                        unchecked: true,
                        ..ReadMade::EQUATION
                    },
                );
            }
        }
        let deps = crate::db::variable_direct_dependencies(db, *source_var, project, empty_inputs);
        let helpers: HashMap<&str, &ImplicitVarDeps> = deps
            .implicit_vars
            .iter()
            .map(|helper| (helper.name.as_str(), helper))
            .collect();
        // A stock's equation is its initial value; what moves it is its flows.
        let is_stock = source_var.kind(db) == SourceVariableKind::Stock;
        if is_stock {
            for flow in source_var
                .inflows(db)
                .iter()
                .chain(source_var.outflows(db).iter())
            {
                reads.note(&simlin_base::canonicalize(flow), name, false, false);
            }
        }
        let mut visited = HashSet::new();
        reads.tables(name, &deps.referenced_tables, is_stock);
        reads.record(
            &helpers,
            &mut visited,
            start_only,
            name,
            name,
            &deps.deps,
            is_stock,
        );
    }

    let polarities: HashMap<(String, String), crate::ltm::LinkPolarity> =
        model_links(db, model, project, None, false)
            .into_iter()
            .map(|link| ((link.to, link.from), link.polarity))
            .collect();
    reads
        .pairs
        .into_iter()
        .map(|((to, from), made)| {
            let polarity = polarities
                .get(&(to.clone(), from.clone()))
                .copied()
                .unwrap_or(crate::ltm::LinkPolarity::Unknown);
            ModelRead {
                from,
                to,
                polarity,
                start_only: made.at_start,
                table: made.table,
                option: made.option,
                unchecked: made.unchecked,
            }
        })
        .collect()
}

/// Whether the compiler reads `var`'s equation: it parses, and its typed tree
/// is built (`ast::typed_ast`, what the dependency walk classifies). One that
/// is not read has no dependency set ([`model_reads`] reads its text instead).
fn equation_is_read(
    db: &dyn crate::db::Db,
    var: crate::db::SourceVariable,
    project: SourceProject,
    dimensions: &simlin_base::dimensions::DimensionsContext,
) -> bool {
    if var.kind(db) == crate::variable::SourceVariableKind::Module {
        return true;
    }
    let parsed = &crate::db::parse_source_variable(db, var, project).variable;
    let typed = |ast: Option<&simlin_base::ast::Ast<simlin_base::ast::Expr0>>| {
        ast.is_none_or(|ast| simlin_base::ast::typed_ast(ast, dimensions).is_ok())
    };
    let written = var
        .equation(db)
        .expression_texts()
        .iter()
        .any(|(_, text)| !text.trim().is_empty());
    let parsed_any = parsed.ast().is_some() || parsed.init_ast().is_some();
    (parsed_any || !written) && typed(parsed.ast()) && typed(parsed.init_ast())
}

/// The variables of a model whose reads no reader can know: an expression
/// text of theirs that does not even parse. Sorted. A claim that a variable
/// has no reader, or no other, cannot hold over such a model.
pub fn model_unread_equations(
    db: &dyn crate::db::Db,
    model: crate::db::SourceModel,
) -> Vec<String> {
    let mut unread: Vec<String> = model
        .variables(db)
        .iter()
        .filter(|(_, var)| {
            let equation = var.equation(db).expression_texts();
            let compat = var.compat(db).expression_texts();
            equation
                .iter()
                .chain(compat.iter())
                .any(|(_, text)| crate::patch::text_references(text).is_none())
        })
        .map(|(name, _)| name.clone())
        .collect();
    unread.sort();
    unread
}

#[cfg(test)]
#[path = "model_reads_tests.rs"]
mod model_reads_tests;
