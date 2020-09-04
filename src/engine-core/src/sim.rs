use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use crate::ast;
use crate::common::{Ident, Result, SDError};
use crate::model::Model;
use crate::variable::Variable;
use crate::xmile;
use crate::Project;

const TIME_OFF: usize = 0;

#[derive(PartialEq, Eq, Hash, Copy, Clone, Debug)]
pub enum Method {
    Euler,
}

#[derive(Clone, Debug)]
pub struct Specs {
    pub start: f64,
    pub stop: f64,
    pub dt: f64,
    pub save_step: f64,
    pub method: Method,
}

const DEFAULT_DT: xmile::Dt = xmile::Dt {
    value: 1.0,
    reciprocal: None,
};

impl Specs {
    pub fn from(specs: &xmile::SimSpecs) -> Self {
        let dt: f64 = {
            let spec_dt = specs.dt.as_ref().unwrap_or(&DEFAULT_DT);
            if spec_dt.reciprocal.unwrap_or(false) {
                1.0 / spec_dt.value
            } else {
                spec_dt.value
            }
        };

        let save_step: f64 = specs.save_step.unwrap_or(dt);

        let method = if specs.method.is_none() {
            Method::Euler
        } else {
            let method_str = specs.method.as_ref().unwrap();
            match method_str.to_lowercase().as_str() {
                "euler" => Method::Euler,
                _ => {
                    eprintln!(
                        "warning, simulation requested '{}' method, but only support Euler",
                        method_str
                    );
                    Method::Euler
                }
            }
        };

        Specs {
            start: specs.start,
            stop: specs.stop,
            dt,
            save_step,
            method,
        }
    }
}

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
        inflows, outflows, ..
    } = var
    {
        // TODO: simplify the expressions we generate
        let inflows = match fold_flows(ctx, inflows) {
            None => Expr::Const(0.0),
            Some(flows) => flows,
        };
        let outflows = match fold_flows(ctx, outflows) {
            None => Expr::Const(0.0),
            Some(flows) => flows,
        };

        Ok(Expr::Op2(
            BinaryOp::Sub,
            Box::new(inflows),
            Box::new(outflows),
        ))
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
        let n_slots = model.variables.len() + 1; // add time in there

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

        fn var_comparator<'a>(
            deps: &'a HashMap<Ident, HashSet<Ident>>,
        ) -> impl Fn(&&Variable, &&Variable) -> std::cmp::Ordering + 'a {
            move |a: &&Variable, b: &&Variable| -> std::cmp::Ordering {
                let a_name = a.ident();
                let b_name = b.ident();
                if deps[b_name].contains(a_name) {
                    std::cmp::Ordering::Less
                } else if deps[a_name].contains(b_name) {
                    std::cmp::Ordering::Greater
                } else {
                    std::cmp::Ordering::Equal
                }
            }
        }

        let initial_deps = model.initial_deps.as_ref().unwrap();
        let ctx = Context {
            offsets: &offsets,
            reverse_deps: invert_deps(initial_deps),
            is_initial: true,
        };

        let mut runlist_initials: Vec<&Variable> =
            var_names.iter().map(|id| &model.variables[*id]).collect();

        runlist_initials.sort_by(var_comparator(initial_deps));

        let runlist_initials: Result<Vec<Var>> = runlist_initials
            .into_iter()
            .map(|v| Var::new(&ctx, v))
            .collect();

        let dt_deps = model.dt_deps.as_ref().unwrap();
        let ctx = Context {
            offsets: &offsets,
            reverse_deps: invert_deps(dt_deps),
            is_initial: false,
        };

        let mut runlist_flows: Vec<&Variable> = var_names
            .iter()
            .map(|id| &model.variables[*id])
            .filter(|v| !v.is_stock())
            .collect();

        runlist_flows.sort_by(var_comparator(dt_deps));

        let runlist_flows: Result<Vec<Var>> = runlist_flows
            .into_iter()
            .map(|v| Var::new(&ctx, v))
            .collect();

        // no sorting needed for stocks
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

    fn calc_initials(&self, _dt: f64, curr: &mut [f64]) {
        for v in self.runlist_initials.iter() {
            curr[v.off] = eval(&v.ast, curr);
        }
    }

    fn calc_flows(&self, _dt: f64, curr: &mut [f64]) {
        for v in self.runlist_flows.iter() {
            curr[v.off] = eval(&v.ast, curr);
        }
    }

    fn calc_stocks(&self, dt: f64, curr: &[f64], next: &mut [f64]) {
        for v in self.runlist_stocks.iter() {
            next[v.off] = curr[v.off] + eval(&v.ast, curr) * dt;
        }
    }
}

fn is_truthy(n: f64) -> bool {
    let is_false = approx_eq!(f64, n, 0.0);
    !is_false
}

fn eval(expr: &Expr, curr: &[f64]) -> f64 {
    match expr {
        Expr::Const(n) => *n,
        Expr::Var(off) => curr[*off],
        Expr::If(cond, t, f) => {
            let cond: f64 = eval(cond, curr);
            if is_truthy(cond) {
                eval(t, curr)
            } else {
                eval(f, curr)
            }
        }
        Expr::Op1(op, l) => {
            let l = eval(l, curr);
            match op {
                UnaryOp::Not => (!is_truthy(l)) as i8 as f64,
            }
        }
        Expr::Op2(op, l, r) => {
            let l = eval(l, curr);
            let r = eval(r, curr);
            match op {
                BinaryOp::Add => l + r,
                BinaryOp::Sub => l - r,
                BinaryOp::Exp => l.powf(r),
                BinaryOp::Mul => l * r,
                BinaryOp::Div => l / r,
                BinaryOp::Mod => l.rem_euclid(r),
                BinaryOp::Gt => (l > r) as i8 as f64,
                BinaryOp::Gte => (l >= r) as i8 as f64,
                BinaryOp::Lt => (l < r) as i8 as f64,
                BinaryOp::Lte => (l <= r) as i8 as f64,
                BinaryOp::Eq => approx_eq!(f64, l, r) as i8 as f64,
                BinaryOp::Neq => !approx_eq!(f64, l, r) as i8 as f64,
                BinaryOp::And => panic!("TODO 'and'"),
                BinaryOp::Or => panic!("TODO 'or'"),
            }
        }
        _ => 0.0,
    }
}

#[derive(Debug)]
pub struct Results {
    offsets: HashMap<String, usize>,
    // one large allocation
    data: Box<[f64]>,
    step_size: usize,
    step_count: usize,
}

impl Results {
    pub fn print_tsv(&self) {
        let var_names = {
            let offset_name_map: HashMap<usize, &str> =
                self.offsets.iter().map(|(k, v)| (*v, k.as_str())).collect();
            let mut var_names: Vec<&str> = Vec::with_capacity(self.step_size);
            for i in 0..(self.step_size) {
                var_names.push(offset_name_map[&i]);
            }
            var_names
        };

        // print header
        for (i, id) in var_names.iter().enumerate() {
            print!("{}", id);
            if i == var_names.len() - 1 {
                println!();
            } else {
                print!("\t");
            }
        }

        for curr in self.data.chunks(self.step_size) {
            for (i, val) in curr.iter().enumerate() {
                print!("{}", val);
                if i == var_names.len() - 1 {
                    println!();
                } else {
                    print!("\t");
                }
            }
        }
    }
}

#[derive(Debug)]
pub struct Simulation {
    root: Module,
    specs: Specs,
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

        // TODO: come up with monomorphizations based on what inputs are used

        // module assign offsets

        // reset

        let specs = Specs::from(project.file.sim_specs.as_ref().unwrap());

        Ok(Simulation { root, specs })
    }

    fn calc_initials(&self, dt: f64, curr: &mut [f64]) {
        curr[TIME_OFF] = self.specs.start;

        self.root.calc_initials(dt, curr);
    }

    fn calc_flows(&self, dt: f64, curr: &mut [f64]) {
        self.root.calc_flows(dt, curr);
    }

    fn calc_stocks(&self, dt: f64, curr: &[f64], next: &mut [f64]) {
        next[TIME_OFF] = curr[TIME_OFF] + dt;
        self.root.calc_stocks(dt, curr, next);
    }

    pub fn run_to_end(&self) -> Result<Results> {
        let spec = &self.specs;
        if spec.stop < spec.start {
            return Err(SDError::new(format!(
                "sim spec stop ({}) < start ({})",
                spec.stop, spec.start
            )));
        }
        let n_chunks: usize = ((spec.stop - spec.start) / spec.dt + 1.0) as usize;

        let dt = spec.dt;
        let stop = spec.stop;

        let slab: Vec<f64> = vec![0.0; self.root.n_slots * (n_chunks + 1)];
        let mut boxed_slab = slab.into_boxed_slice();
        {
            let mut slabs = boxed_slab.chunks_mut(self.root.n_slots);

            // let mut results: Vec<&[f64]> = Vec::with_capacity(n_chunks + 1);

            let mut curr = slabs.next().unwrap();
            self.calc_initials(dt, curr);

            for next in slabs {
                self.calc_flows(dt, curr);
                self.calc_stocks(dt, curr, next);
                curr = next;
            }
            // ensure we've calculated stock + flow values for the dt <= end_time
            assert!(curr[TIME_OFF] > stop);
        }

        let mut step = 0;
        let mut save_step_off = 0;
        let save_every = std::cmp::max(1, (spec.save_step / spec.dt + 0.5) as usize);
        let n_save_chunks: usize = ((spec.stop - spec.start) / spec.dt + 1.0) as usize;

        let results_slab: Vec<f64> = vec![0.0; self.root.n_slots * n_save_chunks];
        let mut boxed_results_slab = results_slab.into_boxed_slice();
        {
            let mut slabs: Vec<&mut [f64]> =
                boxed_results_slab.chunks_mut(self.root.n_slots).collect();

            for curr in boxed_slab.chunks(self.root.n_slots) {
                if curr[TIME_OFF] > stop {
                    break;
                }
                if step % save_every == 0 {
                    slabs[save_step_off].copy_from_slice(curr);
                    save_step_off += 1;
                }
                step += 1;
            }
        }

        Ok(Results {
            offsets: self.root.offsets.clone(),
            data: boxed_results_slab,
            step_size: self.root.n_slots,
            step_count: n_save_chunks,
        })
    }
}
