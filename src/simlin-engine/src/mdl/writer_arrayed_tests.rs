// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! An arrayed variable saved as MDL reads back over its own dimensions. Each
//! test imports an MDL file, saves it, and asks `check_save` what the save
//! changes, so the variable is the shape the reader produces.

use crate::datamodel::{self, Equation};
use crate::mdl::{parse_mdl, project_to_mdl};
use crate::save_check::{SaveFormat, check_save};

const CONTROL: &str =
    "INITIAL TIME = 0 ~~|\nFINAL TIME = 2 ~~|\nTIME STEP = 1 ~~|\nSAVEPER = TIME STEP ~~|\n";

/// What saving the project `source` reads as changes, and the save.
fn changes(source: &str) -> (Vec<String>, String) {
    let project = parse_mdl(&format!("{source}{CONTROL}")).expect("the source reads");
    let save = project_to_mdl(&project).expect("the model writes");
    let changes = check_save(&project, SaveFormat::Mdl)
        .expect("the check runs")
        .into_iter()
        .map(|c| c.reason)
        .collect();
    (changes, save.replace("\r\n", "\n"))
}

#[test]
fn an_array_of_numbers_is_written_as_the_list_that_names_its_dimensions() {
    // Over a subrange, over one of two dimensions of the same elements, and
    // over three dimensions, which Vensim writes as one list per element of
    // the leading one.
    let rows = [
        (
            "DimA: A1, A2, A3 ~~|\nSubA: A2, A3 ~~|\nu[SubA] = 1, 2 ~~|\n",
            "u[SubA]=\n\t1, 2",
        ),
        (
            "DimA: A1, A2 ~~|\nDimB: A1, A2 ~~|\nx[DimB] = 1, 2 ~~|\n",
            "x[DimB]=\n\t1, 2",
        ),
        (
            "c: E, F ~~|\nd: A, B ~~|\nd1: A, B ~~|\n\
             m[E, d, d1] = 3, 3.5; 3.2, 3.6; ~~|\nm[F, d, d1] = 0.6, -0.7; -0.2, 0.4; ~~|\n",
            "m[E,d,d1]=\n\t3, 3.5;3.2, 3.6;",
        ),
    ];
    for (source, written) in rows {
        let (changes, save) = changes(source);
        assert!(save.contains(written), "{save}");
        assert_eq!(changes, Vec::<String>::new(), "{save}");
    }
}

#[test]
fn elements_that_agree_across_a_dimension_are_written_under_its_name() {
    // DimB shares its elements with DimC, declared first, so only a
    // left-hand side naming DimB says the variable is over it; and an
    // element equation that reads a subrange is written under the subrange,
    // where Vensim reads its name.
    let rows = [
        (
            "DimA: A1, A2 ~~|\nDimC: B1, B2 ~~|\nDimB: B1, B2 ~~|\n\
             x[A1, DimB] = Time ~~|\nx[A2, DimB] = 2 * Time ~~|\n",
            "x[A1,DimB]=",
        ),
        (
            "DimA: A1, A2, A3 ~~|\nSubA: A2, A3 ~~|\nDimB: B1, B2 ~~|\n\
             input[DimA] = Time ~~|\ns[SubA, B1] = SMOOTH(input[SubA], 2) ~~|\n",
            "s[SubA,B1]=\n\tSMOOTH(input[SubA], 2)",
        ),
    ];
    for (source, written) in rows {
        let (changes, save) = changes(source);
        assert!(save.contains(written), "{save}");
        assert_eq!(changes, Vec::<String>::new(), "{save}");
    }
}

#[test]
fn a_dimension_sharing_its_elements_is_kept_where_an_equation_can_name_it() {
    // The reader takes a variable over the dimension its left-hand sides
    // name (Simlin's rule); a save names it where the elements' equations
    // allow, and otherwise the elements alone read back over the first
    // declared dimension holding them, a limit of the format that
    // `check_save` reports. Rows: equations that differ, over a second
    // dimension and over a subrange with every element of its parent; a
    // number list and equations that agree across the dimension.
    let rows = [
        (
            "DimA: A1, A2 ~~|\nDimB: A1, A2 ~~|\ny[DimB] = Time ~~|\ny[A2] = 2 * Time ~~|\n",
            "DimB",
            "DimA",
        ),
        (
            "DimA: A1, A2 ~~|\nSubA: A1, A2 ~~|\ny[SubA] = Time ~~|\ny[A2] = 2 * Time ~~|\n",
            "SubA",
            "DimA",
        ),
        (
            "DimA: A1, A2 ~~|\nDimB: A1, A2 ~~|\ny[DimB] = 1, 2 ~~|\n",
            "DimB",
            "DimB",
        ),
        (
            "DimA: A1, A2 ~~|\nDimB: A1, A2 ~~|\nDimC: C1, C2 ~~|\n\
             y[DimB, C1] = Time ~~|\ny[DimB, C2] = 2 * Time ~~|\n",
            "DimB",
            "DimB",
        ),
    ];
    for (source, imported, read_back_over) in rows {
        let (project, save, _) = save_of(source);
        let dims = |p: &datamodel::Project| {
            p.models[0]
                .variables
                .iter()
                .find(|v| v.get_ident() == "y")
                .and_then(|v| match v.get_equation() {
                    Some(Equation::Arrayed(dims, ..)) | Some(Equation::ApplyToAll(dims, _)) => {
                        dims.first().cloned()
                    }
                    _ => None,
                })
        };
        assert_eq!(dims(&project).as_deref(), Some(imported), "{source}");
        let back = parse_mdl(&save).expect("the save reads");
        assert_eq!(dims(&back).as_deref(), Some(read_back_over), "{save}");
        let reported = check_save(&project, SaveFormat::Mdl)
            .expect("the check runs")
            .iter()
            .any(|c| c.reason.contains("'y'"));
        assert_eq!(reported, imported != read_back_over, "{source}");
    }
}

#[test]
fn an_element_is_keyed_canonically_and_written_as_its_dimension_spells_it() {
    // Every reader of a datamodel keys an element canonically
    // (`CanonicalElementName::from_subscript`); the spelling is the
    // dimension's, and the save writes it.
    let source = "Fuel: OilGas, Coal ~~|\nx[oilgas] = 1 ~~|\nx[COAL] = Time ~~|\n";
    let project = parse_mdl(&format!("{source}{CONTROL}")).expect("the source reads");
    let x = project.models[0]
        .variables
        .iter()
        .find(|v| v.get_ident() == "x")
        .expect("x reads");
    let Some(Equation::Arrayed(_, slots, _, _)) = x.get_equation() else {
        panic!("x is arrayed: {:?}", x.get_equation());
    };
    let keys: Vec<&str> = slots.iter().map(|(key, _, _, _)| key.as_str()).collect();
    assert_eq!(keys, ["coal", "oilgas"]);
    let (changes, save) = changes(source);
    assert!(
        save.contains("x[OilGas]=") && save.contains("x[Coal]="),
        "{save}"
    );
    assert_eq!(changes, Vec::<String>::new(), "{save}");
}

/// The save of `source` and its warnings.
fn save_of(source: &str) -> (datamodel::Project, String, Vec<String>) {
    let project = parse_mdl(&format!("{source}{CONTROL}")).expect("the source reads");
    let (save, warnings) =
        crate::mdl::project_to_mdl_with_warnings(&project).expect("the model writes");
    let warnings = warnings.into_iter().map(|w| w.message).collect();
    (project, save.replace("\r\n", "\n"), warnings)
}

#[test]
fn an_except_default_naming_a_subrange_is_written_over_that_subrange() {
    // Each default names a subrange of one of the variable's dimensions, as
    // the corpus files `except_subranges` and `except_multiple` do; the
    // :EXCEPT: equation is written over the subrange, which holds every
    // range its right-hand side names, and the save keeps the variable.
    let rows = [
        (
            "REGION: A, B, C ~~|\nSEC ALL: X, Y, W, Z ~~|\nSEC A: X, Z ~~|\nSEC B: Y, W ~~|\n\
             origin[REGION, SEC A] = 1 ~~|\n\
             my var[REGION, SEC A] :EXCEPT: [C, SEC A] = origin[REGION, SEC A] ~~|\n\
             my var[REGION, SEC B] = 2 ~~|\nmy var[C, SEC A] = 3 ~~|\n",
            "my var[REGION,SEC A] :EXCEPT:",
        ),
        (
            "dim1: (sub1-sub4) ~~|\ndim1up: (sub1-sub2) ~~|\ndim2: d1, d2 ~~|\n\
             v[dim1] = 1 ~~|\nw[dim1] = 2 ~~|\n\
             e[dim1, dim2] :EXCEPT: [dim1up, d1] = v[dim1] ~~|\n\
             e[dim1up, dim2] :EXCEPT: [dim1up, d2] = w[dim1up] ~~|\n\
             e[dim1up, d2] = 3 * v[dim1up] ~~|\n",
            "e[dim1up,dim2] :EXCEPT:",
        ),
    ];
    for (source, written) in rows {
        let (project, save, warnings) = save_of(source);
        assert!(save.contains(written), "{warnings:?}\n{save}");
        assert_eq!(
            crate::mdl::subscript_rule::ranges_not_on_the_left(&save, &project),
            Vec::<String>::new(),
            "{save}"
        );
        let back = parse_mdl(&save).expect("the save reads");
        assert_eq!(
            crate::mdl::save_roundtrip_tests::first_difference(
                &crate::mdl::save_roundtrip_tests::normalized(&project),
                &crate::mdl::save_roundtrip_tests::normalized(&back)
            ),
            None,
            "{save}"
        );
    }
}

#[test]
fn an_except_equation_that_would_define_nothing_is_written_as_element_equations() {
    // Every element differs from the default, so an :EXCEPT: equation would
    // except every element it names. The elements are written one by one,
    // and the default, which no element takes, is not kept, with a warning.
    let (project, save, warnings) = save_of(
        "DimA: A1, A2, A3 ~~|\ny[DimA] :EXCEPT: [A1], [A2], [A3] = 5 ~~|\n\
         y[A1] = 1 ~~|\ny[A2] = 2 ~~|\ny[A3] = 3 ~~|\n",
    );
    assert!(
        matches!(
            project.models[0]
                .variables
                .iter()
                .find(|v| v.get_ident() == "y")
                .and_then(|v| v.get_equation()),
            Some(Equation::Arrayed(_, _, Some(_), true))
        ),
        "the reader keeps the default"
    );
    assert!(!save.contains(":EXCEPT:"), "{save}");
    assert!(
        warnings
            .iter()
            .any(|w| w.contains("'y'") && w.contains("default is not kept")),
        "{warnings:?}"
    );
    let back = parse_mdl(&save).expect("the save reads");
    let y = back.models[0]
        .variables
        .iter()
        .find(|v| v.get_ident() == "y")
        .and_then(|v| v.get_equation().cloned());
    let Some(Equation::Arrayed(dims, slots, None, false)) = y else {
        panic!("y reads back without its default: {y:?}");
    };
    assert_eq!(dims, ["DimA"]);
    let slots: Vec<(&str, &str)> = slots.iter().map(|s| (s.0.as_str(), s.1.as_str())).collect();
    assert_eq!(slots, [("a1", "1"), ("a2", "2"), ("a3", "3")]);
}

#[test]
fn an_except_default_calling_a_table_or_a_macro_is_written_as_it_was() {
    // The read-back that admits an :EXCEPT: equation reads the names the
    // default uses as the model has them: a call of the model's table is a
    // lookup and one of its macro a macro call, as in the real read.
    let rows = [
        (
            "DimA: A1, A2, A3 ~~|\ntbl((0,0),(2,20)) ~~|\n\
             x[DimA] :EXCEPT: [A1] = tbl(Time) ~~|\nx[A1] = 5 ~~|\n",
            "x[DimA] :EXCEPT: [A1]=\n\ttbl ( Time )",
        ),
        (
            ":MACRO: DBL(a)\nDBL = a * 2 ~~|\n:END OF MACRO:\nDimA: A1, A2, A3 ~~|\n\
             y[DimA] = 1, 2, 3 ~~|\nx[DimA] :EXCEPT: [A1] = DBL(y[DimA]) ~~|\nx[A1] = 5 ~~|\n",
            "x[DimA] :EXCEPT: [A1]=\n\tDBL(y[DimA])",
        ),
    ];
    for (source, written) in rows {
        let (project, save, warnings) = save_of(source);
        assert!(save.contains(written), "{warnings:?}\n{save}");
        let back = parse_mdl(&save).expect("the save reads");
        assert_eq!(
            crate::mdl::save_roundtrip_tests::first_difference(
                &crate::mdl::save_roundtrip_tests::normalized(&project),
                &crate::mdl::save_roundtrip_tests::normalized(&back)
            ),
            None,
            "{save}"
        );
    }
}

#[test]
fn an_except_default_naming_a_mapped_range_is_written_as_element_equations() {
    // A limit of the writer: the default names DimA, mapped onto the
    // variable's DimB, and no element's equation is the default with DimB's
    // element put in, so no :EXCEPT: equation defines an element. The
    // elements are written one by one, with a warning, and the model's
    // results are kept.
    let source = "DimA: A1, A2 -> DimB ~~|\nDimB: B1, B2 ~~|\ny[DimA] = 1, 2 ~~|\n\
                  x[DimB] :EXCEPT: [B1] = y[DimA] * 2 ~~|\nx[B1] = 0 ~~|\n";
    let (project, save, warnings) = save_of(source);
    assert!(!save.contains(":EXCEPT:"), "{save}");
    assert!(
        warnings
            .iter()
            .any(|w| w.contains("'x'") && w.contains("default is not kept")),
        "{warnings:?}"
    );
    let results: Vec<String> = check_save(&project, SaveFormat::Mdl)
        .expect("the check runs")
        .into_iter()
        .filter(|c| c.kind == crate::save_check::ChangeKind::Results)
        .map(|c| c.reason)
        .collect();
    assert_eq!(results, Vec::<String>::new());
}
