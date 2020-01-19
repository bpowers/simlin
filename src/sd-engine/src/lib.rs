#![allow(dead_code)]

extern crate lazy_static;
extern crate regex;
extern crate serde;
extern crate unicode_xid;

#[macro_use]
mod common;
mod enum_set;
mod eqn;
mod sim;
mod tok;
mod xmile;

use std::collections::HashMap;
use std::error;
use std::fmt;
use std::fs;
use std::path::Path;
use std::rc::Rc;

use self::common::Result;
use self::xmile::XmileNode;

use xml5ever::driver::parse_document;
use xml5ever::rcdom::*;
use xml5ever::tendril::TendrilSink;

pub use self::sim::Simulation;

pub struct Project {
    name: String,
    files: Vec<Rc<xmile::File>>,
    models: HashMap<String, Rc<xmile::Model>>,
}

impl fmt::Debug for Project {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "Project{{\n")?;
        write!(f, "  name: {}\n", self.name)?;
        write!(f, "  files: {{\n")?;
        for file in &self.files {
            write!(f, "    {:?}\n", file)?;
        }
        write!(f, "  }}\n")?;
        write!(f, "}}")
    }
}

impl<'a> Project {
    pub fn open(path: &Path) -> Result<Project> {
        let mut file = match fs::File::open(path) {
            // The `description` method of `io::Error` returns a string that
            // describes the error
            Err(why) => {
                return err!(
                    "open({}): {}",
                    path.display(),
                    error::Error::description(&why)
                )
            }
            Ok(file) => file,
        };

        let dom: RcDom = parse_document(RcDom::default(), Default::default())
            .from_utf8()
            .read_from(&mut file)?;

        let f = xmile::File::deserialize(&dom.document)?;
        let mut files = Vec::new();
        let mut models = HashMap::new();

        for model in f.get_models() {
            models.insert(model.get_name().clone(), model.clone());
        }

        files.push(f);

        // TODO: other files referenced by first

        let project = Project {
            name: "test".to_string(),
            files: files,
            models: models,
        };

        Ok(project)
    }

    pub fn new_sim(&self, model_name: &str) -> Result<Simulation> {
        if !self.models.contains_key(model_name) {
            return err!("unknown model");
        }

        // get reference to model, increasing refcount
        let model: Rc<xmile::Model> = self.models.get(model_name).unwrap().clone();

        return Simulation::new(self, model);
    }
}
