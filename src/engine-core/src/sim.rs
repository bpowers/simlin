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
    Eq,
    And,
    Or,
}

// simplified/lowered from ast::UnaryOp version
#[derive(PartialEq, Eq, Hash, Copy, Clone, Debug)]
pub enum UnaryOp {
    Not,
}

#[derive(PartialEq, Clone, Debug)]
pub enum Expr {
    Const(f64),
    Var(usize), // offset
    App(String, Vec<Expr>),
    Op2(BinaryOp, Box<Expr>, Box<Expr>),
    Op1(UnaryOp, Box<Expr>),
    If(Rc<Expr>, Box<Expr>, Box<Expr>),
}

pub struct Context<'a> {
    is_initial: bool,
    offsets: &'a HashMap<String, usize>,
    reverse_deps: HashMap<String, HashSet<String>>,
}

fn lower(ctx: &Context, expr: &ast::Expr) -> Result<Expr> {
    match expr {
        _ => Ok(Expr::Const(0.0)),
    }
}

pub struct Var {
    off: usize,
    ast: Rc<Expr>,
}

impl Var {
    pub fn new(ctx: &Context, var: &Variable) -> Result<Self> {
        let off = ctx.offsets[var.ident()];
        let ast = match var {
            Variable::Module { .. } => (),
            Variable::Stock { .. } => (),
            Variable::Var { ast, .. } => {}
        };
        Err(SDError::new("not implemented".to_string()))
    }
}

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
    use std::iter::FromIterator;

    fn mapify(input: &[(&str, &[&str])]) -> HashMap<String, HashSet<String>> {
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
}
