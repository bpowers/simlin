// Copyright 2020 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// simplified/lowered from ast::BinaryOp version
#[derive(PartialEq, Eq, Hash, Copy, Clone, Debug)]
pub enum BinaryOp {
    Add,
    Sub,
    Exp,
    Mul,
    Div,
    Mod,
    Gt,
    Gte,
    Lt,
    Lte,
    Eq,
    Neq,
    And,
    Or,
}

// simplified/lowered from ast::UnaryOp version
#[derive(PartialEq, Eq, Hash, Copy, Clone, Debug)]
pub enum UnaryOp {
    Not,
}

/*
#[derive(PartialEq, Clone, Debug)]
pub enum Builtin1Fn {}

#[derive(PartialEq, Clone, Debug)]
pub enum Builtin2Fn {}

#[derive(PartialEq, Clone, Debug)]
pub enum Builtin3Fn {}

#[derive(PartialEq, Clone, Debug)]
pub enum Bytecode {
    LoadConst(f64),
    LoadVar(usize),
    Builtin1(Builtin1Fn),
    Builtin2(Builtin2Fn),
    Builtin3(Builtin3Fn),
    Op2(BinaryOp),
    Op1(BinaryOp),
}
*/
