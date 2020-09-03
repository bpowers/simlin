use std::collections::HashMap;
use std::rc::Rc;

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

pub struct Var {
    off: usize,
    ast: Rc<Expr>,
}

impl Var {
    pub fn new(off: usize, var: &Variable, is_initial: bool) -> Result<Self> {
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

impl Module {
    fn new(_project: &Project, model: Rc<Model>, is_root: bool) -> Result<Self> {
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

        let mut runlist_initials = Vec::new();
        let mut runlist_flows = Vec::new();
        let mut runlist_stocks = Vec::new();
        for ident in var_names.into_iter() {
            let off = offsets[ident];
            let initial_var = Var::new(off, &model.variables[ident], true)?;
            let var = Var::new(off, &model.variables[ident], false)?;
        }

        Ok(Module {
            base_off: 0,
            n_slots,
            runlist_initials,
            runlist_flows,
            runlist_stocks,
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
