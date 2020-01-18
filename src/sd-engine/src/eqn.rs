use sd::ast::Expr;
use sd::ast::Expr::*;
use sd::ast::Op1::*;
use sd::ast::Op2::*;
use sd::common;
use sd::tok::{self, Tok};
extern crate lalrpop_util as __lalrpop_util;

mod __parse__Program {
    #![allow(
        non_snake_case,
        non_camel_case_types,
        unused_mut,
        unused_variables,
        unused_imports
    )]

    use sd::ast::Expr;
    use sd::ast::Expr::*;
    use sd::ast::Op1::*;
    use sd::ast::Op2::*;
    use sd::common;
    use sd::tok::{self, Tok};
    extern crate lalrpop_util as __lalrpop_util;
    use super::__ToTriple;
    #[allow(dead_code)]
    pub enum __Symbol<'input> {
        Term_22_21_22(Tok<'input>),
        Term_22_26_26_22(Tok<'input>),
        Term_22_28_22(Tok<'input>),
        Term_22_29_22(Tok<'input>),
        Term_22_2a_22(Tok<'input>),
        Term_22_2b_22(Tok<'input>),
        Term_22_2c_22(Tok<'input>),
        Term_22_2d_22(Tok<'input>),
        Term_22_2f_22(Tok<'input>),
        Term_22_3c_22(Tok<'input>),
        Term_22_3c_3d_22(Tok<'input>),
        Term_22_3c_3e_22(Tok<'input>),
        Term_22_3d_22(Tok<'input>),
        Term_22_3e_22(Tok<'input>),
        Term_22_3e_3d_22(Tok<'input>),
        Term_22Iden_22(&'input str),
        Term_22Num_22(i64),
        Term_22_5b_22(Tok<'input>),
        Term_22_5d_22(Tok<'input>),
        Term_22_5e_22(Tok<'input>),
        Term_22else_22(Tok<'input>),
        Term_22if_22(Tok<'input>),
        Term_22then_22(Tok<'input>),
        Term_22_7c_7c_22(Tok<'input>),
        Termerror(__lalrpop_util::ErrorRecovery<usize, Tok<'input>, tok::Error>),
        NtACmp(Box<Expr>),
        NtAdd(Box<Expr>),
        NtApp(Box<Expr>),
        NtAtom(Box<Expr>),
        NtEq(Box<Expr>),
        NtExp(Box<Expr>),
        NtExpr(Box<Expr>),
        NtExprs(Box<Expr>),
        NtIdent(common::Ident),
        NtInt(Box<Expr>),
        NtLCmp(Box<Expr>),
        NtMul(Box<Expr>),
        NtNum(i64),
        NtProgram(Box<Expr>),
        NtUnary(Box<Expr>),
        Nt____Program(Box<Expr>),
    }
    const __ACTION: &'static [i32] = &[
        // State 0
        16, 0, 17, 0, 0, 18, 0, 19, 0, 0, 0, 0, 0, 0, 0, 20, 21, 0, 0, 0, 0, 22, 0, 0, 0,
        // State 1
        0, 23, 0, -25, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, -25, 0, -25, 24, 0,
        // State 2
        0, 0, 0, 0, 0, 0, 0, 0, 0, 25, 26, -15, -15, 27, 28, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        // State 3
        0, 0, 0, -17, -17, -17, -17, -17, -17, -17, -17, -17, -17, -17, -17, 0, 0, 0, 0, -17, 0, 0,
        0, 0, 0, // State 4
        0, 0, 0, -7, -7, -7, -7, -7, -7, -7, -7, -7, -7, -7, -7, 0, 0, 0, 0, -7, 0, 0, 0, 0, 0,
        // State 5
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 29, 30, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        // State 6
        0, 0, 0, -34, -34, -34, -34, -34, -34, -34, -34, -34, -34, -34, -34, 0, 0, 0, 0, 31, 0, 0,
        0, 0, 0, // State 7
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        // State 8
        0, 0, 32, -8, -8, -8, -8, -8, -8, -8, -8, -8, -8, -8, -8, 0, 0, 0, 0, -8, 0, 0, 0, 0, 0,
        // State 9
        0, 0, 0, -10, -10, -10, -10, -10, -10, -10, -10, -10, -10, -10, -10, 0, 0, 0, 0, -10, 0, 0,
        0, 0, 0, // State 10
        0, 0, 0, -19, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, -19, 0, -19, 0, 0,
        // State 11
        0, 0, 0, -5, 33, 34, -5, 35, 36, -5, -5, -5, -5, -5, -5, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        // State 12
        0, 0, 0, -22, -22, -22, -22, -22, -22, -22, -22, -22, -22, -22, -22, 0, 0, 0, 0, -22, 0, 0,
        0, 0, 0, // State 13
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        // State 14
        0, 0, 0, -28, -28, -28, -28, -28, -28, -28, -28, -28, -28, -28, -28, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, // State 15
        0, 0, 17, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 20, 21, 0, 0, 0, 0, 0, 0, 0, 0,
        // State 16
        16, 0, 17, 0, 0, 18, 0, 19, 0, 0, 0, 0, 0, 0, 0, 20, 21, 0, 0, 0, 0, 22, 0, 0, 0,
        // State 17
        0, 0, 17, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 20, 21, 0, 0, 0, 0, 0, 0, 0, 0,
        // State 18
        0, 0, 17, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 20, 21, 0, 0, 0, 0, 0, 0, 0, 0,
        // State 19
        0, 0, -21, -21, -21, -21, -21, -21, -21, -21, -21, -21, -21, -21, -21, 0, 0, 0, 0, -21, 0,
        0, 0, 0, 0, // State 20
        0, 0, 0, -29, -29, -29, -29, -29, -29, -29, -29, -29, -29, -29, -29, 0, 0, 0, 0, -29, 0, 0,
        0, 0, 0, // State 21
        16, 0, 17, 0, 0, 18, 0, 19, 0, 0, 0, 0, 0, 0, 0, 20, 21, 0, 0, 0, 0, 22, 0, 0, 0,
        // State 22
        16, 0, 17, 0, 0, 18, 0, 19, 0, 0, 0, 0, 0, 0, 0, 20, 21, 0, 0, 0, 0, 0, 0, 0, 0,
        // State 23
        16, 0, 17, 0, 0, 18, 0, 19, 0, 0, 0, 0, 0, 0, 0, 20, 21, 0, 0, 0, 0, 0, 0, 0, 0,
        // State 24
        16, 0, 17, 0, 0, 18, 0, 19, 0, 0, 0, 0, 0, 0, 0, 20, 21, 0, 0, 0, 0, 0, 0, 0, 0,
        // State 25
        16, 0, 17, 0, 0, 18, 0, 19, 0, 0, 0, 0, 0, 0, 0, 20, 21, 0, 0, 0, 0, 0, 0, 0, 0,
        // State 26
        16, 0, 17, 0, 0, 18, 0, 19, 0, 0, 0, 0, 0, 0, 0, 20, 21, 0, 0, 0, 0, 0, 0, 0, 0,
        // State 27
        16, 0, 17, 0, 0, 18, 0, 19, 0, 0, 0, 0, 0, 0, 0, 20, 21, 0, 0, 0, 0, 0, 0, 0, 0,
        // State 28
        16, 0, 17, 0, 0, 18, 0, 19, 0, 0, 0, 0, 0, 0, 0, 20, 21, 0, 0, 0, 0, 0, 0, 0, 0,
        // State 29
        16, 0, 17, 0, 0, 18, 0, 19, 0, 0, 0, 0, 0, 0, 0, 20, 21, 0, 0, 0, 0, 0, 0, 0, 0,
        // State 30
        0, 0, 17, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 20, 21, 0, 0, 0, 0, 0, 0, 0, 0,
        // State 31
        16, 0, 17, 0, 0, 18, 0, 19, 0, 0, 0, 0, 0, 0, 0, 20, 21, 0, 0, 0, 0, 0, 0, 0, 0,
        // State 32
        16, 0, 17, 0, 0, 18, 0, 19, 0, 0, 0, 0, 0, 0, 0, 20, 21, 0, 0, 0, 0, 0, 0, 0, 0,
        // State 33
        16, 0, 17, 0, 0, 18, 0, 19, 0, 0, 0, 0, 0, 0, 0, 20, 21, 0, 0, 0, 0, 0, 0, 0, 0,
        // State 34
        16, 0, 17, 0, 0, 18, 0, 19, 0, 0, 0, 0, 0, 0, 0, 20, 21, 0, 0, 0, 0, 0, 0, 0, 0,
        // State 35
        16, 0, 17, 0, 0, 18, 0, 19, 0, 0, 0, 0, 0, 0, 0, 20, 21, 0, 0, 0, 0, 0, 0, 0, 0,
        // State 36
        0, 0, 0, -33, -33, -33, -33, -33, -33, -33, -33, -33, -33, -33, -33, 0, 0, 0, 0, 31, 0, 0,
        0, 0, 0, // State 37
        0, 0, 0, -20, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        // State 38
        0, 0, 0, 57, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        // State 39
        0, 0, 0, -31, -31, -31, -31, -31, -31, -31, -31, -31, -31, -31, -31, 0, 0, 0, 0, 31, 0, 0,
        0, 0, 0, // State 40
        0, 0, 0, -32, -32, -32, -32, -32, -32, -32, -32, -32, -32, -32, -32, 0, 0, 0, 0, 31, 0, 0,
        0, 0, 0, // State 41
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 58, 0, 0,
        // State 42
        0, 0, 0, -23, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, -23, 0, -23, 0, 0,
        // State 43
        0, 0, 0, -24, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, -24, 0, -24, 0, 0,
        // State 44
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, -11, -11, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        // State 45
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, -12, -12, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        // State 46
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, -13, -13, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        // State 47
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, -14, -14, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        // State 48
        0, -2, 0, -2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, -2, 0, -2, -2, 0,
        // State 49
        0, -1, 0, -1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, -1, 0, -1, -1, 0,
        // State 50
        0, 0, 0, -16, -16, -16, -16, -16, -16, -16, -16, -16, -16, -16, -16, 0, 0, 0, 0, -16, 0, 0,
        0, 0, 0, // State 51
        0, 0, 0, 0, 0, 0, 59, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        // State 52
        0, 0, 0, -26, -26, -26, -26, -26, -26, -26, -26, -26, -26, -26, -26, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, // State 53
        0, 0, 0, -4, 0, 0, -4, 0, 0, -4, -4, -4, -4, -4, -4, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        // State 54
        0, 0, 0, -3, 0, 0, -3, 0, 0, -3, -3, -3, -3, -3, -3, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        // State 55
        0, 0, 0, -27, -27, -27, -27, -27, -27, -27, -27, -27, -27, -27, -27, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, // State 56
        0, 0, 0, -9, -9, -9, -9, -9, -9, -9, -9, -9, -9, -9, -9, 0, 0, 0, 0, -9, 0, 0, 0, 0, 0,
        // State 57
        16, 0, 17, 0, 0, 18, 0, 19, 0, 0, 0, 0, 0, 0, 0, 20, 21, 0, 0, 0, 0, 22, 0, 0, 0,
        // State 58
        16, 0, 17, 0, 0, 18, 0, 19, 0, 0, 0, 0, 0, 0, 0, 20, 21, 0, 0, 0, 0, 0, 0, 0, 0,
        // State 59
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 62, 0, 0, 0, 0,
        // State 60
        0, 0, 0, 63, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        // State 61
        16, 0, 17, 0, 0, 18, 0, 19, 0, 0, 0, 0, 0, 0, 0, 20, 21, 0, 0, 0, 0, 22, 0, 0, 0,
        // State 62
        0, 0, 0, -6, -6, -6, -6, -6, -6, -6, -6, -6, -6, -6, -6, 0, 0, 0, 0, -6, 0, 0, 0, 0, 0,
        // State 63
        0, 0, 0, -18, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, -18, 0, -18, 0, 0,
    ];
    const __EOF_ACTION: &'static [i32] = &[
        0, -25, 0, 0, 0, 0, 0, -30, 0, 0, -19, 0, 0, -35, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, -23, -24, 0, 0, 0, 0, -2, -1, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 0, -18,
    ];
    const __GOTO: &'static [i32] = &[
        // State 0
        2, 3, 4, 5, 6, 7, 8, 0, 9, 10, 11, 12, 13, 14, 15, 0, // State 1
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, // State 2
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, // State 3
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, // State 4
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, // State 5
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, // State 6
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, // State 7
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, // State 8
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, // State 9
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, // State 10
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, // State 11
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, // State 12
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, // State 13
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, // State 14
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, // State 15
        0, 0, 4, 5, 0, 37, 0, 0, 9, 10, 0, 0, 13, 0, 0, 0, // State 16
        2, 3, 4, 5, 6, 7, 38, 39, 9, 10, 11, 12, 13, 0, 15, 0, // State 17
        0, 0, 4, 5, 0, 40, 0, 0, 9, 10, 0, 0, 13, 0, 0, 0, // State 18
        0, 0, 4, 5, 0, 41, 0, 0, 9, 10, 0, 0, 13, 0, 0, 0, // State 19
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, // State 20
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, // State 21
        2, 3, 4, 5, 6, 7, 42, 0, 9, 10, 11, 12, 13, 0, 15, 0, // State 22
        2, 3, 4, 5, 6, 7, 0, 0, 9, 10, 43, 12, 13, 0, 15, 0, // State 23
        2, 3, 4, 5, 6, 7, 0, 0, 9, 10, 44, 12, 13, 0, 15, 0, // State 24
        0, 3, 4, 5, 45, 7, 0, 0, 9, 10, 0, 12, 13, 0, 15, 0, // State 25
        0, 3, 4, 5, 46, 7, 0, 0, 9, 10, 0, 12, 13, 0, 15, 0, // State 26
        0, 3, 4, 5, 47, 7, 0, 0, 9, 10, 0, 12, 13, 0, 15, 0, // State 27
        0, 3, 4, 5, 48, 7, 0, 0, 9, 10, 0, 12, 13, 0, 15, 0, // State 28
        49, 3, 4, 5, 6, 7, 0, 0, 9, 10, 0, 12, 13, 0, 15, 0, // State 29
        50, 3, 4, 5, 6, 7, 0, 0, 9, 10, 0, 12, 13, 0, 15, 0, // State 30
        0, 0, 51, 5, 0, 0, 0, 0, 9, 10, 0, 0, 13, 0, 0, 0, // State 31
        0, 52, 4, 5, 0, 7, 0, 0, 9, 10, 0, 12, 13, 0, 15, 0, // State 32
        0, 0, 4, 5, 0, 7, 0, 0, 9, 10, 0, 0, 13, 0, 53, 0, // State 33
        0, 54, 4, 5, 0, 7, 0, 0, 9, 10, 0, 12, 13, 0, 15, 0, // State 34
        0, 55, 4, 5, 0, 7, 0, 0, 9, 10, 0, 12, 13, 0, 15, 0, // State 35
        0, 0, 4, 5, 0, 7, 0, 0, 9, 10, 0, 0, 13, 0, 56, 0, // State 36
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, // State 37
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, // State 38
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, // State 39
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, // State 40
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, // State 41
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, // State 42
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, // State 43
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, // State 44
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, // State 45
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, // State 46
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, // State 47
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, // State 48
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, // State 49
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, // State 50
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, // State 51
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, // State 52
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, // State 53
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, // State 54
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, // State 55
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, // State 56
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, // State 57
        2, 3, 4, 5, 6, 7, 60, 0, 9, 10, 11, 12, 13, 0, 15, 0, // State 58
        0, 61, 4, 5, 0, 7, 0, 0, 9, 10, 0, 12, 13, 0, 15, 0, // State 59
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, // State 60
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, // State 61
        2, 3, 4, 5, 6, 7, 64, 0, 9, 10, 11, 12, 13, 0, 15, 0, // State 62
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, // State 63
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    ];
    pub fn parse_Program<
        'input,
        __TOKEN: __ToTriple<'input, Error = tok::Error>,
        __TOKENS: IntoIterator<Item = __TOKEN>,
    >(
        text: &'input str,
        __tokens0: __TOKENS,
    ) -> Result<Box<Expr>, __lalrpop_util::ParseError<usize, Tok<'input>, tok::Error>> {
        let __tokens = __tokens0.into_iter();
        let mut __tokens = __tokens.map(|t| __ToTriple::to_triple(t));
        let mut __states = vec![0_i32];
        let mut __symbols = vec![];
        let mut __integer;
        let mut __lookahead;
        let mut __last_location = Default::default();
        '__shift: loop {
            __lookahead = match __tokens.next() {
                Some(Ok(v)) => v,
                None => break '__shift,
                Some(Err(e)) => return Err(__lalrpop_util::ParseError::User { error: e }),
            };
            __last_location = __lookahead.2.clone();
            __integer = match __lookahead.1 {
                Tok::Not if true => 0,
                Tok::And if true => 1,
                Tok::LParen if true => 2,
                Tok::RParen if true => 3,
                Tok::Mul if true => 4,
                Tok::Plus if true => 5,
                Tok::Comma if true => 6,
                Tok::Minus if true => 7,
                Tok::Div if true => 8,
                Tok::Lt if true => 9,
                Tok::Lte if true => 10,
                Tok::Neq if true => 11,
                Tok::Eq if true => 12,
                Tok::Gt if true => 13,
                Tok::Gte if true => 14,
                Tok::Ident(_) if true => 15,
                Tok::Num(_) if true => 16,
                Tok::LBracket if true => 17,
                Tok::RBracket if true => 18,
                Tok::Exp if true => 19,
                Tok::Else if true => 20,
                Tok::If if true => 21,
                Tok::Then if true => 22,
                Tok::Or if true => 23,
                _ => {
                    return Err(__lalrpop_util::ParseError::UnrecognizedToken {
                        token: Some(__lookahead),
                        expected: vec![],
                    });
                }
            };
            '__inner: loop {
                let __state = *__states.last().unwrap() as usize;
                let __action = __ACTION[__state * 25 + __integer];
                if __action > 0 {
                    let __symbol = match __integer {
                        0 => match __lookahead.1 {
                            __tok @ Tok::Not => __Symbol::Term_22_21_22(__tok),
                            _ => unreachable!(),
                        },
                        1 => match __lookahead.1 {
                            __tok @ Tok::And => __Symbol::Term_22_26_26_22(__tok),
                            _ => unreachable!(),
                        },
                        2 => match __lookahead.1 {
                            __tok @ Tok::LParen => __Symbol::Term_22_28_22(__tok),
                            _ => unreachable!(),
                        },
                        3 => match __lookahead.1 {
                            __tok @ Tok::RParen => __Symbol::Term_22_29_22(__tok),
                            _ => unreachable!(),
                        },
                        4 => match __lookahead.1 {
                            __tok @ Tok::Mul => __Symbol::Term_22_2a_22(__tok),
                            _ => unreachable!(),
                        },
                        5 => match __lookahead.1 {
                            __tok @ Tok::Plus => __Symbol::Term_22_2b_22(__tok),
                            _ => unreachable!(),
                        },
                        6 => match __lookahead.1 {
                            __tok @ Tok::Comma => __Symbol::Term_22_2c_22(__tok),
                            _ => unreachable!(),
                        },
                        7 => match __lookahead.1 {
                            __tok @ Tok::Minus => __Symbol::Term_22_2d_22(__tok),
                            _ => unreachable!(),
                        },
                        8 => match __lookahead.1 {
                            __tok @ Tok::Div => __Symbol::Term_22_2f_22(__tok),
                            _ => unreachable!(),
                        },
                        9 => match __lookahead.1 {
                            __tok @ Tok::Lt => __Symbol::Term_22_3c_22(__tok),
                            _ => unreachable!(),
                        },
                        10 => match __lookahead.1 {
                            __tok @ Tok::Lte => __Symbol::Term_22_3c_3d_22(__tok),
                            _ => unreachable!(),
                        },
                        11 => match __lookahead.1 {
                            __tok @ Tok::Neq => __Symbol::Term_22_3c_3e_22(__tok),
                            _ => unreachable!(),
                        },
                        12 => match __lookahead.1 {
                            __tok @ Tok::Eq => __Symbol::Term_22_3d_22(__tok),
                            _ => unreachable!(),
                        },
                        13 => match __lookahead.1 {
                            __tok @ Tok::Gt => __Symbol::Term_22_3e_22(__tok),
                            _ => unreachable!(),
                        },
                        14 => match __lookahead.1 {
                            __tok @ Tok::Gte => __Symbol::Term_22_3e_3d_22(__tok),
                            _ => unreachable!(),
                        },
                        15 => match __lookahead.1 {
                            Tok::Ident(__tok0) => __Symbol::Term_22Iden_22(__tok0),
                            _ => unreachable!(),
                        },
                        16 => match __lookahead.1 {
                            Tok::Num(__tok0) => __Symbol::Term_22Num_22(__tok0),
                            _ => unreachable!(),
                        },
                        17 => match __lookahead.1 {
                            __tok @ Tok::LBracket => __Symbol::Term_22_5b_22(__tok),
                            _ => unreachable!(),
                        },
                        18 => match __lookahead.1 {
                            __tok @ Tok::RBracket => __Symbol::Term_22_5d_22(__tok),
                            _ => unreachable!(),
                        },
                        19 => match __lookahead.1 {
                            __tok @ Tok::Exp => __Symbol::Term_22_5e_22(__tok),
                            _ => unreachable!(),
                        },
                        20 => match __lookahead.1 {
                            __tok @ Tok::Else => __Symbol::Term_22else_22(__tok),
                            _ => unreachable!(),
                        },
                        21 => match __lookahead.1 {
                            __tok @ Tok::If => __Symbol::Term_22if_22(__tok),
                            _ => unreachable!(),
                        },
                        22 => match __lookahead.1 {
                            __tok @ Tok::Then => __Symbol::Term_22then_22(__tok),
                            _ => unreachable!(),
                        },
                        23 => match __lookahead.1 {
                            __tok @ Tok::Or => __Symbol::Term_22_7c_7c_22(__tok),
                            _ => unreachable!(),
                        },
                        _ => unreachable!(),
                    };
                    __states.push(__action - 1);
                    __symbols.push((__lookahead.0, __symbol, __lookahead.2));
                    continue '__shift;
                } else if __action < 0 {
                    if let Some(r) = __reduce(
                        text,
                        __action,
                        Some(&__lookahead.0),
                        &mut __states,
                        &mut __symbols,
                        ::std::marker::PhantomData::<()>,
                    ) {
                        return r;
                    }
                } else {
                    return Err(__lalrpop_util::ParseError::UnrecognizedToken {
                        token: Some(__lookahead),
                        expected: vec![],
                    });
                }
            }
        }
        loop {
            let __state = *__states.last().unwrap() as usize;
            let __action = __EOF_ACTION[__state];
            if __action < 0 {
                if let Some(r) = __reduce(
                    text,
                    __action,
                    None,
                    &mut __states,
                    &mut __symbols,
                    ::std::marker::PhantomData::<()>,
                ) {
                    return r;
                }
            } else {
                let __error = __lalrpop_util::ParseError::UnrecognizedToken {
                    token: None,
                    expected: vec![],
                };
                return Err(__error);
            }
        }
    }
    pub fn __reduce<'input>(
        text: &'input str,
        __action: i32,
        __lookahead_start: Option<&usize>,
        __states: &mut ::std::vec::Vec<i32>,
        __symbols: &mut ::std::vec::Vec<(usize, __Symbol<'input>, usize)>,
        _: ::std::marker::PhantomData<()>,
    ) -> Option<Result<Box<Expr>, __lalrpop_util::ParseError<usize, Tok<'input>, tok::Error>>> {
        let __nonterminal = match -__action {
            1 => {
                // ACmp = Eq, "=", ACmp => ActionFn(7);
                let __sym2 = __pop_NtACmp(__symbols);
                let __sym1 = __pop_Term_22_3d_22(__symbols);
                let __sym0 = __pop_NtEq(__symbols);
                let __start = __sym0.0.clone();
                let __end = __sym2.2.clone();
                let __nt = super::__action7(text, __sym0, __sym1, __sym2);
                let __states_len = __states.len();
                __states.truncate(__states_len - 3);
                __symbols.push((__start, __Symbol::NtACmp(__nt), __end));
                0
            }
            2 => {
                // ACmp = Eq, "<>", ACmp => ActionFn(8);
                let __sym2 = __pop_NtACmp(__symbols);
                let __sym1 = __pop_Term_22_3c_3e_22(__symbols);
                let __sym0 = __pop_NtEq(__symbols);
                let __start = __sym0.0.clone();
                let __end = __sym2.2.clone();
                let __nt = super::__action8(text, __sym0, __sym1, __sym2);
                let __states_len = __states.len();
                __states.truncate(__states_len - 3);
                __symbols.push((__start, __Symbol::NtACmp(__nt), __end));
                0
            }
            3 => {
                // Add = Mul, "-", Add => ActionFn(14);
                let __sym2 = __pop_NtAdd(__symbols);
                let __sym1 = __pop_Term_22_2d_22(__symbols);
                let __sym0 = __pop_NtMul(__symbols);
                let __start = __sym0.0.clone();
                let __end = __sym2.2.clone();
                let __nt = super::__action14(text, __sym0, __sym1, __sym2);
                let __states_len = __states.len();
                __states.truncate(__states_len - 3);
                __symbols.push((__start, __Symbol::NtAdd(__nt), __end));
                1
            }
            4 => {
                // Add = Mul, "+", Add => ActionFn(15);
                let __sym2 = __pop_NtAdd(__symbols);
                let __sym1 = __pop_Term_22_2b_22(__symbols);
                let __sym0 = __pop_NtMul(__symbols);
                let __start = __sym0.0.clone();
                let __end = __sym2.2.clone();
                let __nt = super::__action15(text, __sym0, __sym1, __sym2);
                let __states_len = __states.len();
                __states.truncate(__states_len - 3);
                __symbols.push((__start, __Symbol::NtAdd(__nt), __end));
                1
            }
            5 => {
                // Add = Mul => ActionFn(16);
                let __sym0 = __pop_NtMul(__symbols);
                let __start = __sym0.0.clone();
                let __end = __sym0.2.clone();
                let __nt = super::__action16(text, __sym0);
                let __states_len = __states.len();
                __states.truncate(__states_len - 1);
                __symbols.push((__start, __Symbol::NtAdd(__nt), __end));
                1
            }
            6 => {
                // App = Ident, "(", Add, ",", Add, ")" => ActionFn(26);
                let __sym5 = __pop_Term_22_29_22(__symbols);
                let __sym4 = __pop_NtAdd(__symbols);
                let __sym3 = __pop_Term_22_2c_22(__symbols);
                let __sym2 = __pop_NtAdd(__symbols);
                let __sym1 = __pop_Term_22_28_22(__symbols);
                let __sym0 = __pop_NtIdent(__symbols);
                let __start = __sym0.0.clone();
                let __end = __sym5.2.clone();
                let __nt = super::__action26(text, __sym0, __sym1, __sym2, __sym3, __sym4, __sym5);
                let __states_len = __states.len();
                __states.truncate(__states_len - 6);
                __symbols.push((__start, __Symbol::NtApp(__nt), __end));
                2
            }
            7 => {
                // App = Atom => ActionFn(27);
                let __sym0 = __pop_NtAtom(__symbols);
                let __start = __sym0.0.clone();
                let __end = __sym0.2.clone();
                let __nt = super::__action27(text, __sym0);
                let __states_len = __states.len();
                __states.truncate(__states_len - 1);
                __symbols.push((__start, __Symbol::NtApp(__nt), __end));
                2
            }
            8 => {
                // Atom = Ident => ActionFn(28);
                let __sym0 = __pop_NtIdent(__symbols);
                let __start = __sym0.0.clone();
                let __end = __sym0.2.clone();
                let __nt = super::__action28(text, __sym0);
                let __states_len = __states.len();
                __states.truncate(__states_len - 1);
                __symbols.push((__start, __Symbol::NtAtom(__nt), __end));
                3
            }
            9 => {
                // Atom = "(", Exprs, ")" => ActionFn(29);
                let __sym2 = __pop_Term_22_29_22(__symbols);
                let __sym1 = __pop_NtExprs(__symbols);
                let __sym0 = __pop_Term_22_28_22(__symbols);
                let __start = __sym0.0.clone();
                let __end = __sym2.2.clone();
                let __nt = super::__action29(text, __sym0, __sym1, __sym2);
                let __states_len = __states.len();
                __states.truncate(__states_len - 3);
                __symbols.push((__start, __Symbol::NtAtom(__nt), __end));
                3
            }
            10 => {
                // Atom = Int => ActionFn(30);
                let __sym0 = __pop_NtInt(__symbols);
                let __start = __sym0.0.clone();
                let __end = __sym0.2.clone();
                let __nt = super::__action30(text, __sym0);
                let __states_len = __states.len();
                __states.truncate(__states_len - 1);
                __symbols.push((__start, __Symbol::NtAtom(__nt), __end));
                3
            }
            11 => {
                // Eq = Add, "<", Eq => ActionFn(9);
                let __sym2 = __pop_NtEq(__symbols);
                let __sym1 = __pop_Term_22_3c_22(__symbols);
                let __sym0 = __pop_NtAdd(__symbols);
                let __start = __sym0.0.clone();
                let __end = __sym2.2.clone();
                let __nt = super::__action9(text, __sym0, __sym1, __sym2);
                let __states_len = __states.len();
                __states.truncate(__states_len - 3);
                __symbols.push((__start, __Symbol::NtEq(__nt), __end));
                4
            }
            12 => {
                // Eq = Add, "<=", Eq => ActionFn(10);
                let __sym2 = __pop_NtEq(__symbols);
                let __sym1 = __pop_Term_22_3c_3d_22(__symbols);
                let __sym0 = __pop_NtAdd(__symbols);
                let __start = __sym0.0.clone();
                let __end = __sym2.2.clone();
                let __nt = super::__action10(text, __sym0, __sym1, __sym2);
                let __states_len = __states.len();
                __states.truncate(__states_len - 3);
                __symbols.push((__start, __Symbol::NtEq(__nt), __end));
                4
            }
            13 => {
                // Eq = Add, ">", Eq => ActionFn(11);
                let __sym2 = __pop_NtEq(__symbols);
                let __sym1 = __pop_Term_22_3e_22(__symbols);
                let __sym0 = __pop_NtAdd(__symbols);
                let __start = __sym0.0.clone();
                let __end = __sym2.2.clone();
                let __nt = super::__action11(text, __sym0, __sym1, __sym2);
                let __states_len = __states.len();
                __states.truncate(__states_len - 3);
                __symbols.push((__start, __Symbol::NtEq(__nt), __end));
                4
            }
            14 => {
                // Eq = Add, ">=", Eq => ActionFn(12);
                let __sym2 = __pop_NtEq(__symbols);
                let __sym1 = __pop_Term_22_3e_3d_22(__symbols);
                let __sym0 = __pop_NtAdd(__symbols);
                let __start = __sym0.0.clone();
                let __end = __sym2.2.clone();
                let __nt = super::__action12(text, __sym0, __sym1, __sym2);
                let __states_len = __states.len();
                __states.truncate(__states_len - 3);
                __symbols.push((__start, __Symbol::NtEq(__nt), __end));
                4
            }
            15 => {
                // Eq = Add => ActionFn(13);
                let __sym0 = __pop_NtAdd(__symbols);
                let __start = __sym0.0.clone();
                let __end = __sym0.2.clone();
                let __nt = super::__action13(text, __sym0);
                let __states_len = __states.len();
                __states.truncate(__states_len - 1);
                __symbols.push((__start, __Symbol::NtEq(__nt), __end));
                4
            }
            16 => {
                // Exp = Exp, "^", App => ActionFn(24);
                let __sym2 = __pop_NtApp(__symbols);
                let __sym1 = __pop_Term_22_5e_22(__symbols);
                let __sym0 = __pop_NtExp(__symbols);
                let __start = __sym0.0.clone();
                let __end = __sym2.2.clone();
                let __nt = super::__action24(text, __sym0, __sym1, __sym2);
                let __states_len = __states.len();
                __states.truncate(__states_len - 3);
                __symbols.push((__start, __Symbol::NtExp(__nt), __end));
                5
            }
            17 => {
                // Exp = App => ActionFn(25);
                let __sym0 = __pop_NtApp(__symbols);
                let __start = __sym0.0.clone();
                let __end = __sym0.2.clone();
                let __nt = super::__action25(text, __sym0);
                let __states_len = __states.len();
                __states.truncate(__states_len - 1);
                __symbols.push((__start, __Symbol::NtExp(__nt), __end));
                5
            }
            18 => {
                // Expr = "if", Expr, "then", Expr, "else", Expr => ActionFn(2);
                let __sym5 = __pop_NtExpr(__symbols);
                let __sym4 = __pop_Term_22else_22(__symbols);
                let __sym3 = __pop_NtExpr(__symbols);
                let __sym2 = __pop_Term_22then_22(__symbols);
                let __sym1 = __pop_NtExpr(__symbols);
                let __sym0 = __pop_Term_22if_22(__symbols);
                let __start = __sym0.0.clone();
                let __end = __sym5.2.clone();
                let __nt = super::__action2(text, __sym0, __sym1, __sym2, __sym3, __sym4, __sym5);
                let __states_len = __states.len();
                __states.truncate(__states_len - 6);
                __symbols.push((__start, __Symbol::NtExpr(__nt), __end));
                6
            }
            19 => {
                // Expr = LCmp => ActionFn(3);
                let __sym0 = __pop_NtLCmp(__symbols);
                let __start = __sym0.0.clone();
                let __end = __sym0.2.clone();
                let __nt = super::__action3(text, __sym0);
                let __states_len = __states.len();
                __states.truncate(__states_len - 1);
                __symbols.push((__start, __Symbol::NtExpr(__nt), __end));
                6
            }
            20 => {
                // Exprs = Expr => ActionFn(31);
                let __sym0 = __pop_NtExpr(__symbols);
                let __start = __sym0.0.clone();
                let __end = __sym0.2.clone();
                let __nt = super::__action31(text, __sym0);
                let __states_len = __states.len();
                __states.truncate(__states_len - 1);
                __symbols.push((__start, __Symbol::NtExprs(__nt), __end));
                7
            }
            21 => {
                // Ident = "Iden" => ActionFn(33);
                let __sym0 = __pop_Term_22Iden_22(__symbols);
                let __start = __sym0.0.clone();
                let __end = __sym0.2.clone();
                let __nt = super::__action33(text, __sym0);
                let __states_len = __states.len();
                __states.truncate(__states_len - 1);
                __symbols.push((__start, __Symbol::NtIdent(__nt), __end));
                8
            }
            22 => {
                // Int = Num => ActionFn(32);
                let __sym0 = __pop_NtNum(__symbols);
                let __start = __sym0.0.clone();
                let __end = __sym0.2.clone();
                let __nt = super::__action32(text, __sym0);
                let __states_len = __states.len();
                __states.truncate(__states_len - 1);
                __symbols.push((__start, __Symbol::NtInt(__nt), __end));
                9
            }
            23 => {
                // LCmp = ACmp, "&&", LCmp => ActionFn(4);
                let __sym2 = __pop_NtLCmp(__symbols);
                let __sym1 = __pop_Term_22_26_26_22(__symbols);
                let __sym0 = __pop_NtACmp(__symbols);
                let __start = __sym0.0.clone();
                let __end = __sym2.2.clone();
                let __nt = super::__action4(text, __sym0, __sym1, __sym2);
                let __states_len = __states.len();
                __states.truncate(__states_len - 3);
                __symbols.push((__start, __Symbol::NtLCmp(__nt), __end));
                10
            }
            24 => {
                // LCmp = ACmp, "||", LCmp => ActionFn(5);
                let __sym2 = __pop_NtLCmp(__symbols);
                let __sym1 = __pop_Term_22_7c_7c_22(__symbols);
                let __sym0 = __pop_NtACmp(__symbols);
                let __start = __sym0.0.clone();
                let __end = __sym2.2.clone();
                let __nt = super::__action5(text, __sym0, __sym1, __sym2);
                let __states_len = __states.len();
                __states.truncate(__states_len - 3);
                __symbols.push((__start, __Symbol::NtLCmp(__nt), __end));
                10
            }
            25 => {
                // LCmp = ACmp => ActionFn(6);
                let __sym0 = __pop_NtACmp(__symbols);
                let __start = __sym0.0.clone();
                let __end = __sym0.2.clone();
                let __nt = super::__action6(text, __sym0);
                let __states_len = __states.len();
                __states.truncate(__states_len - 1);
                __symbols.push((__start, __Symbol::NtLCmp(__nt), __end));
                10
            }
            26 => {
                // Mul = Mul, "*", Unary => ActionFn(17);
                let __sym2 = __pop_NtUnary(__symbols);
                let __sym1 = __pop_Term_22_2a_22(__symbols);
                let __sym0 = __pop_NtMul(__symbols);
                let __start = __sym0.0.clone();
                let __end = __sym2.2.clone();
                let __nt = super::__action17(text, __sym0, __sym1, __sym2);
                let __states_len = __states.len();
                __states.truncate(__states_len - 3);
                __symbols.push((__start, __Symbol::NtMul(__nt), __end));
                11
            }
            27 => {
                // Mul = Mul, "/", Unary => ActionFn(18);
                let __sym2 = __pop_NtUnary(__symbols);
                let __sym1 = __pop_Term_22_2f_22(__symbols);
                let __sym0 = __pop_NtMul(__symbols);
                let __start = __sym0.0.clone();
                let __end = __sym2.2.clone();
                let __nt = super::__action18(text, __sym0, __sym1, __sym2);
                let __states_len = __states.len();
                __states.truncate(__states_len - 3);
                __symbols.push((__start, __Symbol::NtMul(__nt), __end));
                11
            }
            28 => {
                // Mul = Unary => ActionFn(19);
                let __sym0 = __pop_NtUnary(__symbols);
                let __start = __sym0.0.clone();
                let __end = __sym0.2.clone();
                let __nt = super::__action19(text, __sym0);
                let __states_len = __states.len();
                __states.truncate(__states_len - 1);
                __symbols.push((__start, __Symbol::NtMul(__nt), __end));
                11
            }
            29 => {
                // Num = "Num" => ActionFn(34);
                let __sym0 = __pop_Term_22Num_22(__symbols);
                let __start = __sym0.0.clone();
                let __end = __sym0.2.clone();
                let __nt = super::__action34(text, __sym0);
                let __states_len = __states.len();
                __states.truncate(__states_len - 1);
                __symbols.push((__start, __Symbol::NtNum(__nt), __end));
                12
            }
            30 => {
                // Program = Expr => ActionFn(1);
                let __sym0 = __pop_NtExpr(__symbols);
                let __start = __sym0.0.clone();
                let __end = __sym0.2.clone();
                let __nt = super::__action1(text, __sym0);
                let __states_len = __states.len();
                __states.truncate(__states_len - 1);
                __symbols.push((__start, __Symbol::NtProgram(__nt), __end));
                13
            }
            31 => {
                // Unary = "+", Exp => ActionFn(20);
                let __sym1 = __pop_NtExp(__symbols);
                let __sym0 = __pop_Term_22_2b_22(__symbols);
                let __start = __sym0.0.clone();
                let __end = __sym1.2.clone();
                let __nt = super::__action20(text, __sym0, __sym1);
                let __states_len = __states.len();
                __states.truncate(__states_len - 2);
                __symbols.push((__start, __Symbol::NtUnary(__nt), __end));
                14
            }
            32 => {
                // Unary = "-", Exp => ActionFn(21);
                let __sym1 = __pop_NtExp(__symbols);
                let __sym0 = __pop_Term_22_2d_22(__symbols);
                let __start = __sym0.0.clone();
                let __end = __sym1.2.clone();
                let __nt = super::__action21(text, __sym0, __sym1);
                let __states_len = __states.len();
                __states.truncate(__states_len - 2);
                __symbols.push((__start, __Symbol::NtUnary(__nt), __end));
                14
            }
            33 => {
                // Unary = "!", Exp => ActionFn(22);
                let __sym1 = __pop_NtExp(__symbols);
                let __sym0 = __pop_Term_22_21_22(__symbols);
                let __start = __sym0.0.clone();
                let __end = __sym1.2.clone();
                let __nt = super::__action22(text, __sym0, __sym1);
                let __states_len = __states.len();
                __states.truncate(__states_len - 2);
                __symbols.push((__start, __Symbol::NtUnary(__nt), __end));
                14
            }
            34 => {
                // Unary = Exp => ActionFn(23);
                let __sym0 = __pop_NtExp(__symbols);
                let __start = __sym0.0.clone();
                let __end = __sym0.2.clone();
                let __nt = super::__action23(text, __sym0);
                let __states_len = __states.len();
                __states.truncate(__states_len - 1);
                __symbols.push((__start, __Symbol::NtUnary(__nt), __end));
                14
            }
            35 => {
                // __Program = Program => ActionFn(0);
                let __sym0 = __pop_NtProgram(__symbols);
                let __start = __sym0.0.clone();
                let __end = __sym0.2.clone();
                let __nt = super::__action0(text, __sym0);
                return Some(Ok(__nt));
            }
            _ => panic!("invalid action code {}", __action),
        };
        let __state = *__states.last().unwrap() as usize;
        let __next_state = __GOTO[__state * 16 + __nonterminal] - 1;
        __states.push(__next_state);
        None
    }
    fn __pop_Term_22_21_22<'input>(
        __symbols: &mut ::std::vec::Vec<(usize, __Symbol<'input>, usize)>,
    ) -> (usize, Tok<'input>, usize) {
        match __symbols.pop().unwrap() {
            (__l, __Symbol::Term_22_21_22(__v), __r) => (__l, __v, __r),
            _ => panic!("symbol type mismatch"),
        }
    }
    fn __pop_Term_22_26_26_22<'input>(
        __symbols: &mut ::std::vec::Vec<(usize, __Symbol<'input>, usize)>,
    ) -> (usize, Tok<'input>, usize) {
        match __symbols.pop().unwrap() {
            (__l, __Symbol::Term_22_26_26_22(__v), __r) => (__l, __v, __r),
            _ => panic!("symbol type mismatch"),
        }
    }
    fn __pop_Term_22_28_22<'input>(
        __symbols: &mut ::std::vec::Vec<(usize, __Symbol<'input>, usize)>,
    ) -> (usize, Tok<'input>, usize) {
        match __symbols.pop().unwrap() {
            (__l, __Symbol::Term_22_28_22(__v), __r) => (__l, __v, __r),
            _ => panic!("symbol type mismatch"),
        }
    }
    fn __pop_Term_22_29_22<'input>(
        __symbols: &mut ::std::vec::Vec<(usize, __Symbol<'input>, usize)>,
    ) -> (usize, Tok<'input>, usize) {
        match __symbols.pop().unwrap() {
            (__l, __Symbol::Term_22_29_22(__v), __r) => (__l, __v, __r),
            _ => panic!("symbol type mismatch"),
        }
    }
    fn __pop_Term_22_2a_22<'input>(
        __symbols: &mut ::std::vec::Vec<(usize, __Symbol<'input>, usize)>,
    ) -> (usize, Tok<'input>, usize) {
        match __symbols.pop().unwrap() {
            (__l, __Symbol::Term_22_2a_22(__v), __r) => (__l, __v, __r),
            _ => panic!("symbol type mismatch"),
        }
    }
    fn __pop_Term_22_2b_22<'input>(
        __symbols: &mut ::std::vec::Vec<(usize, __Symbol<'input>, usize)>,
    ) -> (usize, Tok<'input>, usize) {
        match __symbols.pop().unwrap() {
            (__l, __Symbol::Term_22_2b_22(__v), __r) => (__l, __v, __r),
            _ => panic!("symbol type mismatch"),
        }
    }
    fn __pop_Term_22_2c_22<'input>(
        __symbols: &mut ::std::vec::Vec<(usize, __Symbol<'input>, usize)>,
    ) -> (usize, Tok<'input>, usize) {
        match __symbols.pop().unwrap() {
            (__l, __Symbol::Term_22_2c_22(__v), __r) => (__l, __v, __r),
            _ => panic!("symbol type mismatch"),
        }
    }
    fn __pop_Term_22_2d_22<'input>(
        __symbols: &mut ::std::vec::Vec<(usize, __Symbol<'input>, usize)>,
    ) -> (usize, Tok<'input>, usize) {
        match __symbols.pop().unwrap() {
            (__l, __Symbol::Term_22_2d_22(__v), __r) => (__l, __v, __r),
            _ => panic!("symbol type mismatch"),
        }
    }
    fn __pop_Term_22_2f_22<'input>(
        __symbols: &mut ::std::vec::Vec<(usize, __Symbol<'input>, usize)>,
    ) -> (usize, Tok<'input>, usize) {
        match __symbols.pop().unwrap() {
            (__l, __Symbol::Term_22_2f_22(__v), __r) => (__l, __v, __r),
            _ => panic!("symbol type mismatch"),
        }
    }
    fn __pop_Term_22_3c_22<'input>(
        __symbols: &mut ::std::vec::Vec<(usize, __Symbol<'input>, usize)>,
    ) -> (usize, Tok<'input>, usize) {
        match __symbols.pop().unwrap() {
            (__l, __Symbol::Term_22_3c_22(__v), __r) => (__l, __v, __r),
            _ => panic!("symbol type mismatch"),
        }
    }
    fn __pop_Term_22_3c_3d_22<'input>(
        __symbols: &mut ::std::vec::Vec<(usize, __Symbol<'input>, usize)>,
    ) -> (usize, Tok<'input>, usize) {
        match __symbols.pop().unwrap() {
            (__l, __Symbol::Term_22_3c_3d_22(__v), __r) => (__l, __v, __r),
            _ => panic!("symbol type mismatch"),
        }
    }
    fn __pop_Term_22_3c_3e_22<'input>(
        __symbols: &mut ::std::vec::Vec<(usize, __Symbol<'input>, usize)>,
    ) -> (usize, Tok<'input>, usize) {
        match __symbols.pop().unwrap() {
            (__l, __Symbol::Term_22_3c_3e_22(__v), __r) => (__l, __v, __r),
            _ => panic!("symbol type mismatch"),
        }
    }
    fn __pop_Term_22_3d_22<'input>(
        __symbols: &mut ::std::vec::Vec<(usize, __Symbol<'input>, usize)>,
    ) -> (usize, Tok<'input>, usize) {
        match __symbols.pop().unwrap() {
            (__l, __Symbol::Term_22_3d_22(__v), __r) => (__l, __v, __r),
            _ => panic!("symbol type mismatch"),
        }
    }
    fn __pop_Term_22_3e_22<'input>(
        __symbols: &mut ::std::vec::Vec<(usize, __Symbol<'input>, usize)>,
    ) -> (usize, Tok<'input>, usize) {
        match __symbols.pop().unwrap() {
            (__l, __Symbol::Term_22_3e_22(__v), __r) => (__l, __v, __r),
            _ => panic!("symbol type mismatch"),
        }
    }
    fn __pop_Term_22_3e_3d_22<'input>(
        __symbols: &mut ::std::vec::Vec<(usize, __Symbol<'input>, usize)>,
    ) -> (usize, Tok<'input>, usize) {
        match __symbols.pop().unwrap() {
            (__l, __Symbol::Term_22_3e_3d_22(__v), __r) => (__l, __v, __r),
            _ => panic!("symbol type mismatch"),
        }
    }
    fn __pop_Term_22Iden_22<'input>(
        __symbols: &mut ::std::vec::Vec<(usize, __Symbol<'input>, usize)>,
    ) -> (usize, &'input str, usize) {
        match __symbols.pop().unwrap() {
            (__l, __Symbol::Term_22Iden_22(__v), __r) => (__l, __v, __r),
            _ => panic!("symbol type mismatch"),
        }
    }
    fn __pop_Term_22Num_22<'input>(
        __symbols: &mut ::std::vec::Vec<(usize, __Symbol<'input>, usize)>,
    ) -> (usize, i64, usize) {
        match __symbols.pop().unwrap() {
            (__l, __Symbol::Term_22Num_22(__v), __r) => (__l, __v, __r),
            _ => panic!("symbol type mismatch"),
        }
    }
    fn __pop_Term_22_5b_22<'input>(
        __symbols: &mut ::std::vec::Vec<(usize, __Symbol<'input>, usize)>,
    ) -> (usize, Tok<'input>, usize) {
        match __symbols.pop().unwrap() {
            (__l, __Symbol::Term_22_5b_22(__v), __r) => (__l, __v, __r),
            _ => panic!("symbol type mismatch"),
        }
    }
    fn __pop_Term_22_5d_22<'input>(
        __symbols: &mut ::std::vec::Vec<(usize, __Symbol<'input>, usize)>,
    ) -> (usize, Tok<'input>, usize) {
        match __symbols.pop().unwrap() {
            (__l, __Symbol::Term_22_5d_22(__v), __r) => (__l, __v, __r),
            _ => panic!("symbol type mismatch"),
        }
    }
    fn __pop_Term_22_5e_22<'input>(
        __symbols: &mut ::std::vec::Vec<(usize, __Symbol<'input>, usize)>,
    ) -> (usize, Tok<'input>, usize) {
        match __symbols.pop().unwrap() {
            (__l, __Symbol::Term_22_5e_22(__v), __r) => (__l, __v, __r),
            _ => panic!("symbol type mismatch"),
        }
    }
    fn __pop_Term_22else_22<'input>(
        __symbols: &mut ::std::vec::Vec<(usize, __Symbol<'input>, usize)>,
    ) -> (usize, Tok<'input>, usize) {
        match __symbols.pop().unwrap() {
            (__l, __Symbol::Term_22else_22(__v), __r) => (__l, __v, __r),
            _ => panic!("symbol type mismatch"),
        }
    }
    fn __pop_Term_22if_22<'input>(
        __symbols: &mut ::std::vec::Vec<(usize, __Symbol<'input>, usize)>,
    ) -> (usize, Tok<'input>, usize) {
        match __symbols.pop().unwrap() {
            (__l, __Symbol::Term_22if_22(__v), __r) => (__l, __v, __r),
            _ => panic!("symbol type mismatch"),
        }
    }
    fn __pop_Term_22then_22<'input>(
        __symbols: &mut ::std::vec::Vec<(usize, __Symbol<'input>, usize)>,
    ) -> (usize, Tok<'input>, usize) {
        match __symbols.pop().unwrap() {
            (__l, __Symbol::Term_22then_22(__v), __r) => (__l, __v, __r),
            _ => panic!("symbol type mismatch"),
        }
    }
    fn __pop_Term_22_7c_7c_22<'input>(
        __symbols: &mut ::std::vec::Vec<(usize, __Symbol<'input>, usize)>,
    ) -> (usize, Tok<'input>, usize) {
        match __symbols.pop().unwrap() {
            (__l, __Symbol::Term_22_7c_7c_22(__v), __r) => (__l, __v, __r),
            _ => panic!("symbol type mismatch"),
        }
    }
    fn __pop_Termerror<'input>(
        __symbols: &mut ::std::vec::Vec<(usize, __Symbol<'input>, usize)>,
    ) -> (
        usize,
        __lalrpop_util::ErrorRecovery<usize, Tok<'input>, tok::Error>,
        usize,
    ) {
        match __symbols.pop().unwrap() {
            (__l, __Symbol::Termerror(__v), __r) => (__l, __v, __r),
            _ => panic!("symbol type mismatch"),
        }
    }
    fn __pop_NtACmp<'input>(
        __symbols: &mut ::std::vec::Vec<(usize, __Symbol<'input>, usize)>,
    ) -> (usize, Box<Expr>, usize) {
        match __symbols.pop().unwrap() {
            (__l, __Symbol::NtACmp(__v), __r) => (__l, __v, __r),
            _ => panic!("symbol type mismatch"),
        }
    }
    fn __pop_NtAdd<'input>(
        __symbols: &mut ::std::vec::Vec<(usize, __Symbol<'input>, usize)>,
    ) -> (usize, Box<Expr>, usize) {
        match __symbols.pop().unwrap() {
            (__l, __Symbol::NtAdd(__v), __r) => (__l, __v, __r),
            _ => panic!("symbol type mismatch"),
        }
    }
    fn __pop_NtApp<'input>(
        __symbols: &mut ::std::vec::Vec<(usize, __Symbol<'input>, usize)>,
    ) -> (usize, Box<Expr>, usize) {
        match __symbols.pop().unwrap() {
            (__l, __Symbol::NtApp(__v), __r) => (__l, __v, __r),
            _ => panic!("symbol type mismatch"),
        }
    }
    fn __pop_NtAtom<'input>(
        __symbols: &mut ::std::vec::Vec<(usize, __Symbol<'input>, usize)>,
    ) -> (usize, Box<Expr>, usize) {
        match __symbols.pop().unwrap() {
            (__l, __Symbol::NtAtom(__v), __r) => (__l, __v, __r),
            _ => panic!("symbol type mismatch"),
        }
    }
    fn __pop_NtEq<'input>(
        __symbols: &mut ::std::vec::Vec<(usize, __Symbol<'input>, usize)>,
    ) -> (usize, Box<Expr>, usize) {
        match __symbols.pop().unwrap() {
            (__l, __Symbol::NtEq(__v), __r) => (__l, __v, __r),
            _ => panic!("symbol type mismatch"),
        }
    }
    fn __pop_NtExp<'input>(
        __symbols: &mut ::std::vec::Vec<(usize, __Symbol<'input>, usize)>,
    ) -> (usize, Box<Expr>, usize) {
        match __symbols.pop().unwrap() {
            (__l, __Symbol::NtExp(__v), __r) => (__l, __v, __r),
            _ => panic!("symbol type mismatch"),
        }
    }
    fn __pop_NtExpr<'input>(
        __symbols: &mut ::std::vec::Vec<(usize, __Symbol<'input>, usize)>,
    ) -> (usize, Box<Expr>, usize) {
        match __symbols.pop().unwrap() {
            (__l, __Symbol::NtExpr(__v), __r) => (__l, __v, __r),
            _ => panic!("symbol type mismatch"),
        }
    }
    fn __pop_NtExprs<'input>(
        __symbols: &mut ::std::vec::Vec<(usize, __Symbol<'input>, usize)>,
    ) -> (usize, Box<Expr>, usize) {
        match __symbols.pop().unwrap() {
            (__l, __Symbol::NtExprs(__v), __r) => (__l, __v, __r),
            _ => panic!("symbol type mismatch"),
        }
    }
    fn __pop_NtIdent<'input>(
        __symbols: &mut ::std::vec::Vec<(usize, __Symbol<'input>, usize)>,
    ) -> (usize, common::Ident, usize) {
        match __symbols.pop().unwrap() {
            (__l, __Symbol::NtIdent(__v), __r) => (__l, __v, __r),
            _ => panic!("symbol type mismatch"),
        }
    }
    fn __pop_NtInt<'input>(
        __symbols: &mut ::std::vec::Vec<(usize, __Symbol<'input>, usize)>,
    ) -> (usize, Box<Expr>, usize) {
        match __symbols.pop().unwrap() {
            (__l, __Symbol::NtInt(__v), __r) => (__l, __v, __r),
            _ => panic!("symbol type mismatch"),
        }
    }
    fn __pop_NtLCmp<'input>(
        __symbols: &mut ::std::vec::Vec<(usize, __Symbol<'input>, usize)>,
    ) -> (usize, Box<Expr>, usize) {
        match __symbols.pop().unwrap() {
            (__l, __Symbol::NtLCmp(__v), __r) => (__l, __v, __r),
            _ => panic!("symbol type mismatch"),
        }
    }
    fn __pop_NtMul<'input>(
        __symbols: &mut ::std::vec::Vec<(usize, __Symbol<'input>, usize)>,
    ) -> (usize, Box<Expr>, usize) {
        match __symbols.pop().unwrap() {
            (__l, __Symbol::NtMul(__v), __r) => (__l, __v, __r),
            _ => panic!("symbol type mismatch"),
        }
    }
    fn __pop_NtNum<'input>(
        __symbols: &mut ::std::vec::Vec<(usize, __Symbol<'input>, usize)>,
    ) -> (usize, i64, usize) {
        match __symbols.pop().unwrap() {
            (__l, __Symbol::NtNum(__v), __r) => (__l, __v, __r),
            _ => panic!("symbol type mismatch"),
        }
    }
    fn __pop_NtProgram<'input>(
        __symbols: &mut ::std::vec::Vec<(usize, __Symbol<'input>, usize)>,
    ) -> (usize, Box<Expr>, usize) {
        match __symbols.pop().unwrap() {
            (__l, __Symbol::NtProgram(__v), __r) => (__l, __v, __r),
            _ => panic!("symbol type mismatch"),
        }
    }
    fn __pop_NtUnary<'input>(
        __symbols: &mut ::std::vec::Vec<(usize, __Symbol<'input>, usize)>,
    ) -> (usize, Box<Expr>, usize) {
        match __symbols.pop().unwrap() {
            (__l, __Symbol::NtUnary(__v), __r) => (__l, __v, __r),
            _ => panic!("symbol type mismatch"),
        }
    }
    fn __pop_Nt____Program<'input>(
        __symbols: &mut ::std::vec::Vec<(usize, __Symbol<'input>, usize)>,
    ) -> (usize, Box<Expr>, usize) {
        match __symbols.pop().unwrap() {
            (__l, __Symbol::Nt____Program(__v), __r) => (__l, __v, __r),
            _ => panic!("symbol type mismatch"),
        }
    }
}
pub use self::__parse__Program::parse_Program;

#[allow(unused_variables)]
pub fn __action0<'input>(text: &'input str, (_, __0, _): (usize, Box<Expr>, usize)) -> Box<Expr> {
    (__0)
}

#[allow(unused_variables)]
pub fn __action1<'input>(text: &'input str, (_, __0, _): (usize, Box<Expr>, usize)) -> Box<Expr> {
    (__0)
}

#[allow(unused_variables)]
pub fn __action2<'input>(
    text: &'input str,
    (_, _, _): (usize, Tok<'input>, usize),
    (_, c, _): (usize, Box<Expr>, usize),
    (_, _, _): (usize, Tok<'input>, usize),
    (_, t, _): (usize, Box<Expr>, usize),
    (_, _, _): (usize, Tok<'input>, usize),
    (_, f, _): (usize, Box<Expr>, usize),
) -> Box<Expr> {
    Box::new(If(c, t, f))
}

#[allow(unused_variables)]
pub fn __action3<'input>(text: &'input str, (_, __0, _): (usize, Box<Expr>, usize)) -> Box<Expr> {
    (__0)
}

#[allow(unused_variables)]
pub fn __action4<'input>(
    text: &'input str,
    (_, l, _): (usize, Box<Expr>, usize),
    (_, _, _): (usize, Tok<'input>, usize),
    (_, r, _): (usize, Box<Expr>, usize),
) -> Box<Expr> {
    Box::new(Op2(And, l, r))
}

#[allow(unused_variables)]
pub fn __action5<'input>(
    text: &'input str,
    (_, l, _): (usize, Box<Expr>, usize),
    (_, _, _): (usize, Tok<'input>, usize),
    (_, r, _): (usize, Box<Expr>, usize),
) -> Box<Expr> {
    Box::new(Op2(Or, l, r))
}

#[allow(unused_variables)]
pub fn __action6<'input>(text: &'input str, (_, __0, _): (usize, Box<Expr>, usize)) -> Box<Expr> {
    (__0)
}

#[allow(unused_variables)]
pub fn __action7<'input>(
    text: &'input str,
    (_, l, _): (usize, Box<Expr>, usize),
    (_, _, _): (usize, Tok<'input>, usize),
    (_, r, _): (usize, Box<Expr>, usize),
) -> Box<Expr> {
    Box::new(Op2(Eq, l, r))
}

#[allow(unused_variables)]
pub fn __action8<'input>(
    text: &'input str,
    (_, l, _): (usize, Box<Expr>, usize),
    (_, _, _): (usize, Tok<'input>, usize),
    (_, r, _): (usize, Box<Expr>, usize),
) -> Box<Expr> {
    Box::new(Op2(Neq, l, r))
}

#[allow(unused_variables)]
pub fn __action9<'input>(
    text: &'input str,
    (_, l, _): (usize, Box<Expr>, usize),
    (_, _, _): (usize, Tok<'input>, usize),
    (_, r, _): (usize, Box<Expr>, usize),
) -> Box<Expr> {
    Box::new(Op2(Lt, l, r))
}

#[allow(unused_variables)]
pub fn __action10<'input>(
    text: &'input str,
    (_, l, _): (usize, Box<Expr>, usize),
    (_, _, _): (usize, Tok<'input>, usize),
    (_, r, _): (usize, Box<Expr>, usize),
) -> Box<Expr> {
    Box::new(Op2(Lte, l, r))
}

#[allow(unused_variables)]
pub fn __action11<'input>(
    text: &'input str,
    (_, l, _): (usize, Box<Expr>, usize),
    (_, _, _): (usize, Tok<'input>, usize),
    (_, r, _): (usize, Box<Expr>, usize),
) -> Box<Expr> {
    Box::new(Op2(Gt, l, r))
}

#[allow(unused_variables)]
pub fn __action12<'input>(
    text: &'input str,
    (_, l, _): (usize, Box<Expr>, usize),
    (_, _, _): (usize, Tok<'input>, usize),
    (_, r, _): (usize, Box<Expr>, usize),
) -> Box<Expr> {
    Box::new(Op2(Gte, l, r))
}

#[allow(unused_variables)]
pub fn __action13<'input>(text: &'input str, (_, __0, _): (usize, Box<Expr>, usize)) -> Box<Expr> {
    (__0)
}

#[allow(unused_variables)]
pub fn __action14<'input>(
    text: &'input str,
    (_, l, _): (usize, Box<Expr>, usize),
    (_, _, _): (usize, Tok<'input>, usize),
    (_, r, _): (usize, Box<Expr>, usize),
) -> Box<Expr> {
    Box::new(Op2(Sub, l, r))
}

#[allow(unused_variables)]
pub fn __action15<'input>(
    text: &'input str,
    (_, l, _): (usize, Box<Expr>, usize),
    (_, _, _): (usize, Tok<'input>, usize),
    (_, r, _): (usize, Box<Expr>, usize),
) -> Box<Expr> {
    Box::new(Op2(Add, l, r))
}

#[allow(unused_variables)]
pub fn __action16<'input>(text: &'input str, (_, __0, _): (usize, Box<Expr>, usize)) -> Box<Expr> {
    (__0)
}

#[allow(unused_variables)]
pub fn __action17<'input>(
    text: &'input str,
    (_, l, _): (usize, Box<Expr>, usize),
    (_, _, _): (usize, Tok<'input>, usize),
    (_, r, _): (usize, Box<Expr>, usize),
) -> Box<Expr> {
    Box::new(Op2(Mul, l, r))
}

#[allow(unused_variables)]
pub fn __action18<'input>(
    text: &'input str,
    (_, l, _): (usize, Box<Expr>, usize),
    (_, _, _): (usize, Tok<'input>, usize),
    (_, r, _): (usize, Box<Expr>, usize),
) -> Box<Expr> {
    Box::new(Op2(Div, l, r))
}

#[allow(unused_variables)]
pub fn __action19<'input>(text: &'input str, (_, __0, _): (usize, Box<Expr>, usize)) -> Box<Expr> {
    (__0)
}

#[allow(unused_variables)]
pub fn __action20<'input>(
    text: &'input str,
    (_, _, _): (usize, Tok<'input>, usize),
    (_, e, _): (usize, Box<Expr>, usize),
) -> Box<Expr> {
    Box::new(Op1(Positive, e))
}

#[allow(unused_variables)]
pub fn __action21<'input>(
    text: &'input str,
    (_, _, _): (usize, Tok<'input>, usize),
    (_, e, _): (usize, Box<Expr>, usize),
) -> Box<Expr> {
    Box::new(Op1(Negative, e))
}

#[allow(unused_variables)]
pub fn __action22<'input>(
    text: &'input str,
    (_, _, _): (usize, Tok<'input>, usize),
    (_, e, _): (usize, Box<Expr>, usize),
) -> Box<Expr> {
    Box::new(Op1(Not, e))
}

#[allow(unused_variables)]
pub fn __action23<'input>(text: &'input str, (_, __0, _): (usize, Box<Expr>, usize)) -> Box<Expr> {
    (__0)
}

#[allow(unused_variables)]
pub fn __action24<'input>(
    text: &'input str,
    (_, l, _): (usize, Box<Expr>, usize),
    (_, _, _): (usize, Tok<'input>, usize),
    (_, r, _): (usize, Box<Expr>, usize),
) -> Box<Expr> {
    Box::new(Op2(Exp, l, r))
}

#[allow(unused_variables)]
pub fn __action25<'input>(text: &'input str, (_, __0, _): (usize, Box<Expr>, usize)) -> Box<Expr> {
    (__0)
}

#[allow(unused_variables)]
pub fn __action26<'input>(
    text: &'input str,
    (_, id, _): (usize, common::Ident, usize),
    (_, _, _): (usize, Tok<'input>, usize),
    (_, a, _): (usize, Box<Expr>, usize),
    (_, _, _): (usize, Tok<'input>, usize),
    (_, b, _): (usize, Box<Expr>, usize),
    (_, _, _): (usize, Tok<'input>, usize),
) -> Box<Expr> {
    Box::new(App(id, vec![a, b]))
}

#[allow(unused_variables)]
pub fn __action27<'input>(text: &'input str, (_, __0, _): (usize, Box<Expr>, usize)) -> Box<Expr> {
    (__0)
}

#[allow(unused_variables)]
pub fn __action28<'input>(
    text: &'input str,
    (_, id, _): (usize, common::Ident, usize),
) -> Box<Expr> {
    Box::new(Var(id))
}

#[allow(unused_variables)]
pub fn __action29<'input>(
    text: &'input str,
    (_, _, _): (usize, Tok<'input>, usize),
    (_, __0, _): (usize, Box<Expr>, usize),
    (_, _, _): (usize, Tok<'input>, usize),
) -> Box<Expr> {
    (__0)
}

#[allow(unused_variables)]
pub fn __action30<'input>(text: &'input str, (_, __0, _): (usize, Box<Expr>, usize)) -> Box<Expr> {
    (__0)
}

#[allow(unused_variables)]
pub fn __action31<'input>(text: &'input str, (_, __0, _): (usize, Box<Expr>, usize)) -> Box<Expr> {
    (__0)
}

#[allow(unused_variables)]
pub fn __action32<'input>(text: &'input str, (_, __0, _): (usize, i64, usize)) -> Box<Expr> {
    Box::new(Const(__0 as f64))
}

#[allow(unused_variables)]
pub fn __action33<'input>(
    text: &'input str,
    (_, id, _): (usize, &'input str, usize),
) -> common::Ident {
    String::from(id)
}

#[allow(unused_variables)]
pub fn __action34<'input>(text: &'input str, (_, __0, _): (usize, i64, usize)) -> i64 {
    (__0)
}

pub trait __ToTriple<'input> {
    type Error;
    fn to_triple(value: Self) -> Result<(usize, Tok<'input>, usize), Self::Error>;
}

impl<'input> __ToTriple<'input> for (usize, Tok<'input>, usize) {
    type Error = tok::Error;
    fn to_triple(value: Self) -> Result<(usize, Tok<'input>, usize), tok::Error> {
        Ok(value)
    }
}
impl<'input> __ToTriple<'input> for Result<(usize, Tok<'input>, usize), tok::Error> {
    type Error = tok::Error;
    fn to_triple(value: Self) -> Result<(usize, Tok<'input>, usize), tok::Error> {
        value
    }
}
