use std::collections::HashMap;
use std::rc::Rc;

use crate::common::{Ident, Result};
use crate::model;
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
    model: Rc<model::Model>,
    // inputs: Vec<f64>,
    base_off: usize, // base offset for this module
    n_slots: usize,  // number of f64s we need storage for
    runlist_initials: Vec<Var<'a>>,
    runlist_flows: Vec<Var<'a>>,
    runlist_stocks: Vec<Var<'a>>,
    offsets: HashMap<Ident, usize>,
}

impl Module<'_> {
    fn new(_project: &Project, model: Rc<model::Model>) -> Result<Self> {
        // FIXME: not right -- needs to adjust for submodules
        let n_slots = model.variables.len();
        let runlist_initials = Vec::new();
        let runlist_flows = Vec::new();
        let runlist_stocks = Vec::new();

        let offsets = HashMap::new();

        Ok(Module {
            model,
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

impl Simulation<'_> {
    pub fn new(project: &Project, model: Rc<model::Model>) -> Result<Simulation> {
        // we start with a project and a root module (one with no references).
        let root = Module::new(project, model).unwrap();

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
