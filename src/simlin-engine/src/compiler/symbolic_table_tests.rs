// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! The every-variant test of the symbolic opcode table.
//!
//! `symbolic_opcode_table!` is the one statement of each opcode's operands and
//! concrete twin, and `resolve_opcode`, `renumber_opcode`, `gf_run`, `var_ref`
//! and `jump_offset` are derived from it. This module derives its EXPECTATIONS
//! from the same table with its own per-kind rules -- a literal id becomes
//! `LIT + LIT_OFF`, a graphical-function id becomes `GF_REMAP[GF]`, a variable
//! reference becomes the slot the fixture layout assigns it, plain data is
//! copied -- and checks every row against the production functions. Adding a
//! row therefore adds a test case without anyone writing one; what the test
//! cannot check is that a row TYPES its operands truthfully (a temp id declared
//! as a bare `u8` is data to the production functions and to this test
//! alike), which is what review of the table is for.
//!
//! Every sentinel is distinct from every other and from every offset, so a
//! derived function that reads the wrong operand, applies the wrong offset, or
//! moves a value between fields produces a value no row expects.

use std::collections::{HashMap, HashSet};

use super::*;
use crate::bytecode::tests::sentinel;

const LIT: LiteralId = 3;
const LIT_OFF: u16 = 100;
const GF: GraphicalFunctionId = 2;
/// `GF_REMAP[GF]` is 70: not `GF` plus any of the flat offsets, so a flat add
/// applied to a graphical-function id is visible.
const GF_REMAP: [GraphicalFunctionId; 4] = [90, 80, 70, 60];
const MOD: ModuleId = 4;
const MOD_OFF: u16 = 200;
const VIEW: ViewId = 5;
const VIEW_OFF: u16 = 300;
const TEMP: TempId = 6;
const TEMP_OFF: u32 = 40;
const DL: DimListId = 7;
const DL_OFF: u16 = 400;
const PC: PcOffset = -3;
/// The layout slot of the sentinel variable reference's base; the resolved
/// slot, `VAR_BASE + VAR_ELEMENT`, is distinct from every sentinel and every
/// renumbered value.
const VAR_BASE: usize = 20;
const VAR_ELEMENT: usize = 1;

fn var() -> SymVarRef {
    SymVarRef::new(Ident::new("population"), VAR_ELEMENT)
}

fn layout() -> VariableLayout {
    let mut entries = HashMap::new();
    entries.insert(
        "population".to_string(),
        LayoutEntry {
            offset: VAR_BASE,
            size: 3,
        },
    );
    VariableLayout::new(entries, VAR_BASE + 3)
}

/// The input value of an operand of a type. The symbolic-only kind gets its
/// sentinel here; every concrete type's comes from the shared statement in
/// `bytecode::tests`.
macro_rules! sym_sentinel {
    (SymVarRef) => {
        var()
    };
    ($ty:ident) => {
        sentinel!($ty)
    };
}

/// What `renumber_opcode` must make of an operand of a type when every flat
/// base and the remap are the distinct constants above. A resource id moves by
/// exactly its own kind's offset; everything else, the variable reference and
/// the jump offset included, comes through untouched.
macro_rules! renumbered {
    (LiteralId) => {
        LIT + LIT_OFF
    };
    (GraphicalFunctionId) => {
        GF_REMAP[GF as usize]
    };
    (ModuleId) => {
        MOD + MOD_OFF
    };
    (ViewId) => {
        VIEW + VIEW_OFF
    };
    (TempId) => {
        TEMP + TEMP_OFF as TempId
    };
    (DimListId) => {
        DL + DL_OFF
    };
    ($ty:ident) => {
        sym_sentinel!($ty)
    };
}

/// What `resolve_opcode` must hand the concrete twin for an operand of a type:
/// a variable reference becomes its layout slot, everything else is copied.
macro_rules! resolved {
    (SymVarRef) => {
        (VAR_BASE + VAR_ELEMENT) as VariableOffset
    };
    ($ty:ident) => {
        sym_sentinel!($ty)
    };
}

/// `Some($value)` if the operand list holds an operand of the named kind,
/// `None` otherwise -- the test's own statement of which rows carry a
/// graphical-function run, a variable reference or a jump.
macro_rules! expect_operand {
    (GraphicalFunctionId => $value:expr; [GraphicalFunctionId $f:ident $($rest:tt)*]) => {
        Some($value)
    };
    (SymVarRef => $value:expr; [SymVarRef $f:ident $($rest:tt)*]) => {
        Some($value)
    };
    (PcOffset => $value:expr; [PcOffset $f:ident $($rest:tt)*]) => {
        Some($value)
    };
    ($kind:ident => $value:expr; [$ty:ident $f:ident $($rest:tt)*]) => {
        expect_operand!($kind => $value; [$($rest)*])
    };
    ($kind:ident => $value:expr; []) => {
        None
    };
}

/// One row of the table, instantiated with sentinels, beside what each derived
/// function must make of it.
struct Row {
    name: &'static str,
    input: SymbolicOpcode,
    renumbered: SymbolicOpcode,
    resolved: Opcode,
    gf_run: Option<(usize, usize)>,
    var: Option<SymVarRef>,
    jump: Option<PcOffset>,
}

/// The table's rows as `Row`s. The concrete twin is built exactly as the row
/// spells it, from locals holding each symbolic operand's RESOLVED value, so
/// a twin field that names a symbolic operand takes that operand's expected
/// resolution.
macro_rules! sentinel_rows {
    ($(
        $(#[$meta:meta])*
        $name:ident $({ $( $(#[$fmeta:meta])* $field:ident : $ty:ident ),* $(,)? })?
            => $cname:ident $({ $( $cfield:ident $(: $csrc:ident)? ),* $(,)? })? ,
    )*) => {
        // A symbolic-only operand (`LookupDirect::table_count`) has no twin
        // field to flow into, so its resolved local goes unread.
        #[allow(unused_variables)]
        fn rows() -> Vec<Row> {
            vec![$({
                $($( let $field = resolved!($ty); )*)?
                Row {
                    name: stringify!($name),
                    input: SymbolicOpcode::$name $({ $( $field: sym_sentinel!($ty) ),* })?,
                    renumbered: SymbolicOpcode::$name $({ $( $field: renumbered!($ty) ),* })?,
                    resolved: Opcode::$cname $({ $( $cfield: concrete_source!($cfield $(, $csrc)?) ),* })?,
                    gf_run: expect_operand!(
                        GraphicalFunctionId => (GF as usize, sentinel!(TableCount) as usize);
                        [$($( $ty $field )*)?]
                    ),
                    var: expect_operand!(SymVarRef => var(); [$($( $ty $field )*)?]),
                    jump: expect_operand!(PcOffset => PC; [$($( $ty $field )*)?]),
                }
            }),*]
        }
    };
}
symbolic_opcode_table!(sentinel_rows);

/// Every row: the concrete twin is the one the row names with the variable
/// reference resolved and everything else copied (and no two rows share a
/// twin, so resolution is 1:1); renumbering moves each resource id by exactly
/// its kind's base -- the literal, module, view, temp and dim-list ids by
/// their flat offsets, the graphical-function id through the remap -- and
/// leaves every other operand, the variable reference and the jump offset
/// included, byte-identical; and the graphical-function run, the variable
/// reference and the jump offset are reported exactly where the row declares
/// an operand of that kind.
#[test]
fn every_row_resolves_renumbers_and_reports_its_operands() {
    let layout = layout();
    let mut twins: HashSet<&'static str> = HashSet::new();
    for row in rows() {
        let resolved = resolve_opcode(&row.input, &layout)
            .unwrap_or_else(|e| panic!("{}: resolve failed: {e}", row.name));
        assert!(
            resolved == row.resolved,
            "{}: resolved to the wrong concrete opcode or operands ({})",
            row.name,
            resolved.name()
        );
        assert!(
            twins.insert(resolved.name()),
            "{}: a second symbolic opcode resolves to {}",
            row.name,
            resolved.name()
        );

        let renumbered = renumber_opcode(
            &row.input, LIT_OFF, &GF_REMAP, MOD_OFF, VIEW_OFF, TEMP_OFF, DL_OFF,
        )
        .unwrap_or_else(|e| panic!("{}: renumber failed: {e}", row.name));
        assert_eq!(
            renumbered, row.renumbered,
            "{}: renumbered operands",
            row.name
        );

        assert_eq!(row.input.gf_run(), row.gf_run, "{}: gf_run", row.name);
        assert_eq!(
            row.input.var_ref(),
            row.var.as_ref(),
            "{}: var_ref",
            row.name
        );
        assert_eq!(
            row.input.jump_offset(),
            row.jump,
            "{}: jump_offset",
            row.name
        );

        // The mutable jump accessor addresses the same operand the immutable
        // one reports, so a relocation through it is what `jump_offset` then
        // reads.
        let mut relocated = row.input.clone();
        if let Some(back) = relocated.jump_offset_mut() {
            *back = PC - 1;
        }
        assert_eq!(
            relocated.jump_offset(),
            row.jump.map(|_| PC - 1),
            "{}: jump_offset_mut",
            row.name
        );
    }
}
