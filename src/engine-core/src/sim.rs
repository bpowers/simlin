use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use crate::ast;
use crate::common::{Result, SDError};
use crate::model::Model;
use crate::variable::Variable;
use crate::Project;

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

#[derive(PartialEq, Eq, Hash, Copy, Clone, Debug)]
pub enum BuiltinFn {
    Lookup,
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
    Safediv,
    Sin,
    Sqrt,
    Tan,
}

#[derive(PartialEq, Clone, Debug)]
pub enum Expr {
    Const(f64),
    Var(usize), // offset
    App(BuiltinFn, Vec<Expr>),
    Op2(BinaryOp, Box<Expr>, Box<Expr>),
    Op1(UnaryOp, Box<Expr>),
    If(Box<Expr>, Box<Expr>, Box<Expr>),
}

pub struct Context<'a> {
    is_initial: bool,
    offsets: &'a HashMap<String, usize>,
    reverse_deps: HashMap<String, HashSet<String>>,
}

fn lower(ctx: &Context, expr: &ast::Expr) -> Result<Expr> {
    let expr = match expr {
        ast::Expr::Const(_, n) => Expr::Const(*n),
        ast::Expr::Var(id) => Expr::Var(ctx.offsets[id]),
        ast::Expr::App(id, _args) => {
            return Err(SDError::new(format!("TODO: apply {}", id)));
        }
        ast::Expr::Op1(op, l) => {
            let l = lower(ctx, l)?;
            match op {
                ast::UnaryOp::Negative => {
                    Expr::Op2(BinaryOp::Sub, Box::new(Expr::Const(0.0)), Box::new(l))
                }
                ast::UnaryOp::Positive => l,
                ast::UnaryOp::Not => Expr::Op1(UnaryOp::Not, Box::new(l)),
            }
        }
        ast::Expr::Op2(op, l, r) => {
            let l = lower(ctx, l)?;
            let r = lower(ctx, r)?;
            match op {
                ast::BinaryOp::Add => Expr::Op2(BinaryOp::Add, Box::new(l), Box::new(r)),
                ast::BinaryOp::Sub => Expr::Op2(BinaryOp::Sub, Box::new(l), Box::new(r)),
                ast::BinaryOp::Exp => Expr::Op2(BinaryOp::Exp, Box::new(l), Box::new(r)),
                ast::BinaryOp::Mul => Expr::Op2(BinaryOp::Mul, Box::new(l), Box::new(r)),
                ast::BinaryOp::Div => Expr::Op2(BinaryOp::Div, Box::new(l), Box::new(r)),
                ast::BinaryOp::Mod => Expr::Op2(BinaryOp::Mod, Box::new(l), Box::new(r)),
                ast::BinaryOp::Gt => Expr::Op2(BinaryOp::Gt, Box::new(l), Box::new(r)),
                ast::BinaryOp::Gte => Expr::Op2(BinaryOp::Gte, Box::new(l), Box::new(r)),
                ast::BinaryOp::Lt => Expr::Op2(BinaryOp::Lt, Box::new(l), Box::new(r)),
                ast::BinaryOp::Lte => Expr::Op2(BinaryOp::Lte, Box::new(l), Box::new(r)),
                ast::BinaryOp::Eq => Expr::Op2(BinaryOp::Eq, Box::new(l), Box::new(r)),
                ast::BinaryOp::Neq => Expr::Op2(BinaryOp::Neq, Box::new(l), Box::new(r)),
                ast::BinaryOp::And => Expr::Op2(BinaryOp::Add, Box::new(l), Box::new(r)),
                ast::BinaryOp::Or => Expr::Op2(BinaryOp::Or, Box::new(l), Box::new(r)),
            }
        }
        ast::Expr::If(cond, t, f) => {
            let cond = lower(ctx, cond)?;
            let t = lower(ctx, t)?;
            let f = lower(ctx, f)?;
            Expr::If(Box::new(cond), Box::new(t), Box::new(f))
        }
    };

    Ok(expr)
}

#[derive(Debug, PartialEq)]
pub struct Var {
    off: usize,
    ast: Expr,
}

fn fold_flows(ctx: &Context, flows: &[String]) -> Option<Expr> {
    if flows.is_empty() {
        return None;
    }

    let mut loads = flows.iter().map(|flow| Expr::Var(ctx.offsets[flow]));

    let first = loads.next().unwrap();
    Some(loads.fold(first, |acc, flow| {
        Expr::Op2(BinaryOp::Add, Box::new(acc), Box::new(flow))
    }))
}

#[test]
fn test_fold_flows() {
    use std::iter::FromIterator;

    let offsets: &[(&str, usize)] = &[("time", 0), ("a", 1), ("b", 2), ("c", 3), ("d", 4)];
    let offsets: HashMap<String, usize> =
        HashMap::from_iter(offsets.into_iter().map(|(k, v)| (k.to_string(), *v)));
    let ctx = Context {
        is_initial: false,
        offsets: &offsets,
        reverse_deps: HashMap::new(),
    };

    assert_eq!(None, fold_flows(&ctx, &[]));
    assert_eq!(Some(Expr::Var(1)), fold_flows(&ctx, &["a".to_string()]));
    assert_eq!(
        Some(Expr::Op2(
            BinaryOp::Add,
            Box::new(Expr::Var(1)),
            Box::new(Expr::Var(4))
        )),
        fold_flows(&ctx, &["a".to_string(), "d".to_string()])
    );
}

fn build_stock_update_expr(ctx: &Context, var: &Variable) -> Result<Expr> {
    if let Variable::Stock {
        ident,
        inflows,
        outflows,
        ..
    } = var
    {
        // start off with stock = stock
        let mut expr = Expr::Var(ctx.offsets[ident]);
        match fold_flows(ctx, inflows) {
            None => (),
            Some(flows) => {
                expr = Expr::Op2(BinaryOp::Add, Box::new(expr), Box::new(flows));
            }
        }
        match fold_flows(ctx, outflows) {
            None => (),
            Some(flows) => {
                expr = Expr::Op2(BinaryOp::Sub, Box::new(expr), Box::new(flows));
            }
        }

        Ok(expr)
    } else {
        panic!(
            "build_stock_update_expr called with non-stock {}",
            var.ident()
        );
    }
}

impl Var {
    pub fn new(ctx: &Context, var: &Variable) -> Result<Self> {
        let off = ctx.offsets[var.ident()];
        let ast = match var {
            Variable::Module { .. } => {
                return Err(SDError::new(format!(
                    "TODO module AST building for {}",
                    var.ident()
                )));
            }
            Variable::Stock { ast, .. } => {
                if ctx.is_initial {
                    if ast.is_none() {
                        return Err(SDError::new(format!(
                            "missing initial AST for stock {}",
                            var.ident()
                        )));
                    }
                    lower(ctx, ast.as_ref().unwrap())?
                } else {
                    build_stock_update_expr(ctx, var)?
                }
            }
            Variable::Var { ast, .. } => {
                if let Some(ast) = ast {
                    lower(ctx, ast)?
                } else {
                    return Err(SDError::new(format!("missing AST for {}", var.ident())));
                }
            }
        };
        Ok(Var { off, ast })
    }
}

#[derive(Debug, PartialEq)]
pub struct Module {
    // inputs: Vec<f64>,
    base_off: usize, // base offset for this module
    n_slots: usize,  // number of f64s we need storage for
    runlist_initials: Vec<Var>,
    runlist_flows: Vec<Var>,
    runlist_stocks: Vec<Var>,
    offsets: HashMap<String, usize>,
}

fn invert_deps(forward: &HashMap<String, HashSet<String>>) -> HashMap<String, HashSet<String>> {
    let mut reverse: HashMap<String, HashSet<String>> = HashMap::new();
    for (ident, deps) in forward.iter() {
        if !reverse.contains_key(ident) {
            reverse.insert(ident.clone(), HashSet::new());
        }
        for dep in deps {
            if !reverse.contains_key(dep) {
                reverse.insert(dep.clone(), HashSet::new());
            }

            reverse.get_mut(dep).unwrap().insert(ident.clone());
        }
    }
    reverse
}

#[test]
fn test_invert_deps() {
    fn mapify(input: &[(&str, &[&str])]) -> HashMap<String, HashSet<String>> {
        use std::iter::FromIterator;
        input
            .into_iter()
            .map(|(k, v)| {
                (
                    k.to_string(),
                    HashSet::from_iter(v.into_iter().map(|s| s.to_string())),
                )
            })
            .collect()
    }

    let forward: &[(&str, &[&str])] = &[
        ("a", &["b", "c"]),
        ("b", &["d", "c"]),
        ("f", &["a"]),
        ("e", &[]),
    ];
    let forward = mapify(forward);

    let reverse = invert_deps(&forward);

    let expected: &[(&str, &[&str])] = &[
        ("a", &["f"]),
        ("b", &["a"]),
        ("c", &["a", "b"]),
        ("d", &["b"]),
        ("f", &[]),
        ("e", &[]),
    ];
    let expected = mapify(expected);

    assert_eq!(expected, reverse);
}

impl Module {
    fn new(_project: &Project, model: Rc<Model>, is_root: bool) -> Result<Self> {
        if model.dt_deps.is_none() || model.initial_deps.is_none() {
            return Err(SDError::new(
                "can't simulate if dependency building failed".to_string(),
            ));
        }

        // FIXME: not right -- needs to adjust for submodules
        let n_slots = model.variables.len();

        let var_names: Vec<&str> = {
            let mut var_names: Vec<_> = model.variables.keys().map(|s| s.as_str()).collect();
            // TODO: if we reorder based on dependencies, we could probably improve performance
            //   through better cache behavior.
            var_names.sort();
            var_names
        };

        let offsets: HashMap<String, usize> = {
            let mut offsets = HashMap::new();
            let base: usize = if is_root {
                offsets.insert("time".to_string(), 0);
                1
            } else {
                0
            };
            offsets.extend(
                var_names
                    .iter()
                    .enumerate()
                    .map(|(i, ident)| (ident.to_string(), base + i)),
            );

            offsets
        };

        let ctx = Context {
            offsets: &offsets,
            reverse_deps: invert_deps(&model.initial_deps.as_ref().unwrap()),
            is_initial: true,
        };

        let runlist_initials: Result<Vec<Var>> = var_names
            .iter()
            .map(|id| &model.variables[*id])
            .map(|v| Var::new(&ctx, v))
            .collect();

        let ctx = Context {
            offsets: &offsets,
            reverse_deps: invert_deps(&model.dt_deps.as_ref().unwrap()),
            is_initial: false,
        };

        let runlist_flows: Result<Vec<Var>> = var_names
            .iter()
            .map(|id| &model.variables[*id])
            .filter(|v| !v.is_stock())
            .map(|v| Var::new(&ctx, v))
            .collect();

        let runlist_stocks: Result<Vec<Var>> = var_names
            .iter()
            .map(|id| &model.variables[*id])
            .filter(|v| v.is_stock())
            .map(|v| Var::new(&ctx, v))
            .collect();

        Ok(Module {
            base_off: 0,
            n_slots,
            runlist_initials: runlist_initials?,
            runlist_flows: runlist_flows?,
            runlist_stocks: runlist_stocks?,
            offsets,
        })
    }
}

#[derive(Debug, PartialEq)]
pub struct Simulation {
    root: Module,
    // spec
    // slab
    // curr
    // next
    // nvars
    // nsaves
    // nsteps
    // step
    // save_step
    // save_every
}

impl Simulation {
    pub fn new(project: &Project, model: Rc<Model>) -> Result<Self> {
        // we start with a project and a root module (one with no references).
        let root = Module::new(project, model, true).unwrap();

        // next, we find all the models (like the main model, stdlib functions, and any
        // user-defined modules) and create sim

        // TODO: come up with monomorphizations based on what inputs are used

        // create AModule for model
        // creates avars for all vars in model + recursive submodules

        // avar_init(module)

        // module assign offsets

        // sort runlists

        // reset

        Ok(Simulation { root })
    }

    pub fn run_to_end(&self) -> Result<()> {
        Ok(())
    }
}
