// Copyright 2020 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

#![allow(dead_code)]

pub type LiteralId = u16;
pub type ModuleId = u16;
pub type Register = u8;
pub type VariableOffset = u16;
pub type ModuleInputOffset = u16;
pub type GraphicalFunctionId = u8;

#[derive(Clone, Debug)]
pub(crate) enum BuiltinId {
    Abs,
    Arccos,
    Arcsin,
    Arctan,
    Cos,
    Exp,
    Inf,
    Int,
    Ln,
    Log10,
    Max,
    Min,
    Pi,
    Pulse,
    SafeDiv,
    Sin,
    Sqrt,
    Tan,
}

#[derive(Clone, Debug)]
pub(crate) enum Opcode {
    Mov {
        dst: Register,
        src: Register,
    },
    Add {
        dest: Register,
        l: Register,
        r: Register,
    },
    Sub {
        dest: Register,
        l: Register,
        r: Register,
    },
    Exp {
        dest: Register,
        l: Register,
        r: Register,
    },
    Mul {
        dest: Register,
        l: Register,
        r: Register,
    },
    Div {
        dest: Register,
        l: Register,
        r: Register,
    },
    Mod {
        dest: Register,
        l: Register,
        r: Register,
    },
    Gt {
        dest: Register,
        l: Register,
        r: Register,
    },
    Gte {
        dest: Register,
        l: Register,
        r: Register,
    },
    Eq {
        dest: Register,
        l: Register,
        r: Register,
    },
    And {
        dest: Register,
        l: Register,
        r: Register,
    },
    Or {
        dest: Register,
        l: Register,
        r: Register,
    },
    Not {
        dest: Register,
        r: Register,
    },
    LoadConstant {
        dest: Register,
        id: LiteralId,
    },
    LoadVar {
        dest: Register,
        off: VariableOffset,
    },
    SetSubscriptIndex {
        index: Register,
        bounds: VariableOffset,
    },
    LoadSubscript {
        dest: Register,
        off: VariableOffset,
    },
    SetCond {
        cond: Register,
    },
    If {
        dest: Register,
        t: Register,
        f: Register,
    },
    LoadModuleInput {
        dest: Register,
        input: ModuleInputOffset,
    },
    EvalModule {
        id: ModuleId,
    },
    AssignCurr {
        off: VariableOffset,
        value: Register,
    },
    AssignNext {
        off: VariableOffset,
        value: Register,
    },
    Apply {
        dest: Register,
        func: BuiltinId,
    },
    Lookup {
        dest: Register,
        gf: GraphicalFunctionId,
        value: Register,
    },
}

#[derive(Clone, Debug)]
pub struct ModuleDeclaration {
    pub(crate) model_name: String,
    pub(crate) off: usize, // offset within the parent module
}

// these are things that will be shared across bytecode runlists
#[derive(Clone, Debug)]
pub struct ByteCodeContext {
    pub(crate) graphical_functions: Vec<Vec<(f64, f64)>>,
    pub(crate) modules: Vec<ModuleDeclaration>,
}

#[derive(Clone, Debug, Default)]
pub struct ByteCode {
    pub(crate) literals: Vec<f64>,
    pub(crate) code: Vec<Opcode>,
}

impl ByteCode {
    pub(crate) fn intern_literal(&mut self, lit: f64) -> LiteralId {
        self.literals.push(lit);
        (self.literals.len() - 1) as u16
    }

    pub(crate) fn push_opcode(&mut self, op: Opcode) {
        self.code.push(op)
    }
}

#[test]
fn test_opcode_size() {
    use std::mem::size_of;
    assert_eq!(4, size_of::<Opcode>());
}

#[derive(Debug)]
pub struct CompiledModule {
    pub(crate) ident: String,
    pub(crate) n_slots: usize,
    pub(crate) context: ByteCodeContext,
    pub(crate) compiled_initials: ByteCode,
    pub(crate) compiled_flows: ByteCode,
    pub(crate) compiled_stocks: ByteCode,
}
