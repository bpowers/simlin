use std::collections::HashMap;
use std::rc::Rc;

use crate::common::Result;
use crate::model::Model;
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
pub enum Expr<'a> {
    Const(f64),
    Var(usize), // offset
    App(&'a str, Vec<Expr<'a>>),
    Op2(BinaryOp, Box<Expr<'a>>, Box<Expr<'a>>),
    Op1(UnaryOp, Box<Expr<'a>>),
    If(Rc<Expr<'a>>, Box<Expr<'a>>, Box<Expr<'a>>),
}

pub struct Var<'a> {
    off: usize,
    ast: Rc<Expr<'a>>,
}

pub struct Module<'a> {
    // inputs: Vec<f64>,
    base_off: usize, // base offset for this module
    n_slots: usize,  // number of f64s we need storage for
    runlist_initials: Vec<Var<'a>>,
    runlist_flows: Vec<Var<'a>>,
    runlist_stocks: Vec<Var<'a>>,
    offsets: HashMap<String, usize>,
}

impl<'a> Module<'a> {
    fn new(_project: &'a Project, model: Rc<Model>, is_root: bool) -> Result<Self> {
        // FIXME: not right -- needs to adjust for submodules
        let n_slots = model.variables.len();
        let mut runlist_initials = Vec::new();
        let mut runlist_flows = Vec::new();
        let mut runlist_stocks = Vec::new();

        let mut offsets: HashMap<String, usize> = HashMap::new();
        let var_names: Vec<&str> = {
            let mut var_names: Vec<_> = model.variables.keys().map(|s| s.as_str()).collect();
            // TODO: if we reorder based on dependencies, we could probably improve performance
            //   through better cache behavior.
            var_names.sort();
            var_names
        };

        let base: usize = if is_root {
            offsets.insert("time".to_string(), 0);
            1
        } else {
            0
        };

        for (i, ident) in var_names.iter().enumerate() {
            offsets.insert(ident.to_string(), base + i);
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

pub struct Simulation<'a> {
    root: Module<'a>,
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

impl<'a> Simulation<'a> {
    pub fn new(project: &'a Project, model: Rc<Model>) -> Result<Self> {
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
