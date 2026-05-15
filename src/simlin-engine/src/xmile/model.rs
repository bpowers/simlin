// Copyright 2026 The Simlin Authors. All rights reserved.
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
    write_tag_start_with_attrs, write_tag_with_attrs,
};

/// Vendor extension for persisting named feedback loop metadata.
/// Serialized as `<simlin:loop-metadata>` within a `<model>` element.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct XmileLoopMetadata {
    #[serde(rename = "@name")]
    pub name: String,
    #[serde(rename = "@deleted", skip_serializing_if = "Option::is_none", default)]
    pub deleted: Option<bool>,
    #[serde(
        rename = "@description",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub description: Option<String>,
    /// Comma-separated list of variable UIDs
    #[serde(rename = "$text", default)]
    pub uids_text: Option<String>,
}

/// One input-port wiring of a multi-output macro invocation:
/// `<simlin:input from="in1" to="a"/>` -- `from` is the argument's source
/// ident, `to` is the bare macro formal-parameter name.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct MacroInvocationInput {
    #[serde(rename = "@from")]
    pub from: String,
    #[serde(rename = "@to")]
    pub to: String,
}

/// One additional-output binding of a multi-output macro invocation:
/// `<simlin:output binding="the_min" output="minval"/>` -- `binding` is the
/// caller-side variable ident, `output` is the macro-internal output name.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct MacroInvocationOutput {
    #[serde(rename = "@binding")]
    pub binding: String,
    #[serde(rename = "@output")]
    pub output: String,
}

/// Vendor extension recording a Vensim multi-output (`:`-list) macro
/// invocation. Standard XMILE `<module>` references a `<model>`, not a
/// `<macro>`, and has no multi-output-call concept, so the Phase-4
/// materialized cluster -- an input-only `Variable::Module` plus the LHS
/// primary-output binding `Aux` and one `Aux` per additional `:`-output --
/// round-trips through this single element instead of standard
/// `<module>`/`<aux>`es. Serialized as `<simlin:macro-invocation>` within a
/// `<model>`. quick-xml strips the `simlin:` prefix on read, so the serde
/// rename is the namespace-stripped local name.
///
/// The reader reconstructs *exactly* the materialized datamodel cluster, so
/// an XMILE->datamodel->XMILE round-trip is byte-stable.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct MacroInvocation {
    /// The materialized module ident (e.g. `total_macro`).
    #[serde(rename = "@module")]
    pub module: String,
    /// The invoked macro's `Model.name` (e.g. `add3`).
    #[serde(rename = "@macro")]
    pub macro_name: String,
    /// The caller-side LHS variable ident the primary output binds to.
    #[serde(rename = "@primary-binding")]
    pub primary_binding: String,
    /// The macro's primary-output name (its `MacroSpec.primary_output`).
    #[serde(rename = "@primary-output")]
    pub primary_output: String,
    /// Documentation carried on the primary-output binding aux (the original
    /// invocation's `~`-doc), preserved for fidelity.
    #[serde(
        rename = "@primary-doc",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub primary_doc: Option<String>,
    /// Units carried on the primary-output binding aux, preserved for
    /// fidelity.
    #[serde(
        rename = "@primary-units",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub primary_units: Option<String>,
    /// Input-port wirings, in positional call order.
    #[serde(rename = "input", default)]
    pub inputs: Vec<MacroInvocationInput>,
    /// Additional-output bindings, in `:`-list declaration order.
    #[serde(rename = "output", default)]
    pub outputs: Vec<MacroInvocationOutput>,
}

impl ToXml<XmlWriter> for MacroInvocation {
    fn write_xml(&self, writer: &mut Writer<XmlWriter>) -> Result<()> {
        let mut attrs: Vec<(&str, &str)> = vec![
            ("module", self.module.as_str()),
            ("macro", self.macro_name.as_str()),
            ("primary-binding", self.primary_binding.as_str()),
            ("primary-output", self.primary_output.as_str()),
        ];
        if let Some(ref doc) = self.primary_doc {
            attrs.push(("primary-doc", doc.as_str()));
        }
        if let Some(ref units) = self.primary_units {
            attrs.push(("primary-units", units.as_str()));
        }
        write_tag_start_with_attrs(writer, "simlin:macro-invocation", &attrs)?;

        for inp in self.inputs.iter() {
            let attrs = &[("from", inp.from.as_str()), ("to", inp.to.as_str())];
            super::write_tag_empty_with_attrs(writer, "simlin:input", attrs)?;
        }
        for out in self.outputs.iter() {
            let attrs = &[
                ("binding", out.binding.as_str()),
                ("output", out.output.as_str()),
            ];
            super::write_tag_empty_with_attrs(writer, "simlin:output", attrs)?;
        }

        write_tag_end(writer, "simlin:macro-invocation")
    }
}

/// A scalar binding `Aux` reading a module output. The module-output
/// reference uses an ASCII period at the datamodel layer (the authoritative
/// Phase-4 Separator convention -- `canonicalize()` converts it to U+00B7
/// only at compile-time parse).
fn binding_aux(
    ident: String,
    module: &str,
    output: &str,
    documentation: String,
    units: Option<String>,
) -> datamodel::Variable {
    datamodel::Variable::Aux(datamodel::Aux {
        ident,
        equation: datamodel::Equation::Scalar(format!("{}.{}", module, output)),
        documentation,
        units,
        gf: None,
        ai_state: None,
        uid: None,
        compat: datamodel::Compat::default(),
    })
}

/// Reconstruct the Phase-4-materialized multi-output invocation cluster from
/// a `simlin:macro-invocation` extension: the input-only `Variable::Module`,
/// the primary-output binding `Aux` (the call-site LHS), and one `Aux` per
/// additional `:`-output. This is the exact inverse of the project-level
/// extraction (`extract_macro_invocation`), so the datamodel shape -- and
/// hence the XMILE round-trip -- is byte-stable.
fn reconstruct_macro_invocation(inv: &MacroInvocation) -> Vec<datamodel::Variable> {
    let mut out: Vec<datamodel::Variable> = Vec::with_capacity(2 + inv.outputs.len());

    let references: Vec<datamodel::ModuleReference> = inv
        .inputs
        .iter()
        .map(|i| datamodel::ModuleReference {
            src: i.from.clone(),
            dst: format!("{}.{}", inv.module, i.to),
        })
        .collect();

    out.push(datamodel::Variable::Module(datamodel::Module {
        ident: inv.module.clone(),
        model_name: inv.macro_name.clone(),
        documentation: String::new(),
        units: None,
        references,
        ai_state: None,
        uid: None,
        compat: datamodel::Compat::default(),
    }));

    // Primary-output binding aux (the call-site LHS), carrying the
    // invocation's preserved doc/units.
    out.push(binding_aux(
        inv.primary_binding.clone(),
        &inv.module,
        &inv.primary_output,
        inv.primary_doc.clone().unwrap_or_default(),
        inv.primary_units.clone(),
    ));

    // One additional-output binding aux per `:`-list entry.
    for o in &inv.outputs {
        out.push(binding_aux(
            o.binding.clone(),
            &inv.module,
            &o.output,
            String::new(),
            None,
        ));
    }

    out
}

/// Extract every Phase-4-materialized multi-output invocation cluster from a
/// `datamodel::Model`, given the project's macro registry
/// (canonical-macro-model-name -> its `MacroSpec`). Returns the residual
/// `datamodel::Model` (the cluster variables removed) plus the extracted
/// invocation records. The residual model is then converted by the ordinary
/// per-model `From<datamodel::Model> for Model` bridge.
///
/// A cluster is recognized structurally: a `Variable::Module` whose
/// `model_name` resolves to a macro-marked model. Its binding auxes are the
/// `Variable::Aux`es whose scalar equation is exactly `"{module}.{output}"`
/// for the macro's primary / additional outputs. Only multi-output macros
/// (non-empty `additional_outputs`) are extracted -- a single-output macro
/// invocation has a plain-text equivalent and is never materialized.
pub(crate) fn extract_macro_invocations(
    mut model: datamodel::Model,
    macro_specs: &std::collections::HashMap<String, datamodel::MacroSpec>,
) -> (datamodel::Model, Vec<MacroInvocation>) {
    use std::collections::HashSet;

    let mut invocations: Vec<MacroInvocation> = Vec::new();
    // Idents of variables that belong to an extracted cluster (the module
    // plus its binding auxes); removed from the residual model.
    let mut consumed: HashSet<String> = HashSet::new();

    // Index the scalar-equation auxes once so a binding lookup is O(1).
    // Maps the (trimmed) scalar equation text to the aux's ident, doc, and
    // units. A binding aux's equation is exactly `{module}.{output}`.
    let mut scalar_auxes: std::collections::HashMap<String, (String, String, Option<String>)> =
        std::collections::HashMap::new();
    for v in &model.variables {
        if let datamodel::Variable::Aux(aux) = v
            && let datamodel::Equation::Scalar(eq) = &aux.equation
        {
            scalar_auxes
                .entry(eq.trim().to_string())
                .or_insert_with(|| {
                    (
                        aux.ident.clone(),
                        aux.documentation.clone(),
                        aux.units.clone(),
                    )
                });
        }
    }

    for v in &model.variables {
        let datamodel::Variable::Module(module) = v else {
            continue;
        };
        let macro_key = canonicalize(&module.model_name).into_owned();
        let Some(spec) = macro_specs.get(&macro_key) else {
            continue;
        };
        // Only *multi-output* macros materialize as a module + bindings;
        // a single-output invocation stays as plain equation text.
        if spec.additional_outputs.is_empty() {
            continue;
        }

        // Input wirings: dst is `{module}.{param}`; strip the `{module}.`
        // prefix to recover the bare formal-parameter name.
        let prefix = format!("{}.", module.ident);
        let inputs: Vec<MacroInvocationInput> = module
            .references
            .iter()
            .map(|r| MacroInvocationInput {
                from: r.src.clone(),
                to: r
                    .dst
                    .strip_prefix(&prefix)
                    .unwrap_or(r.dst.as_str())
                    .to_string(),
            })
            .collect();

        // Primary-output binding: the aux reading `{module}.{primary}`.
        let primary_key = format!("{}.{}", module.ident, spec.primary_output);
        let Some((primary_binding, primary_doc, primary_units)) =
            scalar_auxes.get(&primary_key).cloned()
        else {
            // No primary binding found -- not a materialized cluster we can
            // faithfully round-trip; leave it as a plain <module>.
            continue;
        };

        // One additional-output binding per `:`-list entry.
        let mut outputs: Vec<MacroInvocationOutput> =
            Vec::with_capacity(spec.additional_outputs.len());
        let mut all_found = true;
        for out_name in &spec.additional_outputs {
            let key = format!("{}.{}", module.ident, out_name);
            match scalar_auxes.get(&key) {
                Some((binding, _, _)) => outputs.push(MacroInvocationOutput {
                    binding: binding.clone(),
                    output: out_name.clone(),
                }),
                None => {
                    all_found = false;
                    break;
                }
            }
        }
        if !all_found {
            continue;
        }

        // Mark the module + every binding aux consumed.
        consumed.insert(module.ident.clone());
        consumed.insert(primary_binding.clone());
        for o in &outputs {
            consumed.insert(o.binding.clone());
        }

        invocations.push(MacroInvocation {
            module: module.ident.clone(),
            macro_name: module.model_name.clone(),
            primary_binding,
            primary_output: spec.primary_output.clone(),
            primary_doc: if primary_doc.is_empty() {
                None
            } else {
                Some(primary_doc)
            },
            primary_units,
            inputs,
            outputs,
        });
    }

    if !consumed.is_empty() {
        model
            .variables
            .retain(|v| !consumed.contains(v.get_ident()));
    }

    (model, invocations)
}

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
    /// Vendor extension: named feedback loop metadata.
    /// Serde rename uses local name without prefix (quick-xml strips namespace prefixes).
    #[serde(rename = "loop-metadata", default)]
    pub loop_metadata: Option<Vec<XmileLoopMetadata>>,
    /// Vendor extension: Vensim multi-output (`:`-list) macro invocations.
    /// Each records a Phase-4-materialized cluster (an input-only
    /// `Variable::Module` + binding `Aux`es) so it round-trips faithfully.
    /// Serde rename uses the local name without prefix (quick-xml strips
    /// namespace prefixes).
    #[serde(rename = "macro-invocation", default)]
    pub macro_invocations: Option<Vec<MacroInvocation>>,
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

        if let Some(ref loop_metadata) = self.loop_metadata {
            for lm in loop_metadata {
                let mut attrs: Vec<(&str, &str)> = vec![("name", &lm.name)];
                let deleted_str;
                if lm.deleted == Some(true) {
                    deleted_str = "true".to_string();
                    attrs.push(("deleted", &deleted_str));
                }
                if let Some(ref desc) = lm.description {
                    attrs.push(("description", desc));
                }
                let uids_text = lm.uids_text.as_deref().unwrap_or("");
                write_tag_with_attrs(writer, "simlin:loop-metadata", uids_text, &attrs)?;
            }
        }

        if let Some(ref invocations) = self.macro_invocations {
            for inv in invocations {
                inv.write_xml(writer)?;
            }
        }

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
        let loop_metadata: Vec<datamodel::LoopMetadata> = model
            .loop_metadata
            .unwrap_or_default()
            .into_iter()
            .map(|lm| {
                let uids: Vec<i32> = lm
                    .uids_text
                    .unwrap_or_default()
                    .split(',')
                    .filter_map(|s| s.trim().parse::<i32>().ok())
                    .collect();
                datamodel::LoopMetadata {
                    uids,
                    deleted: lm.deleted.unwrap_or(false),
                    name: lm.name,
                    description: lm.description.unwrap_or_default(),
                }
            })
            .collect();

        let mut variables: Vec<datamodel::Variable> = match model.variables {
            Some(Variables {
                variables: vars, ..
            }) => vars
                .into_iter()
                .filter(|v| !matches!(v, Var::Unhandled))
                .map(datamodel::Variable::from)
                .collect(),
            _ => vec![],
        };

        // Reconstruct each multi-output macro-invocation cluster (the
        // input-only `Variable::Module` + the primary-output binding `Aux` +
        // one `Aux` per additional `:`-output) from its `simlin:`-namespaced
        // extension. This is the exact inverse of the project-level
        // extraction, so an XMILE->datamodel->XMILE round-trip is byte-stable.
        if let Some(ref invocations) = model.macro_invocations {
            for inv in invocations {
                variables.extend(reconstruct_macro_invocation(inv));
            }
        }

        // Sort variables by canonical identifier for deterministic ordering.
        variables.sort_by(|a, b| {
            crate::canonicalize(a.get_ident()).cmp(&crate::canonicalize(b.get_ident()))
        });

        datamodel::Model {
            name: model.name.as_deref().unwrap_or("main").to_string(),
            sim_specs: model.sim_specs.map(datamodel::SimSpecs::from),
            variables,
            views,
            loop_metadata,
            groups,
            macro_spec: None,
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
            loop_metadata,
            groups,
            // XMILE has no macro concept; macro_spec is dropped on export.
            // Macro round-tripping through XMILE is a later phase's concern.
            macro_spec: _,
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

        let xmile_loop_metadata: Option<Vec<XmileLoopMetadata>> = if loop_metadata.is_empty() {
            None
        } else {
            Some(
                loop_metadata
                    .into_iter()
                    .map(|lm| {
                        let uids_text = lm
                            .uids
                            .iter()
                            .map(|uid| uid.to_string())
                            .collect::<Vec<_>>()
                            .join(",");
                        XmileLoopMetadata {
                            name: lm.name,
                            deleted: if lm.deleted { Some(true) } else { None },
                            description: if lm.description.is_empty() {
                                None
                            } else {
                                Some(lm.description)
                            },
                            uids_text: Some(uids_text),
                        }
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
            loop_metadata: xmile_loop_metadata,
            // Populated at the project level (it needs the set of macro
            // model names); this per-model bridge defaults it to None.
            macro_invocations: None,
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
            compat: datamodel::Compat {
                can_be_module_input: can_be_module_input(&module.access),
                visibility: visibility(&module.access),
                ..datamodel::Compat::default()
            },
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
            access: access_from(module.compat.visibility, module.compat.can_be_module_input),
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
            equation: datamodel::Equation::Scalar("1".to_string()),
            documentation: String::new(),
            units: None,
            gf: None,
            ai_state: None,
            uid: None,
            compat: datamodel::Compat::default(),
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
        macro_spec: None,
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
