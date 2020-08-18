use std::rc::Rc;

use crate::common::Result;
use crate::model;
use crate::Project;

pub struct Var {
    direct_deps: Vec<String>,
}

pub struct Module {
    model: Rc<model::Model>,
    vars: Vec<Rc<Var>>,
}

impl Module {
    fn new(
        _project: &Project,
        _parent: Option<Rc<Var>>,
        _model: Rc<model::Model>,
        _vmodule: Option<Rc<Var>>,
    ) -> Result<Module> {
        return err!("Module::new not implemented");
    }
}

pub struct Simulation {
    module: Rc<Module>,
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
    pub fn new(project: &Project, model: Rc<model::Model>) -> Result<Simulation> {
        // we start with a project and a root module (one with no references).
        let _root = Module::new(project, None, model, None);

        // next, we find all the models (like the main model, stdlib functions, and any
        // user-defined modules) and create sim

        // TODO: come up with monomorphizations based on what inputs are used

        // create AModule for model
        // creates avars for all vars in model + recursive submodules

        // avar_init(module)

        // module assign offsets

        // sort runlists

        // reset

        err!("Simulation::new not implemented")
    }
}
