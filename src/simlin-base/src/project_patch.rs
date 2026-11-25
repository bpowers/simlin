// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! A patch to a project: the operations a host asks for, as data.
//! `patch::apply_patch` carries one out, and `json` is the form it is sent in.

use crate::datamodel;

/// A patch to apply to a project. Contains project-level operations
/// (like changing sim specs or adding models) and per-model patches
/// (like upserting variables or views).
#[derive(Clone)]
pub struct ProjectPatch {
    pub project_ops: Vec<ProjectOperation>,
    pub models: Vec<ModelPatch>,
}

/// A project-level operation.
#[derive(Clone)]
pub enum ProjectOperation {
    /// Sets the sim specs the project runs under.
    ///
    /// A run takes its specs from the model it runs when that model has specs
    /// of its own, and from the project otherwise (XMILE 1.0 section 2.3: a
    /// model's `<sim_specs>` override the file's). So the edit sets the
    /// project's specs and takes the root model's own away: afterwards the
    /// project's are the one statement of what the root model runs under, and
    /// every reader of either agrees with the run. The root model is the one a
    /// host runs when it names none (`Project::default_model`). Another
    /// model's own specs are that model's and stay, so a run of another model
    /// that has its own still runs under them.
    SetSimSpecs(datamodel::SimSpecs),
    SetSource(datamodel::Source),
    AddModel {
        name: String,
    },
    /// Adds a dimension, or sets the elements, mappings and parent of the
    /// declared dimension of its name (`patch_dimensions::upsert_dimension`).
    UpsertDimension(datamodel::Dimension),
    /// Renames a dimension, and every variable and equation that names it.
    RenameDimension {
        from: String,
        to: String,
    },
    /// Renames one element of a dimension, and every key and subscript that
    /// names it.
    RenameElement {
        dimension: String,
        from: String,
        to: String,
    },
    /// Deletes a dimension nothing uses.
    DeleteDimension {
        name: String,
    },
}

/// A patch targeting a specific model within the project.
#[derive(Clone)]
pub struct ModelPatch {
    pub name: String,
    pub ops: Vec<ModelOperation>,
}

/// An operation on a single model.
#[derive(Clone)]
pub enum ModelOperation {
    UpsertStock(datamodel::Stock),
    UpsertFlow(datamodel::Flow),
    UpsertAux(datamodel::Aux),
    UpsertModule(datamodel::Module),
    DeleteVariable {
        ident: String,
    },
    /// Renames a variable and respells every reference to it, in every model
    /// that can name it (its own and every model instantiating that one), and
    /// every other place a model holds its name, but for a diagram's labels:
    /// every view's label keeps naming the old name until a view edit
    /// (`EditView`, as `editing::plan_rename` plans one, or an upserted view)
    /// or a diagram sync that carries the patch relabels it. A host that
    /// draws the model sends the rename that way, or syncs its diagrams after
    /// it.
    ///
    /// Refused, changing nothing, where carrying it out would decide what an
    /// identifier inside a subscript's brackets names, which the compiler
    /// does not answer one way (`patch::Reader`): when the old or the new name
    /// is a dimension's, and when the variable is written alone in a
    /// subscript (or at an end of a range there) and its old or new name is an
    /// element of a dimension. Refused too when the new name is one of the
    /// run's clock slots (`results::is_implicit_global`: the time, the time step,
    /// the initial and the final time): a variable of that name takes the
    /// slot's results key, so a rename does not give a variable one, though
    /// the compiler resolves the quoted name; a model that already declares
    /// one keeps it.
    RenameVariable {
        from: String,
        to: String,
    },
    UpsertView {
        index: u32,
        view: datamodel::View,
    },
    DeleteView {
        index: u32,
    },
    UpdateStockFlows {
        ident: String,
        inflows: Vec<String>,
        outflows: Vec<String>,
    },
    SetLoopName {
        variables: Vec<String>,
        name: String,
        description: Option<String>,
    },
    /// Edit one stock-and-flow view by upserting elements (by uid: substituting
    /// or appending) and removing uids, and apply every model operation the
    /// edit implies -- renames, deletes, creates, stock list updates -- derived
    /// against the model as it is when this operation applies
    /// (`editing::derived_operations`). A diagram host describes an edit only by
    /// the elements it changed; the model follows.
    EditView {
        index: u32,
        upsert: Vec<datamodel::ViewElement>,
        remove: Vec<i32>,
    },
    /// A scalar's equation (a stock's initial value), or the equation every
    /// element of an arrayed variable follows; elements with equations of
    /// their own keep them (`patch_fields::set_equation`).
    SetEquation {
        ident: String,
        equation: String,
    },
    /// One element's own equation (`element` names it on each of the
    /// variable's dimensions), or with `None` none, so the element follows
    /// the equation every element follows again
    /// (`patch_fields::set_element_equation`).
    SetElementEquation {
        ident: String,
        element: Vec<String>,
        equation: Option<String>,
    },
    /// The variable's units; `None` or empty clears them.
    SetUnits {
        ident: String,
        units: Option<String>,
    },
    /// The variable's documentation; empty clears it.
    SetNotes {
        ident: String,
        notes: String,
    },
    /// The variable's lookup table, or one element's; `None` removes it
    /// (`patch_fields::set_lookup`).
    SetLookup {
        ident: String,
        element: Option<Vec<String>>,
        lookup: Option<datamodel::GraphicalFunction>,
    },
    /// The dimensions the variable is arrayed over, in order; none makes it
    /// a scalar. `keep` names the element kept on each dimension removed
    /// (`patch_fields::set_dimensions`).
    SetDimensions {
        ident: String,
        dimensions: Vec<String>,
        keep: Option<Vec<String>>,
    },
}
