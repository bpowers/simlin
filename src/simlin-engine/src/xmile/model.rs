// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use quick_xml::Writer;
use serde::{Deserialize, Serialize};

use crate::common::{Canonical, Ident, Result, canonicalize};
use crate::datamodel;
use crate::datamodel::Visibility;
use crate::xmile::variables::{Var, ai_state_from};
use crate::xmile::views::{View, ViewType};
use crate::xmile::{
    SimSpecs, ToXml, XmlWriter, write_tag, write_tag_end, write_tag_start,
    write_tag_start_with_attrs,
};

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Deserialize, Serialize)]
pub struct Model {
    #[serde(rename = "@name", default)]
    pub name: Option<String>,
    #[serde(rename = "namespace")]
    pub namespaces: Option<String>, // comma separated list of namespaces
    pub resource: Option<String>, // path or URL to separate resource file
    pub sim_specs: Option<SimSpecs>,
    pub variables: Option<Variables>,
    pub views: Option<Views>,
}

impl ToXml<XmlWriter> for Model {
    fn write_xml(&self, writer: &mut Writer<XmlWriter>) -> Result<()> {
        if self.name.is_none() || self.name.as_ref().unwrap() == "main" {
            write_tag_start(writer, "model")?;
        } else {
            let attrs = &[("name", self.name.as_deref().unwrap())];
            write_tag_start_with_attrs(writer, "model", attrs)?;
        }

        write_tag_start(writer, "variables")?;

        if let Some(Variables { ref variables }) = self.variables {
            for var in variables.iter() {
                var.write_xml(writer)?;
            }
        }

        write_tag_end(writer, "variables")?;

        write_tag_start(writer, "views")?;

        if let Some(ref views) = self.views {
            if let Some(ref view_list) = views.view {
                for view in view_list.iter() {
                    view.write_xml(writer)?;
                }
            }

            // Write semantic groups
            if let Some(ref groups) = views.groups {
                for group in groups.iter() {
                    let mut attrs: Vec<(&str, &str)> = vec![("name", &group.name)];
                    if let Some(ref owner) = group.owner {
                        attrs.push(("owner", owner));
                    }
                    let run_str;
                    if group.run == Some(true) {
                        run_str = "true".to_string();
                        attrs.push(("run", &run_str));
                    }
                    write_tag_start_with_attrs(writer, "group", &attrs)?;
                    for var in &group.vars {
                        write_tag(writer, "var", var)?;
                    }
                    write_tag_end(writer, "group")?;
                }
            }
        }

        write_tag_end(writer, "views")?;

        write_tag_end(writer, "model")
    }
}

impl From<Model> for datamodel::Model {
    fn from(model: Model) -> Self {
        let xmile_views = model.views.clone().unwrap_or(Views {
            view: None,
            groups: None,
        });
        let views = xmile_views
            .view
            .unwrap_or_default()
            .into_iter()
            .filter(|v| v.kind.unwrap_or(ViewType::StockFlow) == ViewType::StockFlow)
            .map(|v| {
                let mut v = v;
                v.normalize(&model);
                datamodel::View::from(v)
            })
            .collect();
        let groups: Vec<datamodel::ModelGroup> = model
            .views
            .as_ref()
            .and_then(|v| v.groups.as_ref())
            .map(|groups| {
                groups
                    .iter()
                    .map(|g| datamodel::ModelGroup {
                        name: g.name.clone(),
                        doc: None,
                        parent: g.owner.clone(),
                        members: g.vars.clone(),
                        run_enabled: g.run.unwrap_or(false),
                    })
                    .collect()
            })
            .unwrap_or_default();
        datamodel::Model {
            name: model.name.as_deref().unwrap_or("main").to_string(),
            sim_specs: model.sim_specs.map(datamodel::SimSpecs::from),
            variables: match model.variables {
                Some(Variables {
                    variables: vars, ..
                }) => {
                    let mut variables: Vec<datamodel::Variable> = vars
                        .into_iter()
                        .filter(|v| !matches!(v, Var::Unhandled))
                        .map(datamodel::Variable::from)
                        .collect();
                    // Sort variables by canonical identifier for deterministic ordering
                    variables.sort_by(|a, b| {
                        crate::canonicalize(a.get_ident()).cmp(&crate::canonicalize(b.get_ident()))
                    });
                    variables
                }
                _ => vec![],
            },
            views,
            loop_metadata: vec![],
            groups,
        }
    }
}

impl From<datamodel::Model> for Model {
    fn from(model: datamodel::Model) -> Self {
        let datamodel::Model {
            name,
            sim_specs,
            variables,
            views,
            groups,
            ..
        } = model;

        // Convert groups to semantic groups
        let semantic_groups: Option<Vec<SemanticGroup>> = if groups.is_empty() {
            None
        } else {
            Some(
                groups
                    .into_iter()
                    .map(|g| SemanticGroup {
                        name: g.name,
                        owner: g.parent,
                        run: if g.run_enabled { Some(true) } else { None },
                        vars: g.members,
                    })
                    .collect(),
            )
        };

        Model {
            name: Some(name),
            namespaces: None,
            resource: None,
            sim_specs: sim_specs.map(SimSpecs::from),
            variables: if variables.is_empty() {
                None
            } else {
                let variables = variables.into_iter().map(Var::from).collect();
                Some(Variables { variables })
            },
            views: if views.is_empty() && semantic_groups.is_none() {
                None
            } else {
                Some(Views {
                    view: if views.is_empty() {
                        None
                    } else {
                        Some(views.into_iter().map(View::from).collect())
                    },
                    groups: semantic_groups,
                })
            },
        }
    }
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, Default, PartialEq, Deserialize, Serialize)]
pub struct Variables {
    #[serde(rename = "$value", default)]
    pub variables: Vec<Var>,
}

/// Semantic group in <views> section (no geometry, just membership).
/// Used when there are no actual diagram views.
/// In Vensim, these are called "sectors".
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Deserialize, Serialize)]
pub struct SemanticGroup {
    #[serde(rename = "@name")]
    pub name: String,
    #[serde(rename = "@owner", skip_serializing_if = "Option::is_none", default)]
    pub owner: Option<String>,
    #[serde(rename = "@run", skip_serializing_if = "Option::is_none", default)]
    pub run: Option<bool>,
    /// Variable names as <var> children (xmutil format)
    #[serde(rename = "var", default)]
    pub vars: Vec<String>,
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Deserialize, Serialize)]
pub struct Views {
    pub view: Option<Vec<View>>,
    /// Semantic groups appear in views section when there are no diagram views
    #[serde(rename = "group", default)]
    pub groups: Option<Vec<SemanticGroup>>,
}

impl Model {
    pub fn get_name(&self) -> &str {
        self.name.as_deref().unwrap_or("main")
    }

    // TODO: if this is a bottleneck, we should have a normalize pass over
    //   the model to canonicalize things once (and build a map)
    pub fn get_var(&self, ident: &str) -> Option<&Var> {
        self.variables.as_ref()?;

        for var in self.variables.as_ref().unwrap().variables.iter() {
            let name = var.get_noncanonical_name();
            if ident == name || ident == &*canonicalize(name) {
                return Some(var);
            }
        }

        None
    }
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct Module {
    #[serde(rename = "@name")]
    pub name: String,
    #[serde(rename = "@model_name")]
    pub model_name: Option<String>,
    pub doc: Option<String>,
    pub units: Option<String>,
    #[serde(rename = "$value", default)]
    pub refs: Vec<Reference>,
    #[serde(rename = "@access")]
    pub access: Option<String>,
    #[serde(rename = "@ai_state")]
    pub ai_state: Option<String>,
}

pub(crate) fn can_be_module_input(access: &Option<String>) -> bool {
    access
        .as_ref()
        .map(|access| access.eq_ignore_ascii_case("input"))
        .unwrap_or_default()
}

pub(crate) fn visibility(access: &Option<String>) -> Visibility {
    access
        .as_ref()
        .map(|access| {
            if access.eq_ignore_ascii_case("output") {
                Visibility::Public
            } else {
                Visibility::Private
            }
        })
        .unwrap_or(Visibility::Private)
}

pub(crate) fn access_from(visibility: Visibility, can_be_module_input: bool) -> Option<String> {
    if visibility == Visibility::Public {
        Some("output".to_owned())
    } else if can_be_module_input {
        Some("input".to_owned())
    } else {
        None
    }
}

impl ToXml<XmlWriter> for Module {
    fn write_xml(&self, writer: &mut Writer<XmlWriter>) -> Result<()> {
        let mut attrs = vec![("name", self.name.as_str())];
        if self.model_name.is_some() {
            attrs.push(("simlin:model_name", self.name.as_str()));
        }
        if let Some(access) = self.access.as_ref() {
            attrs.push(("access", access.as_str()));
        }
        write_tag_start_with_attrs(writer, "module", &attrs)?;

        if let Some(ref doc) = self.doc {
            write_tag(writer, "doc", doc)?;
        }
        if let Some(ref units) = self.units {
            write_tag(writer, "units", units)?;
        }

        for reference in self.refs.iter() {
            match reference {
                Reference::Connect(connect) => {
                    let attrs = &[("to", connect.dst.as_str()), ("from", connect.src.as_str())];
                    write_tag_start_with_attrs(writer, "connect", attrs)?;
                    write_tag_end(writer, "connect")?;
                }
                Reference::Connect2(_) => {
                    // explicitly ignore these for now
                }
            }
        }

        if let Some(ref ai_state) = self.ai_state {
            write_tag(writer, "ai_state", ai_state)?;
        }

        write_tag_end(writer, "module")
    }
}

impl From<Module> for datamodel::Module {
    fn from(module: Module) -> Self {
        let ident = module.name.clone();
        // TODO: we should filter these to only module inputs, and rewrite
        //       the equations of variables that use module outputs
        let references: Vec<datamodel::ModuleReference> = module
            .refs
            .into_iter()
            .filter(|r| matches!(r, Reference::Connect(_)))
            .map(|r| {
                if let Reference::Connect(r) = r {
                    datamodel::ModuleReference {
                        src: canonicalize(&r.src).into_owned(),
                        dst: canonicalize(&r.dst).into_owned(),
                    }
                } else {
                    unreachable!();
                }
            })
            .collect();
        datamodel::Module {
            ident,
            model_name: match module.model_name {
                Some(model_name) => canonicalize(&model_name).into_owned(),
                None => canonicalize(&module.name).into_owned(),
            },
            documentation: module.doc.unwrap_or_default(),
            units: module.units,
            references,
            can_be_module_input: can_be_module_input(&module.access),
            visibility: visibility(&module.access),
            ai_state: ai_state_from(module.ai_state),
            uid: None,
        }
    }
}

impl From<datamodel::Module> for Module {
    fn from(module: datamodel::Module) -> Self {
        let refs: Vec<Reference> = module
            .references
            .into_iter()
            .map(|mi| {
                Reference::Connect(Connect {
                    src: Ident::<Canonical>::new(&mi.src).to_source_repr(),
                    dst: Ident::<Canonical>::new(&mi.dst).to_source_repr(),
                })
            })
            .collect();
        Module {
            name: module.ident,
            model_name: Some(module.model_name),
            doc: if module.documentation.is_empty() {
                None
            } else {
                Some(module.documentation)
            },
            units: module.units,
            refs,
            access: access_from(module.visibility, module.can_be_module_input),
            ai_state: None, // TODO
        }
    }
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Reference {
    // these only differ in the semantics of their contents
    Connect(Connect),
    Connect2(Connect),
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct Connect {
    #[serde(rename = "@from")]
    pub src: String,
    #[serde(rename = "@to")]
    pub dst: String,
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct NonNegative {}

#[test]
fn test_semantic_group_parsing() {
    let input = r#"<views>
        <group name="Control Panel">
            <var>alpha</var>
            <var>beta</var>
        </group>
        <group name="Financial Sector" owner="Control Panel">
            <var>revenue</var>
        </group>
    </views>"#;

    use quick_xml::de;
    let views: Views = de::from_reader(input.as_bytes()).unwrap();

    let groups = views.groups.expect("groups should exist");
    assert_eq!(2, groups.len());

    assert_eq!("Control Panel", groups[0].name);
    assert_eq!(None, groups[0].owner);
    assert_eq!(
        vec!["alpha".to_string(), "beta".to_string()],
        groups[0].vars
    );

    assert_eq!("Financial Sector", groups[1].name);
    assert_eq!(Some("Control Panel".to_string()), groups[1].owner);
    assert_eq!(vec!["revenue".to_string()], groups[1].vars);
}

#[test]
fn test_semantic_group_roundtrip() {
    let original_model = datamodel::Model {
        name: "main".to_string(),
        sim_specs: None,
        variables: vec![datamodel::Variable::Aux(datamodel::Aux {
            ident: "test_var".to_string(),
            equation: datamodel::Equation::Scalar("1".to_string(), None),
            documentation: String::new(),
            units: None,
            gf: None,
            can_be_module_input: false,
            visibility: datamodel::Visibility::Private,
            ai_state: None,
            uid: None,
        })],
        views: vec![],
        loop_metadata: vec![],
        groups: vec![
            datamodel::ModelGroup {
                name: "Control Panel".to_string(),
                doc: None,
                parent: None,
                members: vec!["test_var".to_string()],
                run_enabled: false,
            },
            datamodel::ModelGroup {
                name: "Financial Sector".to_string(),
                doc: None,
                parent: Some("Control Panel".to_string()),
                members: vec![],
                run_enabled: true,
            },
        ],
    };

    let xmile_model: Model = original_model.clone().into();
    let roundtripped: datamodel::Model = xmile_model.into();

    assert_eq!(original_model.groups.len(), roundtripped.groups.len());
    assert_eq!(original_model.groups[0].name, roundtripped.groups[0].name);
    assert_eq!(
        original_model.groups[0].parent,
        roundtripped.groups[0].parent
    );
    assert_eq!(
        original_model.groups[0].members,
        roundtripped.groups[0].members
    );
    assert_eq!(original_model.groups[1].name, roundtripped.groups[1].name);
    assert_eq!(
        original_model.groups[1].parent,
        roundtripped.groups[1].parent
    );
    assert_eq!(
        original_model.groups[1].run_enabled,
        roundtripped.groups[1].run_enabled
    );
}
