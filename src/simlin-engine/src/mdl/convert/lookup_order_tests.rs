// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! The importer holds a lookup's points in x order (`points_in_x_order`)
//! whichever way the MDL file writes the lookup.

use crate::datamodel::{Equation as Defined, GraphicalFunction, Variable};
use crate::mdl::ast::{Equation, LookupTable, MdlItem};
use crate::mdl::reader::EquationReader;

/// The equations that carry a table.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Carrier {
    /// `t((x,y),...)`
    Definition,
    /// `t = WITH LOOKUP(input, ((x,y),...))`
    WithLookup,
}

impl Carrier {
    const ALL: [Carrier; 2] = [Carrier::Definition, Carrier::WithLookup];

    /// The ways the parser reads this equation's points: a lookup
    /// definition's arms (`Parser::parse_lookup_def`) and `WITH LOOKUP`'s,
    /// which reads pairs only (`Parser::parse_table_vals`).
    fn syntaxes(self) -> &'static [Syntax] {
        match self {
            Carrier::Definition => &[
                Syntax::Pairs,
                Syntax::RangedPairs,
                Syntax::Legacy,
                Syntax::RangedLegacy,
            ],
            Carrier::WithLookup => &[Syntax::Pairs, Syntax::RangedPairs],
        }
    }
}

/// How a table's points are written.
#[derive(Clone, Copy, Debug)]
enum Syntax {
    /// `(2,6),(0,1),(1,2)`
    Pairs,
    /// `[(0,0)-(2,10)],(2,6),(0,1),(1,2)`
    RangedPairs,
    /// `2,0,1,6,1,2`: the x values, then the y values
    Legacy,
    /// `[(0,0)-(2,10)],2,0,1,6,1,2`
    RangedLegacy,
}

impl Syntax {
    /// The points (2,6), (0,1), (1,2), out of x order, in this syntax.
    fn points(self) -> &'static str {
        match self {
            Syntax::Pairs => "(2,6),(0,1),(1,2)",
            Syntax::RangedPairs => "[(0,0)-(2,10)],(2,6),(0,1),(1,2)",
            Syntax::Legacy => "2,0,1,6,1,2",
            Syntax::RangedLegacy => "[(0,0)-(2,10)],2,0,1,6,1,2",
        }
    }

    fn ranged(self) -> bool {
        match self {
            Syntax::Pairs | Syntax::Legacy => false,
            Syntax::RangedPairs | Syntax::RangedLegacy => true,
        }
    }
}

/// What the left-hand side names, which decides whether the table is the
/// variable's or one element's.
#[derive(Clone, Copy, Debug)]
enum Shape {
    Scalar,
    ApplyToAll,
    PerElement,
}

impl Shape {
    const ALL: [Shape; 3] = [Shape::Scalar, Shape::ApplyToAll, Shape::PerElement];

    fn left_hand_sides(self) -> &'static [&'static str] {
        match self {
            Shape::Scalar => &["t"],
            Shape::ApplyToAll => &["t[D]"],
            Shape::PerElement => &["t[a]", "t[b]"],
        }
    }
}

/// The table `equation` carries and the equation that carries it. Every
/// kind of equation is listed, so a new one does not compile here until it
/// says whether it carries a table.
fn carried_table<'a>(equation: &'a Equation<'_>) -> Option<(Carrier, &'a LookupTable)> {
    match equation {
        Equation::Lookup(_, table) => Some((Carrier::Definition, table)),
        Equation::WithLookup(_, _, table) => Some((Carrier::WithLookup, table)),
        Equation::Regular(..)
        | Equation::EmptyRhs(..)
        | Equation::Implicit(_)
        | Equation::Data(..)
        | Equation::TabbedArray(..)
        | Equation::NumberList(..)
        | Equation::SubscriptDef(..)
        | Equation::Equivalence(..) => None,
    }
}

fn definition(carrier: Carrier, syntax: Syntax, lhs: &str) -> String {
    let points = syntax.points();
    match carrier {
        Carrier::Definition => format!("{lhs}({points})"),
        Carrier::WithLookup => format!("{lhs} = WITH LOOKUP(Time, ({points}))"),
    }
}

/// Every table the importer gave `variable`, the variable's own and each
/// element's.
fn tables(variable: &Variable) -> Vec<&GraphicalFunction> {
    let Variable::Aux(aux) = variable else {
        panic!("{} is imported as an auxiliary", variable.get_ident());
    };
    let mut tables: Vec<&GraphicalFunction> = aux.gf.iter().collect();
    if let Defined::Arrayed(_, elements, _, _) = &aux.equation {
        tables.extend(elements.iter().filter_map(|(_, _, _, gf)| gf.as_ref()));
    }
    tables
}

#[test]
fn a_lookup_listed_out_of_x_order_is_imported_in_x_order_whatever_its_syntax() {
    let mut carriers = Vec::new();
    for carrier in Carrier::ALL {
        for &syntax in carrier.syntaxes() {
            for shape in Shape::ALL {
                let row = format!("{carrier:?}, {syntax:?}, {shape:?}");
                let definitions: Vec<String> = shape
                    .left_hand_sides()
                    .iter()
                    .map(|lhs| format!("{}\n\t~\t\n\t~\t\t|\n\n", definition(carrier, syntax, lhs)))
                    .collect();

                // The row is the equation and table syntax it says it is.
                for text in &definitions {
                    let Some(Ok(MdlItem::Equation(parsed))) = EquationReader::new(text).next()
                    else {
                        panic!("{row}: {text} parses");
                    };
                    let (carried_by, table) =
                        carried_table(&parsed.equation).unwrap_or_else(|| panic!("{row}: a table"));
                    assert_eq!(carried_by, carrier, "{row}");
                    assert_eq!(table.x_range.is_some(), syntax.ranged(), "{row}");
                    carriers.push(carried_by);
                }

                let mdl = format!(
                    "{{UTF-8}}\nD: a, b\n\t~\t\n\t~\t\t|\n\n{}\
                     INITIAL TIME = 0\n\t~\t\n\t~\t\t|\n\n\
                     FINAL TIME = 2\n\t~\t\n\t~\t\t|\n\n\
                     TIME STEP = 1\n\t~\t\n\t~\t\t|\n\n\
                     SAVEPER = TIME STEP\n\t~\t\n\t~\t\t|\n\n\
                     \\\\\\---/// Sketch information - do not modify anything except names\n",
                    definitions.concat()
                );
                let project = crate::compat::open_vensim(&mdl)
                    .unwrap_or_else(|err| panic!("{row}: the model imports: {err:?}"));
                let variable = project.models[0]
                    .variables
                    .iter()
                    .find(|v| v.get_ident() == "t")
                    .unwrap_or_else(|| panic!("{row}: t is imported"));
                let tables = tables(variable);
                assert!(!tables.is_empty(), "{row}: t holds a table");
                for gf in tables {
                    assert_eq!(gf.x_points, Some(vec![0.0, 1.0, 2.0]), "{row}");
                    assert_eq!(gf.y_points, vec![1.0, 2.0, 6.0], "{row}");
                    assert!(
                        crate::variable::parse_table(Some(gf)).is_ok(),
                        "{row}: the compiler reads the table"
                    );
                }
            }
        }
    }
    for carrier in Carrier::ALL {
        assert!(carriers.contains(&carrier), "{carrier:?} has rows");
    }
}
