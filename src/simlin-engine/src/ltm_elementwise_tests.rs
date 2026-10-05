// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! A chain written with offsets is scored by loop analysis as the same chain
//! written as each element's own equation: the same loops, the same loop
//! scores bit for bit, the same diagnostics. Chains are generated over the
//! shapes that tell the two apart -- layers started off a straight line,
//! offsets of one and two, one and two dimensions, an offset under a stdlib
//! call or beside a read of every element, the ends written as exceptions or
//! as a guard on the element's position.

use crate::datamodel::{self, Equation, Project, Variable};
use crate::ltm_loop_scores_test_support::loop_scores;
use crate::test_common::TestProject;

type Arm = (
    String,
    String,
    Option<String>,
    Option<datamodel::GraphicalFunction>,
);

fn set(project: &mut Project, ident: &str, equation: Equation) {
    let at = project.models[0]
        .variables
        .iter()
        .position(|v| v.get_ident() == ident)
        .expect("the variable");
    let mut var = project.models[0].variables[at].clone();
    match &mut var {
        Variable::Aux(a) => a.equation = equation,
        Variable::Flow(f) => f.equation = equation,
        Variable::Stock(s) => s.equation = equation,
        Variable::Module(_) => unreachable!("no module here"),
    }
    project.models[0].variables.replace(at, var);
}

/// How each layer's flux reads the layer `k` before it, `x`.
#[derive(Clone, Copy, Debug)]
enum Read {
    Plain,
    Smoothed,
    Delayed,
    /// Beside a read of every element.
    Shared,
}

impl Read {
    fn of(self, x: &str, own: &str, every: &str) -> String {
        match self {
            Read::Plain => x.to_string(),
            Read::Smoothed => format!("SMTH1({x}, 2)"),
            Read::Delayed => format!("DELAY1({x}, 2)"),
            Read::Shared => format!("{x} * {own} / SUM({every})"),
        }
    }
}

/// How the elements the offset runs off are written.
#[derive(Clone, Copy, Debug)]
enum Ends {
    Exceptions,
    Guarded,
}

/// One generated chain: `layers` elements of `L`, and over `R` too when
/// `regions` is not zero, each layer's flux reading the layer `k` before it
/// (on `R` as well, one before, when `both`).
#[derive(Clone, Copy, Debug)]
struct Chain {
    layers: usize,
    k: usize,
    regions: usize,
    both: bool,
    read: Read,
    ends: Ends,
}

impl Chain {
    fn dims(&self) -> Vec<String> {
        if self.regions > 0 {
            vec!["R".to_string(), "L".to_string()]
        } else {
            vec!["L".to_string()]
        }
    }

    fn base(&self) -> Project {
        let layers: Vec<String> = (1..=self.layers).map(|i| format!("l{i}")).collect();
        let layers: Vec<&str> = layers.iter().map(String::as_str).collect();
        let regions: Vec<String> = (1..=self.regions).map(|i| format!("r{i}")).collect();
        let regions: Vec<&str> = regions.iter().map(String::as_str).collect();
        let over = if self.regions > 0 { "[R, L]" } else { "[L]" };
        let mut project = TestProject::new("chain")
            .with_sim_time(0.0, 8.0, 1.0)
            .named_dimension("L", &layers);
        if self.regions > 0 {
            project = project.named_dimension("R", &regions);
        }
        let start = if self.regions > 0 {
            "10 * L * L + 3 * R"
        } else {
            "10 * L * L + 3"
        };
        project
            .array_stock(&format!("c{over}"), start, &["net"], &[], None)
            .array_flow(&format!("net{over}"), "0", None)
            .array_aux(&format!("flux{over}"), "0")
            .build_datamodel()
    }

    /// Every element as `(key, region, layer)`, 1-based.
    fn elements(&self) -> Vec<(String, usize, usize)> {
        let regions = if self.regions > 0 { self.regions } else { 1 };
        let mut out = Vec::new();
        for r in 1..=regions {
            for l in 1..=self.layers {
                let key = if self.regions > 0 {
                    format!("r{r},l{l}")
                } else {
                    format!("l{l}")
                };
                out.push((key, r, l));
            }
        }
        out
    }

    /// Every element of `c`.
    fn every(&self) -> &'static str {
        if self.regions > 0 { "c[*, *]" } else { "c[*]" }
    }

    fn at(&self, var: &str, r: usize, l: usize) -> String {
        if self.regions > 0 {
            format!("{var}[r{r}, l{l}]")
        } else {
            format!("{var}[l{l}]")
        }
    }

    fn over(&self, var: &str, r: &str, l: &str) -> String {
        if self.regions > 0 {
            format!("{var}[{r}, {l}]")
        } else {
            format!("{var}[{l}]")
        }
    }

    /// Whether the element at `(r, l)` has a layer `k` before it.
    fn reads_back(&self, r: usize, l: usize) -> bool {
        l > self.k && (!self.both || r > 1)
    }

    /// The chain written as each element's own equation.
    fn by_element(&self) -> Project {
        let mut project = self.base();
        let (mut flux, mut net): (Vec<Arm>, Vec<Arm>) = (Vec::new(), Vec::new());
        for (key, r, l) in self.elements() {
            let own = self.at("c", r, l);
            let flux_text = if self.reads_back(r, l) {
                let back = self.at("c", if self.both { r - 1 } else { r }, l - self.k);
                format!(
                    "({} - {own}) * 0.1",
                    self.read.of(&back, &own, self.every())
                )
            } else {
                format!("0 * {own}")
            };
            let net_text = if l + self.k <= self.layers {
                format!(
                    "{} - {}",
                    self.at("flux", r, l),
                    self.at("flux", r, l + self.k)
                )
            } else {
                self.at("flux", r, l)
            };
            flux.push((key.clone(), flux_text, None, None));
            net.push((key, net_text, None, None));
        }
        set(
            &mut project,
            "flux",
            Equation::Arrayed(self.dims(), flux, None, false),
        );
        set(
            &mut project,
            "net",
            Equation::Arrayed(self.dims(), net, None, false),
        );
        project
    }

    /// The chain written with offsets.
    fn by_offset(&self) -> Project {
        let mut project = self.base();
        let k = self.k;
        let back_r = if self.both { "R - 1" } else { "R" };
        let own = self.over("c", "R", "L");
        let back = self.over("c", back_r, &format!("L - {k}"));
        let reads = format!(
            "({} - {own}) * 0.1",
            self.read.of(&back, &own, self.every())
        );
        let next = format!(
            "{} - {}",
            self.over("flux", "R", "L"),
            self.over("flux", "R", &format!("L + {k}"))
        );
        match self.ends {
            Ends::Exceptions => {
                let mut flux_arms = Vec::new();
                let mut net_arms = Vec::new();
                for (key, r, l) in self.elements() {
                    if !self.reads_back(r, l) {
                        flux_arms.push((
                            key.clone(),
                            format!("0 * {}", self.at("c", r, l)),
                            None,
                            None,
                        ));
                    }
                    if l + k > self.layers {
                        net_arms.push((key, self.at("flux", r, l), None, None));
                    }
                }
                set(
                    &mut project,
                    "flux",
                    Equation::Arrayed(self.dims(), flux_arms, Some(reads), true),
                );
                set(
                    &mut project,
                    "net",
                    Equation::Arrayed(self.dims(), net_arms, Some(next), true),
                );
            }
            Ends::Guarded => {
                let guard = if self.both {
                    format!("L <= {k} OR R = 1")
                } else {
                    format!("L <= {k}")
                };
                let flux = format!("IF {guard} THEN 0 * {own} ELSE {reads}");
                let net = format!(
                    "IF L > {} THEN {} ELSE {next}",
                    self.layers - k,
                    self.over("flux", "R", "L")
                );
                set(
                    &mut project,
                    "flux",
                    Equation::ApplyToAll(self.dims(), flux),
                );
                set(&mut project, "net", Equation::ApplyToAll(self.dims(), net));
            }
        }
        project
    }
}

fn chains() -> Vec<Chain> {
    let mut out = Vec::new();
    for read in [Read::Plain, Read::Smoothed, Read::Delayed, Read::Shared] {
        for ends in [Ends::Exceptions, Ends::Guarded] {
            for (layers, k) in [(3, 1), (4, 1), (4, 2)] {
                out.push(Chain {
                    layers,
                    k,
                    regions: 0,
                    both: false,
                    read,
                    ends,
                });
            }
            for both in [false, true] {
                out.push(Chain {
                    layers: 3,
                    k: 1,
                    regions: 2,
                    both,
                    read,
                    ends,
                });
            }
        }
    }
    out
}

#[test]
fn an_offset_chain_is_scored_as_its_elements_equations() {
    let mut differ = Vec::new();
    for chain in chains() {
        let by_element = loop_scores(&chain.by_element())
            .unwrap_or_else(|e| panic!("{chain:?} by element: {e}"));
        let by_offset =
            loop_scores(&chain.by_offset()).unwrap_or_else(|e| panic!("{chain:?} by offset: {e}"));
        assert!(
            !by_element.loops.is_empty() || !by_element.discovered.is_empty(),
            "{chain:?} has loops"
        );
        if by_offset != by_element {
            differ.push(format!(
                "{chain:?}: {} loops scored by element, {} by offset; {} found by element, {} by offset; diagnostics {:?}",
                by_element.loops.len(),
                by_offset.loops.len(),
                by_element.discovered.len(),
                by_offset.discovered.len(),
                by_offset.diagnostics
            ));
        }
    }
    assert_eq!(differ, Vec::<String>::new());
}

/// A bare reducer argument in a per-element slot reads the row the slot's
/// element pins (`slot[a] = SUM(m) * 0.001` sums `m[a, *]` on the run), and
/// is scored as that row written out.
#[test]
fn a_slots_bare_reducer_argument_is_scored_as_the_row_it_reads() {
    let project = |a: &str, b: &str, c: &str| {
        TestProject::new("per_element_slot_bare_argument")
            .with_sim_time(0.0, 4.0, 1.0)
            .named_dimension("d1", &["a", "b", "c"])
            .named_dimension("d2", &["x", "y"])
            .array_with_ranges("init[d1]", vec![("a", "10"), ("b", "20"), ("c", "40")])
            .array_stock("p[d1]", "init[d1]", &["fp"], &[], None)
            .array_aux("q[d2]", "3")
            .array_aux("m[d1, d2]", "p[d1] * q[d2] + 1")
            .array_with_ranges("slot[d1]", vec![("a", a), ("b", b), ("c", c)])
            .scalar_aux("z", "SUM(m) * 0.0001")
            .array_aux("row[d1]", "SUM(m) * 0.004")
            .array_flow("fp[d1]", "row[d1] * 0.1 + slot[d1] * 0.01 + z * 0.1", None)
            .build_datamodel()
    };
    let bare = project("SUM(m) * 0.001", "SUM(m) * 0.002", "SUM(m) * 0.003");
    let rows = project(
        "SUM(m[a, *]) * 0.001",
        "SUM(m[b, *]) * 0.002",
        "SUM(m[c, *]) * 0.003",
    );
    let numbered = project(
        "SUM(m[1, *]) * 0.001",
        "SUM(m[2, *]) * 0.002",
        "SUM(m[3, *]) * 0.003",
    );
    let numbered = loop_scores(&numbered).unwrap();
    let (bare, rows) = (loop_scores(&bare).unwrap(), loop_scores(&rows).unwrap());
    assert!(!rows.loops.is_empty());
    assert_eq!(numbered, rows, "numbered");
    assert_eq!(bare, rows);
}

/// The same for a slot beside an apply-to-all owner of one text.
#[test]
fn a_slot_beside_an_a2a_owner_of_one_text_is_scored_as_the_row_it_reads() {
    let project = |north: &str, south: &str| {
        TestProject::new("dedup_arrayed_owner")
            .with_sim_time(0.0, 8.0, 1.0)
            .named_dimension("region", &["north", "south"])
            .array_with_ranges("init[region]", vec![("north", "100"), ("south", "200")])
            .array_stock("pop[region]", "init[region]", &["growth"], &[], None)
            .scalar_aux("k", "0.01")
            .array_with_ranges("a[region]", vec![("north", north), ("south", south)])
            .array_aux("b[region]", "SUM(pop) * k")
            .array_flow(
                "growth[region]",
                "a[region] * 0.001 + b[region] * 0.1",
                None,
            )
            .build_datamodel()
    };
    let bare = project("SUM(pop) * 0.5", "SUM(pop) * 0.25");
    let rows = project("SUM(pop[north]) * 0.5", "SUM(pop[south]) * 0.25");
    let (bare, rows) = (loop_scores(&bare).unwrap(), loop_scores(&rows).unwrap());
    assert!(!rows.loops.is_empty());
    assert_eq!(bare, rows);
}

/// A read at an element the model computes as it runs (`c[idx[L]]`) is one
/// loop analysis cannot follow element by element: it scores the edge as a
/// read of any element, and says that score is not exact.
#[test]
fn a_loop_through_a_computed_index_says_its_score_is_not_exact() {
    let project = TestProject::new("computed")
        .with_sim_time(0.0, 6.0, 1.0)
        .named_dimension("L", &["l1", "l2", "l3"])
        .array_aux("idx[L]", "IF L = 1 THEN 1 ELSE L - 1")
        .array_stock("c[L]", "10 * L * L", &["net"], &[], None)
        .array_flow("net[L]", "(c[idx[L]] - c[L]) * 0.1", None)
        .build_datamodel();
    let scored = loop_scores(&project).expect("the model runs");
    assert!(
        scored
            .diagnostics
            .iter()
            .any(|d| d.contains("edge c -> net is not exact")
                && d.contains("an element computed as the model runs")),
        "{:?}",
        scored.diagnostics
    );
}
