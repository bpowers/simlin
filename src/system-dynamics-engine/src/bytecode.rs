// Copyright 2020 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::collections::HashMap;

use ordered_float::OrderedFloat;

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
    Ramp,
    SafeDiv,
    Sin,
    Sqrt,
    Step,
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
    Lt {
        dest: Register,
        l: Register,
        r: Register,
    },
    Lte {
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
    LoadGlobalVar {
        dest: Register,
        off: VariableOffset,
    },
    PushSubscriptIndex {
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
    Ret,
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

#[derive(Clone, Debug, Default)]
pub struct ByteCodeBuilder {
    bytecode: ByteCode,
    interned_literals: HashMap<OrderedFloat<f64>, LiteralId>,
}

impl ByteCodeBuilder {
    pub(crate) fn intern_literal(&mut self, lit: f64) -> LiteralId {
        let key: OrderedFloat<f64> = lit.into();
        if self.interned_literals.contains_key(&key) {
            return self.interned_literals[&key];
        }
        self.bytecode.literals.push(lit);
        let literal_id = (self.bytecode.literals.len() - 1) as u16;
        self.interned_literals.insert(key, literal_id);
        literal_id
    }

    pub(crate) fn push_opcode(&mut self, op: Opcode) {
        self.bytecode.code.push(op)
    }

    pub(crate) fn finish(self) -> ByteCode {
        self.bytecode
    }
}

#[test]
fn test_memoizing_interning() {
    let mut bytecode = ByteCodeBuilder::default();
    let a1 = bytecode.intern_literal(1.0);
    let b1 = bytecode.intern_literal(1.01);
    let b2 = bytecode.intern_literal(1.01);
    let b3 = bytecode.intern_literal(1.01);
    let a2 = bytecode.intern_literal(1.0);
    let b4 = bytecode.intern_literal(1.01);

    assert_eq!(a1, a2);
    assert_eq!(b1, b2);
    assert_eq!(b1, b3);
    assert_eq!(b1, b4);
    assert_ne!(a1, b1);

    let bytecode = bytecode.finish();
    assert_eq!(2, bytecode.literals.len());
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
