// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.
use prost::Message;
use simlin_engine::common::{EquationError, ErrorCode, UnitError};
use simlin_engine::ltm::{detect_loops, LoopPolarity};
use simlin_engine::{self as engine, canonicalize, serde, Vm};
use std::alloc::{alloc, dealloc, Layout};
use std::ffi::{CStr, CString};
use std::io::BufReader;
use std::os::raw::{c_char, c_double, c_int};
use std::ptr;
use std::sync::atomic::{AtomicUsize, Ordering};

mod ffi;
pub use ffi::{
    SimlinErrorDetail, SimlinErrorDetails, SimlinLink, SimlinLinkPolarity, SimlinLinks, SimlinLoop,
    SimlinLoopPolarity, SimlinLoops,
};

/// Error codes for the C API
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SimlinErrorCode {
    /// Success - no error
    NoError = 0,
    DoesNotExist = 1,
    XmlDeserialization = 2,
    VensimConversion = 3,
    ProtobufDecode = 4,
    InvalidToken = 5,
    UnrecognizedEof = 6,
    UnrecognizedToken = 7,
    ExtraToken = 8,
    UnclosedComment = 9,
    UnclosedQuotedIdent = 10,
    ExpectedNumber = 11,
    UnknownBuiltin = 12,
    BadBuiltinArgs = 13,
    EmptyEquation = 14,
    BadModuleInputDst = 15,
    BadModuleInputSrc = 16,
    NotSimulatable = 17,
    BadTable = 18,
    BadSimSpecs = 19,
    NoAbsoluteReferences = 20,
    CircularDependency = 21,
    ArraysNotImplemented = 22,
    MultiDimensionalArraysNotImplemented = 23,
    BadDimensionName = 24,
    BadModelName = 25,
    MismatchedDimensions = 26,
    ArrayReferenceNeedsExplicitSubscripts = 27,
    DuplicateVariable = 28,
    UnknownDependency = 29,
    VariablesHaveErrors = 30,
    UnitDefinitionErrors = 31,
    Generic = 32,
}

impl From<engine::ErrorCode> for SimlinErrorCode {
    fn from(code: engine::ErrorCode) -> Self {
        match code {
            engine::ErrorCode::NoError => SimlinErrorCode::NoError,
            engine::ErrorCode::DoesNotExist => SimlinErrorCode::DoesNotExist,
            engine::ErrorCode::XmlDeserialization => SimlinErrorCode::XmlDeserialization,
            engine::ErrorCode::VensimConversion => SimlinErrorCode::VensimConversion,
            engine::ErrorCode::ProtobufDecode => SimlinErrorCode::ProtobufDecode,
            engine::ErrorCode::InvalidToken => SimlinErrorCode::InvalidToken,
            engine::ErrorCode::UnrecognizedEof => SimlinErrorCode::UnrecognizedEof,
            engine::ErrorCode::UnrecognizedToken => SimlinErrorCode::UnrecognizedToken,
            engine::ErrorCode::ExtraToken => SimlinErrorCode::ExtraToken,
            engine::ErrorCode::UnclosedComment => SimlinErrorCode::UnclosedComment,
            engine::ErrorCode::UnclosedQuotedIdent => SimlinErrorCode::UnclosedQuotedIdent,
            engine::ErrorCode::ExpectedNumber => SimlinErrorCode::ExpectedNumber,
            engine::ErrorCode::UnknownBuiltin => SimlinErrorCode::UnknownBuiltin,
            engine::ErrorCode::BadBuiltinArgs => SimlinErrorCode::BadBuiltinArgs,
            engine::ErrorCode::EmptyEquation => SimlinErrorCode::EmptyEquation,
            engine::ErrorCode::BadModuleInputDst => SimlinErrorCode::BadModuleInputDst,
            engine::ErrorCode::BadModuleInputSrc => SimlinErrorCode::BadModuleInputSrc,
            engine::ErrorCode::NotSimulatable => SimlinErrorCode::NotSimulatable,
            engine::ErrorCode::BadTable => SimlinErrorCode::BadTable,
            engine::ErrorCode::BadSimSpecs => SimlinErrorCode::BadSimSpecs,
            engine::ErrorCode::NoAbsoluteReferences => SimlinErrorCode::NoAbsoluteReferences,
            engine::ErrorCode::CircularDependency => SimlinErrorCode::CircularDependency,
            engine::ErrorCode::ArraysNotImplemented => SimlinErrorCode::ArraysNotImplemented,
            engine::ErrorCode::MultiDimensionalArraysNotImplemented => {
                SimlinErrorCode::MultiDimensionalArraysNotImplemented
            }
            engine::ErrorCode::BadDimensionName => SimlinErrorCode::BadDimensionName,
            engine::ErrorCode::BadModelName => SimlinErrorCode::BadModelName,
            engine::ErrorCode::MismatchedDimensions => SimlinErrorCode::MismatchedDimensions,
            engine::ErrorCode::ArrayReferenceNeedsExplicitSubscripts => {
                SimlinErrorCode::ArrayReferenceNeedsExplicitSubscripts
            }
            engine::ErrorCode::DuplicateVariable => SimlinErrorCode::DuplicateVariable,
            engine::ErrorCode::UnknownDependency => SimlinErrorCode::UnknownDependency,
            engine::ErrorCode::VariablesHaveErrors => SimlinErrorCode::VariablesHaveErrors,
            engine::ErrorCode::UnitDefinitionErrors => SimlinErrorCode::UnitDefinitionErrors,
            engine::ErrorCode::Generic => SimlinErrorCode::Generic,
            engine::ErrorCode::NoAppInUnits => SimlinErrorCode::Generic,
            engine::ErrorCode::NoSubscriptInUnits => SimlinErrorCode::Generic,
            engine::ErrorCode::NoIfInUnits => SimlinErrorCode::Generic,
            engine::ErrorCode::NoUnaryOpInUnits => SimlinErrorCode::Generic,
            engine::ErrorCode::BadBinaryOpInUnits => SimlinErrorCode::Generic,
            engine::ErrorCode::NoConstInUnits => SimlinErrorCode::Generic,
            engine::ErrorCode::ExpectedInteger => SimlinErrorCode::Generic,
            engine::ErrorCode::ExpectedIntegerOne => SimlinErrorCode::Generic,
            engine::ErrorCode::DuplicateUnit => SimlinErrorCode::Generic,
            engine::ErrorCode::ExpectedModule => SimlinErrorCode::Generic,
            engine::ErrorCode::ExpectedIdent => SimlinErrorCode::Generic,
            engine::ErrorCode::UnitMismatch => SimlinErrorCode::Generic,
            engine::ErrorCode::TodoWildcard => SimlinErrorCode::Generic,
            engine::ErrorCode::TodoStarRange => SimlinErrorCode::Generic,
            engine::ErrorCode::TodoRange => SimlinErrorCode::Generic,
            engine::ErrorCode::TodoArrayBuiltin => SimlinErrorCode::Generic,
            engine::ErrorCode::CantSubscriptScalar => SimlinErrorCode::Generic,
            engine::ErrorCode::DimensionInScalarContext => SimlinErrorCode::Generic,
        }
    }
}
/// Opaque project structure
pub struct SimlinProject {
    project: engine::Project,
    ref_count: AtomicUsize,
}
/// Opaque model structure
pub struct SimlinModel {
    project: *const SimlinProject,
    model_name: String,
    ref_count: AtomicUsize,
}
/// Opaque simulation structure
pub struct SimlinSim {
    model: *const SimlinModel,
    enable_ltm: bool,
    vm: Option<Vm>,
    results: Option<engine::Results>,
    ref_count: AtomicUsize,
}
/// simlin_error_str returns a string representation of an error code.
/// The returned string must not be freed or modified.
#[no_mangle]
pub extern "C" fn simlin_error_str(err: c_int) -> *const c_char {
    // Map an engine::ErrorCode discriminant to its string form.
    // Unknown values map to "unknown_error".
    let s: &'static str = match err {
        x if x == engine::ErrorCode::NoError as c_int => "no_error\0",
        x if x == engine::ErrorCode::DoesNotExist as c_int => "does_not_exist\0",
        x if x == engine::ErrorCode::XmlDeserialization as c_int => "xml_deserialization\0",
        x if x == engine::ErrorCode::VensimConversion as c_int => "vensim_conversion\0",
        x if x == engine::ErrorCode::ProtobufDecode as c_int => "protobuf_decode\0",
        x if x == engine::ErrorCode::InvalidToken as c_int => "invalid_token\0",
        x if x == engine::ErrorCode::UnrecognizedEof as c_int => "unrecognized_eof\0",
        x if x == engine::ErrorCode::UnrecognizedToken as c_int => "unrecognized_token\0",
        x if x == engine::ErrorCode::ExtraToken as c_int => "extra_token\0",
        x if x == engine::ErrorCode::UnclosedComment as c_int => "unclosed_comment\0",
        x if x == engine::ErrorCode::UnclosedQuotedIdent as c_int => "unclosed_quoted_ident\0",
        x if x == engine::ErrorCode::ExpectedNumber as c_int => "expected_number\0",
        x if x == engine::ErrorCode::UnknownBuiltin as c_int => "unknown_builtin\0",
        x if x == engine::ErrorCode::BadBuiltinArgs as c_int => "bad_builtin_args\0",
        x if x == engine::ErrorCode::EmptyEquation as c_int => "empty_equation\0",
        x if x == engine::ErrorCode::BadModuleInputDst as c_int => "bad_module_input_dst\0",
        x if x == engine::ErrorCode::BadModuleInputSrc as c_int => "bad_module_input_src\0",
        x if x == engine::ErrorCode::NotSimulatable as c_int => "not_simulatable\0",
        x if x == engine::ErrorCode::BadTable as c_int => "bad_table\0",
        x if x == engine::ErrorCode::BadSimSpecs as c_int => "bad_sim_specs\0",
        x if x == engine::ErrorCode::NoAbsoluteReferences as c_int => "no_absolute_references\0",
        x if x == engine::ErrorCode::CircularDependency as c_int => "circular_dependency\0",
        x if x == engine::ErrorCode::ArraysNotImplemented as c_int => "arrays_not_implemented\0",
        x if x == engine::ErrorCode::MultiDimensionalArraysNotImplemented as c_int => {
            "multi_dimensional_arrays_not_implemented\0"
        }
        x if x == engine::ErrorCode::BadDimensionName as c_int => "bad_dimension_name\0",
        x if x == engine::ErrorCode::BadModelName as c_int => "bad_model_name\0",
        x if x == engine::ErrorCode::MismatchedDimensions as c_int => "mismatched_dimensions\0",
        x if x == engine::ErrorCode::ArrayReferenceNeedsExplicitSubscripts as c_int => {
            "array_reference_needs_explicit_subscripts\0"
        }
        x if x == engine::ErrorCode::DuplicateVariable as c_int => "duplicate_variable\0",
        x if x == engine::ErrorCode::UnknownDependency as c_int => "unknown_dependency\0",
        x if x == engine::ErrorCode::VariablesHaveErrors as c_int => "variables_have_errors\0",
        x if x == engine::ErrorCode::UnitDefinitionErrors as c_int => "unit_definition_errors\0",
        x if x == engine::ErrorCode::Generic as c_int => "generic\0",
        x if x == engine::ErrorCode::NoAppInUnits as c_int => "no_app_in_units\0",
        x if x == engine::ErrorCode::NoSubscriptInUnits as c_int => "no_subscript_in_units\0",
        x if x == engine::ErrorCode::NoIfInUnits as c_int => "no_if_in_units\0",
        x if x == engine::ErrorCode::NoUnaryOpInUnits as c_int => "no_unary_op_in_units\0",
        x if x == engine::ErrorCode::BadBinaryOpInUnits as c_int => "bad_binary_op_in_units\0",
        x if x == engine::ErrorCode::NoConstInUnits as c_int => "no_const_in_units\0",
        x if x == engine::ErrorCode::ExpectedInteger as c_int => "expected_integer\0",
        x if x == engine::ErrorCode::ExpectedIntegerOne as c_int => "expected_integer_one\0",
        x if x == engine::ErrorCode::DuplicateUnit as c_int => "duplicate_unit\0",
        x if x == engine::ErrorCode::ExpectedModule as c_int => "expected_module\0",
        x if x == engine::ErrorCode::ExpectedIdent as c_int => "expected_ident\0",
        x if x == engine::ErrorCode::UnitMismatch as c_int => "unit_mismatch\0",
        x if x == engine::ErrorCode::TodoWildcard as c_int => "todo_wildcard\0",
        x if x == engine::ErrorCode::TodoStarRange as c_int => "todo_star_range\0",
        x if x == engine::ErrorCode::TodoRange as c_int => "todo_range\0",
        x if x == engine::ErrorCode::TodoArrayBuiltin as c_int => "todo_array_builtin\0",
        x if x == engine::ErrorCode::CantSubscriptScalar as c_int => "cant_subscript_scalar\0",
        x if x == engine::ErrorCode::DimensionInScalarContext as c_int => {
            "dimension_in_scalar_context\0"
        }
        _ => "unknown_error\0",
    };
    s.as_ptr() as *const c_char
}
/// simlin_project_open opens a project from protobuf data.
/// If an error occurs, the function returns NULL and if the err parameter
/// is not NULL, details of the error are placed in it.
///
/// # Safety
/// - `data` must be a valid pointer to at least `len` bytes
/// - `err` may be null
#[no_mangle]
pub unsafe extern "C" fn simlin_project_open(
    data: *const u8,
    len: usize,
    err: *mut c_int,
) -> *mut SimlinProject {
    if data.is_null() {
        if !err.is_null() {
            *err = engine::ErrorCode::Generic as c_int;
        }
        return ptr::null_mut();
    }
    let slice = std::slice::from_raw_parts(data, len);
    // Only accept full Project protobufs
    let project: engine::Project = match engine::project_io::Project::decode(slice) {
        Ok(pb_project) => serde::deserialize(pb_project).into(),
        Err(_) => {
            if !err.is_null() {
                *err = engine::ErrorCode::ProtobufDecode as c_int;
            }
            return ptr::null_mut();
        }
    };
    let boxed = Box::new(SimlinProject {
        project,
        ref_count: AtomicUsize::new(1),
    });
    if !err.is_null() {
        *err = engine::ErrorCode::NoError as c_int;
    }
    Box::into_raw(boxed)
}
/// Increments the reference count of a project
///
/// # Safety
/// - `project` must be a valid pointer to a SimlinProject
#[no_mangle]
pub unsafe extern "C" fn simlin_project_ref(project: *mut SimlinProject) {
    if !project.is_null() {
        (*project).ref_count.fetch_add(1, Ordering::SeqCst);
    }
}
/// Decrements the reference count and frees the project if it reaches zero
///
/// # Safety
/// - `project` must be a valid pointer to a SimlinProject
#[no_mangle]
pub unsafe extern "C" fn simlin_project_unref(project: *mut SimlinProject) {
    if project.is_null() {
        return;
    }
    let prev_count = (*project).ref_count.fetch_sub(1, Ordering::SeqCst);
    if prev_count == 1 {
        std::sync::atomic::fence(Ordering::SeqCst);
        let _ = Box::from_raw(project);
    }
}

/// Gets the number of models in the project
///
/// # Safety
/// - `project` must be a valid pointer to a SimlinProject
#[no_mangle]
pub unsafe extern "C" fn simlin_project_get_model_count(project: *mut SimlinProject) -> c_int {
    if project.is_null() {
        return 0;
    }
    (*project).project.datamodel.models.len() as c_int
}

/// Gets the list of model names in the project
///
/// # Safety
/// - `project` must be a valid pointer to a SimlinProject
/// - `result` must be a valid pointer to an array of at least `max` char pointers
/// - The returned strings are owned by the caller and must be freed with simlin_free_string
#[no_mangle]
pub unsafe extern "C" fn simlin_project_get_model_names(
    project: *mut SimlinProject,
    result: *mut *mut c_char,
    max: usize,
) -> c_int {
    if project.is_null() || result.is_null() {
        return engine::ErrorCode::Generic as c_int;
    }

    let proj = &*project;
    let models = &proj.project.datamodel.models;
    let count = models.len().min(max);

    for (i, model) in models.iter().take(count).enumerate() {
        let c_string = match CString::new(model.name.clone()) {
            Ok(s) => s,
            Err(_) => return engine::ErrorCode::Generic as c_int,
        };
        *result.add(i) = c_string.into_raw();
    }

    count as c_int
}

/// Adds a new model to a project
///
/// Creates a new empty model with the given name and adds it to the project.
/// The model will have no variables initially.
///
/// # Safety
/// - `project` must be a valid pointer to a SimlinProject
/// - `model_name` must be a valid C string
///
/// # Returns
/// - 0 on success
/// - SimlinErrorCode::Generic if project or model_name is null or empty
/// - SimlinErrorCode::DuplicateVariable if a model with that name already exists
#[no_mangle]
pub unsafe extern "C" fn simlin_project_add_model(
    project: *mut SimlinProject,
    model_name: *const c_char,
) -> c_int {
    if project.is_null() || model_name.is_null() {
        return engine::ErrorCode::Generic as c_int;
    }

    let model_name_str = match CStr::from_ptr(model_name).to_str() {
        Ok(s) if !s.is_empty() => s,
        _ => return engine::ErrorCode::Generic as c_int,
    };

    let proj = &mut *project;

    // Check if model already exists
    for model in &proj.project.datamodel.models {
        if model.name == model_name_str {
            return engine::ErrorCode::DuplicateVariable as c_int;
        }
    }

    // Create new empty model
    let new_model = engine::datamodel::Model {
        name: model_name_str.to_string(),
        variables: vec![],
        views: vec![],
        loop_metadata: vec![],
    };

    // Add to datamodel
    proj.project.datamodel.models.push(new_model);

    // Rebuild the project's internal structures
    proj.project = engine::Project::from(proj.project.datamodel.clone());

    engine::ErrorCode::NoError as c_int
}

/// Gets a model from a project by name
///
/// # Safety
/// - `project` must be a valid pointer to a SimlinProject
/// - `model_name` may be null (uses default model)
/// - The returned model must be freed with simlin_model_unref
#[no_mangle]
pub unsafe extern "C" fn simlin_project_get_model(
    project: *mut SimlinProject,
    model_name: *const c_char,
) -> *mut SimlinModel {
    if project.is_null() {
        return ptr::null_mut();
    }

    let mut model_name = if model_name.is_null() {
        None
    } else {
        match CStr::from_ptr(model_name).to_str() {
            Ok(s) => Some(s.to_string()),
            Err(_) => return ptr::null_mut(),
        }
    };

    // If no model name specified or model doesn't exist, use first model
    let proj = &*project;
    if model_name.is_none()
        || proj
            .project
            .datamodel
            .get_model(model_name.as_deref().unwrap())
            .is_none()
    {
        if proj.project.datamodel.models.is_empty() {
            return ptr::null_mut();
        }
        model_name = Some(proj.project.datamodel.models[0].name.clone());
    }

    // Increment project ref count since model holds a reference
    simlin_project_ref(project);

    let model = SimlinModel {
        project,
        model_name: model_name.unwrap(),
        ref_count: AtomicUsize::new(1),
    };

    Box::into_raw(Box::new(model))
}

/// Increments the reference count of a model
///
/// # Safety
/// - `model` must be a valid pointer to a SimlinModel
#[no_mangle]
pub unsafe extern "C" fn simlin_model_ref(model: *mut SimlinModel) {
    if !model.is_null() {
        (*model).ref_count.fetch_add(1, Ordering::SeqCst);
    }
}

/// Decrements the reference count and frees the model if it reaches zero
///
/// # Safety
/// - `model` must be a valid pointer to a SimlinModel
#[no_mangle]
pub unsafe extern "C" fn simlin_model_unref(model: *mut SimlinModel) {
    if model.is_null() {
        return;
    }
    let prev_count = (*model).ref_count.fetch_sub(1, Ordering::SeqCst);
    if prev_count == 1 {
        std::sync::atomic::fence(Ordering::SeqCst);
        let model = Box::from_raw(model);
        // Decrement project reference count
        simlin_project_unref(model.project as *mut SimlinProject);
    }
}

/// Gets the number of variables in the model
///
/// # Safety
/// - `model` must be a valid pointer to a SimlinModel
#[no_mangle]
pub unsafe extern "C" fn simlin_model_get_var_count(model: *mut SimlinModel) -> c_int {
    if model.is_null() {
        return -1;
    }
    let model = &*model;
    let project = &(*model.project).project;

    // Calculate offsets to get variable count
    let offsets = engine::interpreter::calc_flattened_offsets(project, &model.model_name);
    offsets.len() as c_int
}

/// Gets the variable names from the model
///
/// # Safety
/// - `model` must be a valid pointer to a SimlinModel
/// - `result` must be a valid pointer to an array of at least `max` char pointers
/// - The returned strings are owned by the caller and must be freed with simlin_free_string
#[no_mangle]
pub unsafe extern "C" fn simlin_model_get_var_names(
    model: *mut SimlinModel,
    result: *mut *mut c_char,
    max: usize,
) -> c_int {
    if model.is_null() || result.is_null() {
        return engine::ErrorCode::Generic as c_int;
    }

    let model = &*model;
    let project = &(*model.project).project;

    // Calculate offsets to get variable names
    let offsets = engine::interpreter::calc_flattened_offsets(project, &model.model_name);
    let count = offsets.len().min(max);

    let mut names: Vec<_> = offsets.keys().collect();
    names.sort();

    for (i, name) in names.iter().take(count).enumerate() {
        let c_string = match CString::new(name.as_str()) {
            Ok(s) => s,
            Err(_) => return engine::ErrorCode::Generic as c_int,
        };
        *result.add(i) = c_string.into_raw();
    }

    count as c_int
}

/// Gets the incoming links (dependencies) for a variable
///
/// # Safety
/// - `model` must be a valid pointer to a SimlinModel
/// - `var_name` must be a valid C string
/// - `result` must be a valid pointer to an array of at least `max` char pointers (or null if max is 0)
/// - The returned strings are owned by the caller and must be freed with simlin_free_string
///
/// # Returns
/// - If max == 0: returns the total number of dependencies (result can be null)
/// - If max is too small: returns a negative error code
/// - Otherwise: returns the number of dependencies written to result
#[no_mangle]
pub unsafe extern "C" fn simlin_model_get_incoming_links(
    model: *mut SimlinModel,
    var_name: *const c_char,
    result: *mut *mut c_char,
    max: usize,
) -> c_int {
    if model.is_null() || var_name.is_null() {
        return engine::ErrorCode::Generic as c_int;
    }

    let model = &*model;
    let project = &(*model.project).project;

    let var_name = match CStr::from_ptr(var_name).to_str() {
        Ok(s) => canonicalize(s),
        Err(_) => return engine::ErrorCode::Generic as c_int,
    };

    // Get the model from the project
    let eng_model = match project.models.get(&canonicalize(&model.model_name)) {
        Some(m) => m,
        None => return engine::ErrorCode::BadModelName as c_int,
    };

    // Get the variable to find its dependencies
    let var = match eng_model.variables.get(&var_name) {
        Some(v) => v,
        None => return engine::ErrorCode::DoesNotExist as c_int,
    };

    // Get dependencies based on variable type
    let deps = match var {
        engine::Variable::Stock { init_ast, .. } => {
            if let Some(ast) = init_ast {
                engine::identifier_set(ast, &[], None)
            } else {
                std::collections::HashSet::new()
            }
        }
        engine::Variable::Var { ast, .. } => {
            if let Some(ast) = ast {
                engine::identifier_set(ast, &[], None)
            } else {
                std::collections::HashSet::new()
            }
        }
        engine::Variable::Module { inputs, .. } => {
            inputs.iter().map(|input| input.src.clone()).collect()
        }
    };

    // If max is 0, just return the count
    if max == 0 {
        return deps.len() as c_int;
    }

    // If result is null but max is not 0, error
    if result.is_null() {
        return engine::ErrorCode::Generic as c_int;
    }

    // If max is smaller than the number of dependencies, error
    if max < deps.len() {
        return engine::ErrorCode::Generic as c_int;
    }

    // Copy the dependency names to the result array
    for (i, dep) in deps.iter().enumerate() {
        let c_string = match CString::new(dep.as_str()) {
            Ok(s) => s,
            Err(_) => return engine::ErrorCode::Generic as c_int,
        };
        *result.add(i) = c_string.into_raw();
    }

    deps.len() as c_int
}

/// Gets all causal links in a model
///
/// Returns all causal links detected in the model.
/// This includes flow-to-stock, stock-to-flow, and auxiliary-to-auxiliary links.
///
/// # Safety
/// - `model` must be a valid pointer to a SimlinModel
/// - The returned SimlinLinks must be freed with simlin_free_links
#[no_mangle]
pub unsafe extern "C" fn simlin_model_get_links(model: *mut SimlinModel) -> *mut SimlinLinks {
    if model.is_null() {
        return ptr::null_mut();
    }

    let model_ref = &*model;
    let project = &(*model_ref.project).project;

    // Get the model
    let eng_model = match project.models.get(&canonicalize(&model_ref.model_name)) {
        Some(m) => m,
        None => return ptr::null_mut(),
    };

    // Collect all unique links (de-duplicate based on from-to pairs)
    let mut unique_links = std::collections::HashMap::new();
    for (var_name, var) in &eng_model.variables {
        let deps = match var {
            engine::Variable::Stock {
                inflows, outflows, ..
            } => {
                let mut deps = Vec::new();
                for flow in inflows.iter().chain(outflows.iter()) {
                    deps.push((flow.clone(), var_name.clone()));
                }
                deps
            }
            engine::Variable::Var { ast, .. } if ast.is_some() => {
                let deps = engine::identifier_set(ast.as_ref().unwrap(), &[], None);
                deps.into_iter()
                    .map(|dep| (dep, var_name.clone()))
                    .collect()
            }
            engine::Variable::Module { inputs, .. } => inputs
                .iter()
                .map(|input| (input.src.clone(), var_name.clone()))
                .collect(),
            _ => Vec::new(),
        };

        for (from, to) in deps {
            let key = (from.clone(), to.clone());
            unique_links.entry(key).or_insert(engine::ltm::Link {
                from,
                to,
                polarity: engine::ltm::LinkPolarity::Unknown,
            });
        }
    }

    if unique_links.is_empty() {
        return Box::into_raw(Box::new(SimlinLinks {
            links: ptr::null_mut(),
            count: 0,
        }));
    }

    // Convert to C structures (without LTM scores since this is model-level)
    let mut c_links = Vec::with_capacity(unique_links.len());
    for (_, link) in unique_links {
        let c_link = SimlinLink {
            from: str_to_c_ptr(link.from.as_str()),
            to: str_to_c_ptr(link.to.as_str()),
            polarity: match link.polarity {
                engine::ltm::LinkPolarity::Positive => SimlinLinkPolarity::Positive,
                engine::ltm::LinkPolarity::Negative => SimlinLinkPolarity::Negative,
                engine::ltm::LinkPolarity::Unknown => SimlinLinkPolarity::Unknown,
            },
            score: ptr::null_mut(),
            score_len: 0,
        };
        c_links.push(c_link);
    }

    let links = Box::new(SimlinLinks {
        links: c_links.as_mut_ptr(),
        count: c_links.len(),
    });
    std::mem::forget(c_links);
    Box::into_raw(links)
}

/// Helper function to create a VM for a given project and model
fn create_vm(project: &engine::Project, model_name: &str) -> Result<Vm, engine::Error> {
    let compiler = engine::Simulation::new(project, model_name)?;
    let compiled = compiler.compile()?;
    Vm::new(compiled)
}

/// Creates a new simulation context
///
/// # Safety
/// - `model` must be a valid pointer to a SimlinModel
#[no_mangle]
pub unsafe extern "C" fn simlin_sim_new(
    model: *mut SimlinModel,
    enable_ltm: bool,
) -> *mut SimlinSim {
    if model.is_null() {
        return ptr::null_mut();
    }

    let model = &*model;
    let project = &*model.project;

    // Increment model reference count
    simlin_model_ref(model as *const _ as *mut _);

    let mut sim = Box::new(SimlinSim {
        model: model as *const _,
        enable_ltm,
        vm: None,
        results: None,
        ref_count: AtomicUsize::new(1),
    });

    // Get the appropriate project based on LTM setting
    let proj_result = if enable_ltm {
        project.project.clone().with_ltm()
    } else {
        Ok(project.project.clone())
    };

    let proj_to_use = match proj_result {
        Ok(proj) => proj,
        Err(_) => {
            // Failed to create LTM project
            simlin_model_unref(model as *const _ as *mut _);
            return ptr::null_mut();
        }
    };

    // Initialize the VM
    sim.vm = create_vm(&proj_to_use, &model.model_name).ok();

    Box::into_raw(sim)
}
/// Increments the reference count of a simulation
///
/// # Safety
/// - `sim` must be a valid pointer to a SimlinSim
#[no_mangle]
pub unsafe extern "C" fn simlin_sim_ref(sim: *mut SimlinSim) {
    if !sim.is_null() {
        (*sim).ref_count.fetch_add(1, Ordering::SeqCst);
    }
}
/// Decrements the reference count and frees the simulation if it reaches zero
///
/// # Safety
/// - `sim` must be a valid pointer to a SimlinSim
#[no_mangle]
pub unsafe extern "C" fn simlin_sim_unref(sim: *mut SimlinSim) {
    if sim.is_null() {
        return;
    }
    let prev_count = (*sim).ref_count.fetch_sub(1, Ordering::SeqCst);
    if prev_count == 1 {
        std::sync::atomic::fence(Ordering::SeqCst);
        let sim = Box::from_raw(sim);
        // Decrement model reference count
        simlin_model_unref(sim.model as *mut SimlinModel);
    }
}
/// Runs the simulation to a specified time
///
/// # Safety
/// - `sim` must be a valid pointer to a SimlinSim
#[no_mangle]
pub unsafe extern "C" fn simlin_sim_run_to(sim: *mut SimlinSim, time: c_double) -> c_int {
    if sim.is_null() {
        return engine::ErrorCode::Generic as c_int;
    }
    let sim = &mut *sim;
    if let Some(ref mut vm) = sim.vm {
        match vm.run_to(time) {
            Ok(_) => engine::ErrorCode::NoError as c_int,
            Err(e) => e.code as c_int,
        }
    } else {
        engine::ErrorCode::Generic as c_int
    }
}
/// Runs the simulation to completion
///
/// # Safety
/// - `sim` must be a valid pointer to a SimlinSim
#[no_mangle]
pub unsafe extern "C" fn simlin_sim_run_to_end(sim: *mut SimlinSim) -> c_int {
    if sim.is_null() {
        return engine::ErrorCode::Generic as c_int;
    }
    let sim = &mut *sim;
    if let Some(mut vm) = sim.vm.take() {
        match vm.run_to_end() {
            Ok(_) => {
                sim.results = Some(vm.into_results());
                engine::ErrorCode::NoError as c_int
            }
            Err(e) => {
                sim.vm = Some(vm);
                e.code as c_int
            }
        }
    } else if sim.results.is_some() {
        // Already ran to completion
        engine::ErrorCode::NoError as c_int
    } else {
        engine::ErrorCode::Generic as c_int
    }
}
/// Gets the number of time steps in the results
///
/// # Safety
/// - `sim` must be a valid pointer to a SimlinSim
#[no_mangle]
pub unsafe extern "C" fn simlin_sim_get_stepcount(sim: *mut SimlinSim) -> c_int {
    if sim.is_null() {
        return -1;
    }
    let sim = &*sim;
    if let Some(ref results) = sim.results {
        results.step_count as c_int
    } else {
        -1
    }
}
/// Resets the simulation to its initial state
///
/// # Safety
/// - `sim` must be a valid pointer to a SimlinSim
#[no_mangle]
pub unsafe extern "C" fn simlin_sim_reset(sim: *mut SimlinSim) -> c_int {
    if sim.is_null() {
        return engine::ErrorCode::Generic as c_int;
    }
    let sim = &mut *sim;
    // Clear results
    sim.results = None;

    let model = &*sim.model;
    let project = &*model.project;

    // Re-create the VM - use appropriate project based on LTM setting
    let proj_result = if sim.enable_ltm {
        project.project.clone().with_ltm()
    } else {
        Ok(project.project.clone())
    };

    let proj_to_use = match proj_result {
        Ok(proj) => proj,
        Err(e) => {
            return e.code as c_int;
        }
    };
    match create_vm(&proj_to_use, &model.model_name) {
        Ok(vm) => {
            sim.vm = Some(vm);
            engine::ErrorCode::NoError as c_int
        }
        Err(e) => e.code as c_int,
    }
}
/// Gets a single value from the simulation
///
/// # Safety
/// - `sim` must be a valid pointer to a SimlinSim
/// - `name` must be a valid C string
/// - `result` must be a valid pointer to a double
#[no_mangle]
pub unsafe extern "C" fn simlin_sim_get_value(
    sim: *mut SimlinSim,
    name: *const c_char,
    result: *mut c_double,
) -> c_int {
    if sim.is_null() || name.is_null() || result.is_null() {
        return engine::ErrorCode::Generic as c_int;
    }
    let sim = &*sim;
    let canon_name = match CStr::from_ptr(name).to_str() {
        Ok(s) => canonicalize(s),
        Err(_) => return engine::ErrorCode::Generic as c_int,
    };
    if let Some(ref vm) = sim.vm {
        // Get current value from VM current timestep
        if let Some(off) = vm.get_offset(&canon_name) {
            *result = vm.get_value_now(off);
            return engine::ErrorCode::NoError as c_int;
        }
        engine::ErrorCode::Generic as c_int
    } else if let Some(ref results) = sim.results {
        // Prefer exact canonical match; fall back to suffix match
        if let Some(&offset) = results.offsets.get(&canon_name) {
            if let Some(last_row) = results.iter().next_back() {
                *result = last_row[offset];
                engine::ErrorCode::NoError as c_int
            } else {
                engine::ErrorCode::Generic as c_int
            }
        } else {
            engine::ErrorCode::Generic as c_int
        }
    } else {
        engine::ErrorCode::Generic as c_int
    }
}
/// Sets a value in the simulation
///
/// This function sets values at different phases of simulation:
/// - Before first run_to: Sets initial value to be used when simulation starts
/// - During simulation (after run_to): Sets value in current data for next iteration
/// - After run_to_end: Returns error (simulation complete)
///
/// # Safety
/// - `sim` must be a valid pointer to a SimlinSim
/// - `name` must be a valid C string
#[no_mangle]
pub unsafe extern "C" fn simlin_sim_set_value(
    sim: *mut SimlinSim,
    name: *const c_char,
    val: c_double,
) -> c_int {
    if sim.is_null() || name.is_null() {
        return engine::ErrorCode::Generic as c_int;
    }
    let sim = &mut *sim;
    let canon_name = match CStr::from_ptr(name).to_str() {
        Ok(s) => canonicalize(s),
        Err(_) => return engine::ErrorCode::Generic as c_int,
    };

    if let Some(ref mut vm) = sim.vm {
        // VM exists - either before first run or during simulation
        if let Some(off) = vm.get_offset(&canon_name) {
            // Set value in current timestep (works for both initial and running phases)
            vm.set_value_now(off, val);
            return engine::ErrorCode::NoError as c_int;
        }
        // Variable not found
        return engine::ErrorCode::UnknownDependency as c_int;
    } else if sim.results.is_some() {
        // Simulation complete - cannot modify
        return engine::ErrorCode::NotSimulatable as c_int;
    }

    // No VM and no results - unexpected state
    engine::ErrorCode::Generic as c_int
}
/// Sets the value for a variable at the last saved timestep by offset
///
/// # Safety
/// - `sim` must be a valid pointer to a SimlinSim
#[no_mangle]
pub unsafe extern "C" fn simlin_sim_set_value_by_offset(
    sim: *mut SimlinSim,
    offset: usize,
    val: c_double,
) -> c_int {
    if sim.is_null() {
        return engine::ErrorCode::Generic as c_int;
    }
    let sim = &mut *sim;
    if let Some(ref mut results) = sim.results {
        if results.step_count == 0 || offset >= results.step_size {
            return engine::ErrorCode::Generic as c_int;
        }
        let idx = (results.step_count - 1) * results.step_size + offset;
        if let Some(slot) = results.data.get_mut(idx) {
            *slot = val;
            return engine::ErrorCode::NoError as c_int;
        }
    }
    engine::ErrorCode::Generic as c_int
}
/// Gets the column offset for a variable by name
///
/// Returns the column offset for a variable name at the current context, or -1 if not found.
/// This canonicalizes the name and resolves in the VM if present, otherwise in results.
/// Intended for debugging/tests to verify name→offset resolution.
///
/// # Safety
/// - `sim` must be a valid pointer to a SimlinSim
/// - `name` must be a valid C string
#[no_mangle]
pub unsafe extern "C" fn simlin_sim_get_offset(sim: *mut SimlinSim, name: *const c_char) -> c_int {
    if sim.is_null() || name.is_null() {
        return -1;
    }
    let sim = &*sim;
    let canon_name = match CStr::from_ptr(name).to_str() {
        Ok(s) => canonicalize(s),
        Err(_) => return -1,
    };
    if let Some(ref vm) = sim.vm {
        if let Some(off) = vm.get_offset(&canon_name) {
            return off as c_int;
        }
        return -1;
    }
    if let Some(ref results) = sim.results {
        if let Some(&off) = results.offsets.get(&canon_name) {
            return off as c_int;
        }
        return -1;
    }
    -1
}
/// Gets a time series for a variable
///
/// # Safety
/// - `sim` must be a valid pointer to a SimlinSim
/// - `name` must be a valid C string
/// - `results_ptr` must point to allocated memory of at least `len` doubles
#[no_mangle]
pub unsafe extern "C" fn simlin_sim_get_series(
    sim: *mut SimlinSim,
    name: *const c_char,
    results_ptr: *mut c_double,
    len: usize,
) -> c_int {
    if sim.is_null() || name.is_null() || results_ptr.is_null() {
        return -1;
    }
    let sim = &*sim;
    let name = match CStr::from_ptr(name).to_str() {
        Ok(s) => canonicalize(s),
        Err(_) => return -1,
    };
    if let Some(ref results) = sim.results {
        if let Some(&offset) = results.offsets.get(&name) {
            let count = std::cmp::min(results.step_count, len);
            for (i, row) in results.iter().take(count).enumerate() {
                *results_ptr.add(i) = row[offset];
            }
            0 // Return 0 for success
        } else {
            engine::ErrorCode::DoesNotExist as c_int
        }
    } else {
        engine::ErrorCode::Generic as c_int
    }
}
/// Frees a string returned by the API
///
/// # Safety
/// - `s` must be a valid pointer returned by simlin API functions that return strings
#[no_mangle]
pub unsafe extern "C" fn simlin_free_string(s: *mut c_char) {
    if !s.is_null() {
        let _ = CString::from_raw(s);
    }
}
/// Gets all feedback loops in the project
///
/// # Safety
/// - `project` must be a valid pointer to a SimlinProject
/// - The returned SimlinLoops must be freed with simlin_free_loops
#[no_mangle]
pub unsafe extern "C" fn simlin_analyze_get_loops(project: *mut SimlinProject) -> *mut SimlinLoops {
    if project.is_null() {
        return ptr::null_mut();
    }
    let project = &(*project).project;
    // Detect loops in the project
    let loops_by_model = match detect_loops(project) {
        Ok(loops) => loops,
        Err(_) => return ptr::null_mut(),
    };
    // Collect all loops from all models
    let mut all_loops = Vec::new();
    for (_model_name, model_loops) in loops_by_model {
        all_loops.extend(model_loops);
    }
    if all_loops.is_empty() {
        // Return empty result
        let result = Box::new(SimlinLoops {
            loops: ptr::null_mut(),
            count: 0,
        });
        return Box::into_raw(result);
    }
    // Convert to C structures
    let mut c_loops = Vec::with_capacity(all_loops.len());
    for loop_item in all_loops {
        // Convert loop ID to C string
        let id = CString::new(loop_item.id).unwrap();
        // Convert variable names to C strings
        let mut var_names = Vec::with_capacity(loop_item.links.len() + 1);
        // Collect unique variables from the loop path
        let mut seen = std::collections::HashSet::new();
        if !loop_item.links.is_empty() {
            // Add the first variable
            let first = &loop_item.links[0].from;
            if seen.insert(first.clone()) {
                let c_str = CString::new(first.as_str()).unwrap();
                var_names.push(c_str.into_raw());
            }
            // Add all 'to' variables
            for link in &loop_item.links {
                if seen.insert(link.to.clone()) {
                    let c_str = CString::new(link.to.as_str()).unwrap();
                    var_names.push(c_str.into_raw());
                }
            }
        }
        let var_count = var_names.len();
        let variables = if var_count > 0 {
            let mut vars = var_names.into_boxed_slice();
            let ptr = vars.as_mut_ptr();
            std::mem::forget(vars);
            ptr
        } else {
            ptr::null_mut()
        };
        // Convert polarity
        let polarity = match loop_item.polarity {
            LoopPolarity::Reinforcing => SimlinLoopPolarity::Reinforcing,
            LoopPolarity::Balancing => SimlinLoopPolarity::Balancing,
        };
        c_loops.push(SimlinLoop {
            id: id.into_raw(),
            variables,
            var_count,
            polarity,
        });
    }
    let count = c_loops.len();
    let mut loops = c_loops.into_boxed_slice();
    let loops_ptr = loops.as_mut_ptr();
    std::mem::forget(loops);
    let result = Box::new(SimlinLoops {
        loops: loops_ptr,
        count,
    });
    Box::into_raw(result)
}
/// Frees a SimlinLoops structure
///
/// # Safety
/// - `loops` must be a valid pointer returned by simlin_analyze_get_loops
#[no_mangle]
pub unsafe extern "C" fn simlin_free_loops(loops: *mut SimlinLoops) {
    if loops.is_null() {
        return;
    }
    let loops = Box::from_raw(loops);
    if !loops.loops.is_null() && loops.count > 0 {
        let loop_slice = std::slice::from_raw_parts_mut(loops.loops, loops.count);
        for loop_item in loop_slice {
            // Free the loop ID
            if !loop_item.id.is_null() {
                let _ = CString::from_raw(loop_item.id);
            }
            // Free the variable names
            if !loop_item.variables.is_null() && loop_item.var_count > 0 {
                let vars = std::slice::from_raw_parts_mut(loop_item.variables, loop_item.var_count);
                for var in vars {
                    if !var.is_null() {
                        let _ = CString::from_raw(*var);
                    }
                }
                let _ = Box::from_raw(std::slice::from_raw_parts_mut(
                    loop_item.variables,
                    loop_item.var_count,
                ));
            }
        }
        let _ = Box::from_raw(std::slice::from_raw_parts_mut(loops.loops, loops.count));
    }
}
/// Gets all causal links in a model
///
/// Returns all causal links detected in the model.
/// This includes flow-to-stock, stock-to-flow, and auxiliary-to-auxiliary links.
/// If the simulation has been run with LTM enabled, link scores will be included.
///
/// # Safety
/// - `sim` must be a valid pointer to a SimlinSim
/// - The returned SimlinLinks must be freed with simlin_free_links
#[no_mangle]
pub unsafe extern "C" fn simlin_analyze_get_links(sim: *mut SimlinSim) -> *mut SimlinLinks {
    if sim.is_null() {
        return ptr::null_mut();
    }

    let sim = &*sim;
    let model_ref = &*sim.model;
    let project = &(*model_ref.project).project;

    // Get the model
    let model = match project.models.get(&canonicalize(&model_ref.model_name)) {
        Some(m) => m,
        None => return ptr::null_mut(),
    };

    // Build a causal graph to get links
    let graph = match engine::ltm::CausalGraph::from_model(model, project) {
        Ok(g) => g,
        Err(_) => return ptr::null_mut(),
    };

    // Get all loops to extract links
    let loops = graph.find_loops();

    // Collect unique links from all loops
    let mut unique_links = std::collections::HashMap::new();
    for loop_item in loops {
        for link in loop_item.links {
            let key = (link.from.clone(), link.to.clone());
            unique_links.entry(key).or_insert(link);
        }
    }

    // Also add direct dependencies that might not be in loops
    for (var_name, var) in &model.variables {
        let deps = match var {
            engine::Variable::Stock {
                inflows, outflows, ..
            } => {
                let mut deps = Vec::new();
                for flow in inflows.iter().chain(outflows.iter()) {
                    deps.push((flow.clone(), var_name.clone()));
                }
                deps
            }
            engine::Variable::Var { ast, .. } if ast.is_some() => {
                let deps = engine::identifier_set(ast.as_ref().unwrap(), &[], None);
                deps.into_iter()
                    .map(|dep| (dep, var_name.clone()))
                    .collect()
            }
            engine::Variable::Module { inputs, .. } => inputs
                .iter()
                .map(|input| (input.src.clone(), var_name.clone()))
                .collect(),
            _ => Vec::new(),
        };

        for (from, to) in deps {
            let key = (from.clone(), to.clone());
            unique_links.entry(key).or_insert(engine::ltm::Link {
                from,
                to,
                polarity: engine::ltm::LinkPolarity::Unknown,
            });
        }
    }

    if unique_links.is_empty() {
        return Box::into_raw(Box::new(SimlinLinks {
            links: ptr::null_mut(),
            count: 0,
        }));
    }

    // Check if LTM is enabled and simulation has been run
    let has_ltm_scores = sim.enable_ltm && sim.results.is_some();

    // Convert to C structures
    let mut c_links = Vec::with_capacity(unique_links.len());
    for (_, link) in unique_links {
        let from = CString::new(link.from.as_str()).unwrap().into_raw();
        let to = CString::new(link.to.as_str()).unwrap().into_raw();

        // Convert polarity
        let polarity = match link.polarity {
            engine::ltm::LinkPolarity::Positive => SimlinLinkPolarity::Positive,
            engine::ltm::LinkPolarity::Negative => SimlinLinkPolarity::Negative,
            engine::ltm::LinkPolarity::Unknown => SimlinLinkPolarity::Unknown,
        };

        // Get link scores if available
        let (score_ptr, score_len) = if has_ltm_scores {
            let link_score_var = format!(
                "$⁚ltm⁚link_score⁚{}⁚{}",
                link.from.as_str(),
                link.to.as_str()
            );
            let var_ident = canonicalize(&link_score_var);

            if let Some(ref results) = sim.results {
                if let Some(&offset) = results.offsets.get(&var_ident) {
                    let mut scores = Vec::with_capacity(results.step_count);
                    for row in results.iter() {
                        scores.push(row[offset]);
                    }
                    let score_len = scores.len();
                    let mut boxed = scores.into_boxed_slice();
                    let score_ptr = boxed.as_mut_ptr();
                    std::mem::forget(boxed);
                    (score_ptr, score_len)
                } else {
                    (ptr::null_mut(), 0)
                }
            } else {
                (ptr::null_mut(), 0)
            }
        } else {
            (ptr::null_mut(), 0)
        };

        c_links.push(SimlinLink {
            from,
            to,
            polarity,
            score: score_ptr,
            score_len,
        });
    }

    let count = c_links.len();
    let mut links = c_links.into_boxed_slice();
    let links_ptr = links.as_mut_ptr();
    std::mem::forget(links);

    Box::into_raw(Box::new(SimlinLinks {
        links: links_ptr,
        count,
    }))
}

/// Frees a SimlinLinks structure
///
/// # Safety
/// - `links` must be valid pointer returned by simlin_analyze_get_links
#[no_mangle]
pub unsafe extern "C" fn simlin_free_links(links: *mut SimlinLinks) {
    if links.is_null() {
        return;
    }
    let links = Box::from_raw(links);
    if !links.links.is_null() && links.count > 0 {
        let link_slice = std::slice::from_raw_parts_mut(links.links, links.count);
        for link in link_slice {
            // Free the from and to strings
            if !link.from.is_null() {
                let _ = CString::from_raw(link.from);
            }
            if !link.to.is_null() {
                let _ = CString::from_raw(link.to);
            }
            // Free the score array if present
            if !link.score.is_null() && link.score_len > 0 {
                let _ = Box::from_raw(std::slice::from_raw_parts_mut(link.score, link.score_len));
            }
        }
        let _ = Box::from_raw(std::slice::from_raw_parts_mut(links.links, links.count));
    }
}

/// Gets the relative loop score time series for a specific loop
///
/// Renamed for clarity from simlin_analyze_get_rel_loop_score
///
/// # Safety
/// - `sim` must be a valid pointer to a SimlinSim that has been run to completion
/// - `loop_id` must be a valid C string
/// - `results` must be a valid pointer to an array of at least `len` doubles
#[no_mangle]
pub unsafe extern "C" fn simlin_analyze_get_relative_loop_score(
    sim: *mut SimlinSim,
    loop_id: *const c_char,
    results_ptr: *mut c_double,
    len: usize,
) -> c_int {
    simlin_analyze_get_rel_loop_score(sim, loop_id, results_ptr, len)
}

/// Gets the relative loop score time series for a specific loop
///
/// # Safety
/// - `sim` must be a valid pointer to a SimlinSim that has been run to completion
/// - `loop_id` must be a valid C string
/// - `results` must be a valid pointer to an array of at least `len` doubles
#[no_mangle]
pub unsafe extern "C" fn simlin_analyze_get_rel_loop_score(
    sim: *mut SimlinSim,
    loop_id: *const c_char,
    results_ptr: *mut c_double,
    len: usize,
) -> c_int {
    if sim.is_null() || loop_id.is_null() || results_ptr.is_null() {
        return -1;
    }
    let sim = &*sim;
    let loop_id = match CStr::from_ptr(loop_id).to_str() {
        Ok(s) => s,
        Err(_) => return -1,
    };
    // The relative loop score variable name format
    let var_name = format!("$⁚ltm⁚rel_loop_score⁚{loop_id}");
    let var_ident = canonicalize(&var_name);
    if let Some(ref results) = sim.results {
        if let Some(&offset) = results.offsets.get(&var_ident) {
            let count = std::cmp::min(results.step_count, len);
            for (i, row) in results.iter().take(count).enumerate() {
                *results_ptr.add(i) = row[offset];
            }
            // Return 0 for success, matching Go's expectations
            0
        } else {
            // Variable not found - project might not have LTM enabled
            -1
        }
    } else {
        -1
    }
}
// Memory management functions for WASM
// We use a simple approach where we store the size before the allocated memory
#[no_mangle]
pub extern "C" fn simlin_malloc(size: usize) -> *mut u8 {
    unsafe {
        // Allocate extra space to store the size
        let total_size = size + size_of::<usize>();
        let layout = Layout::from_size_align_unchecked(total_size, align_of::<usize>());
        let ptr = alloc(layout);
        if ptr.is_null() {
            return ptr;
        }
        // Store the size at the beginning
        *(ptr as *mut usize) = size;
        // Return pointer to the user data (after the size)
        ptr.add(size_of::<usize>())
    }
}
/// Frees memory allocated by simlin_malloc
///
/// # Safety
/// - `ptr` must be a valid pointer returned by simlin_malloc, or null
/// - The pointer must not be used after calling this function
#[no_mangle]
pub unsafe extern "C" fn simlin_free(ptr: *mut u8) {
    if ptr.is_null() {
        return;
    }
    // Get the actual allocation pointer (before the user data)
    let actual_ptr = ptr.sub(size_of::<usize>());
    // Read the size
    let size = *(actual_ptr as *mut usize);
    let total_size = size + size_of::<usize>();
    let layout = Layout::from_size_align_unchecked(total_size, align_of::<usize>());
    dealloc(actual_ptr, layout);
}
/// simlin_import_xmile opens a project from XMILE/STMX format data.
/// If an error occurs, the function returns NULL and if the err parameter
/// is not NULL, details of the error are placed in it.
///
/// # Safety
/// - `data` must be a valid pointer to at least `len` bytes
/// - `err` may be null
#[no_mangle]
pub unsafe extern "C" fn simlin_import_xmile(
    data: *const u8,
    len: usize,
    err: *mut c_int,
) -> *mut SimlinProject {
    if data.is_null() {
        if !err.is_null() {
            *err = engine::ErrorCode::Generic as c_int;
        }
        return ptr::null_mut();
    }

    let slice = std::slice::from_raw_parts(data, len);
    let mut reader = BufReader::new(slice);

    match simlin_compat::open_xmile(&mut reader) {
        Ok(datamodel_project) => {
            let project = datamodel_project.into();
            let boxed = Box::new(SimlinProject {
                project,
                ref_count: AtomicUsize::new(1),
            });
            if !err.is_null() {
                *err = engine::ErrorCode::NoError as c_int;
            }
            Box::into_raw(boxed)
        }
        Err(e) => {
            if !err.is_null() {
                *err = e.code as c_int;
            }
            ptr::null_mut()
        }
    }
}
/// simlin_import_mdl opens a project from Vensim MDL format data.
/// If an error occurs, the function returns NULL and if the err parameter
/// is not NULL, details of the error are placed in it.
///
/// # Safety
/// - `data` must be a valid pointer to at least `len` bytes
/// - `err` may be null
#[no_mangle]
pub unsafe extern "C" fn simlin_import_mdl(
    data: *const u8,
    len: usize,
    err: *mut c_int,
) -> *mut SimlinProject {
    if data.is_null() {
        if !err.is_null() {
            *err = engine::ErrorCode::Generic as c_int;
        }
        return ptr::null_mut();
    }

    let slice = std::slice::from_raw_parts(data, len);
    let mut reader = BufReader::new(slice);

    match simlin_compat::open_vensim(&mut reader) {
        Ok(datamodel_project) => {
            let project = datamodel_project.into();
            let boxed = Box::new(SimlinProject {
                project,
                ref_count: AtomicUsize::new(1),
            });
            if !err.is_null() {
                *err = engine::ErrorCode::NoError as c_int;
            }
            Box::into_raw(boxed)
        }
        Err(e) => {
            if !err.is_null() {
                *err = e.code as c_int;
            }
            ptr::null_mut()
        }
    }
}
/// simlin_export_xmile exports a project to XMILE format.
/// Returns 0 on success, error code on failure.
/// Caller must free output with simlin_free().
///
/// # Safety
/// - `project` must be a valid pointer to a SimlinProject
/// - `output` and `output_len` must be valid pointers
#[no_mangle]
pub unsafe extern "C" fn simlin_export_xmile(
    project: *mut SimlinProject,
    output: *mut *mut u8,
    output_len: *mut usize,
) -> c_int {
    if project.is_null() || output.is_null() || output_len.is_null() {
        return engine::ErrorCode::Generic as c_int;
    }

    let proj = &(*project).project;

    match simlin_compat::to_xmile(&proj.datamodel) {
        Ok(xmile_str) => {
            let bytes = xmile_str.into_bytes();
            let len = bytes.len();

            let buf = simlin_malloc(len);
            if buf.is_null() {
                return engine::ErrorCode::Generic as c_int;
            }

            std::ptr::copy_nonoverlapping(bytes.as_ptr(), buf, len);

            *output = buf;
            *output_len = len;
            engine::ErrorCode::NoError as c_int
        }
        Err(e) => e.code as c_int,
    }
}

/// Serializes a project to binary protobuf format
///
/// Returns the project's datamodel serialized as protobuf bytes.
/// This is the native format expected by simlin_project_open.
/// Useful for saving projects or transferring them between systems.
///
/// Returns 0 on success, error code on failure.
/// Caller must free output with simlin_free().
///
/// # Safety
/// - `project` must be a valid pointer to a SimlinProject
/// - `output` and `output_len` must be valid pointers
#[no_mangle]
pub unsafe extern "C" fn simlin_project_serialize(
    project: *mut SimlinProject,
    output: *mut *mut u8,
    output_len: *mut usize,
) -> c_int {
    if project.is_null() || output.is_null() || output_len.is_null() {
        return engine::ErrorCode::Generic as c_int;
    }

    let proj = &(*project).project;

    // Serialize the datamodel to protobuf
    let pb_project = engine::serde::serialize(&proj.datamodel);

    let mut bytes = Vec::new();
    if pb_project.encode(&mut bytes).is_err() {
        return engine::ErrorCode::ProtobufDecode as c_int;
    }

    let len = bytes.len();
    let buf = simlin_malloc(len);
    if buf.is_null() {
        return engine::ErrorCode::Generic as c_int;
    }

    std::ptr::copy_nonoverlapping(bytes.as_ptr(), buf, len);

    *output = buf;
    *output_len = len;
    engine::ErrorCode::NoError as c_int
}

/// Applies a patch to the project datamodel.
///
/// The patch is encoded as a `project_io.Patch` protobuf message. The caller can
/// request a dry run (which performs validation without committing) and control
/// whether errors are permitted. When `allow_errors` is false, any static or
/// simulation error will cause the patch to be rejected.
///
/// On success returns `SimlinErrorCode::NoError`. On failure returns an error
/// code describing why the patch could not be applied. When `out_errors` is not
/// NULL it will receive a pointer to a `SimlinErrorDetails` structure
/// describing all encountered errors; callers must free it with
/// `simlin_free_error_details`.
///
/// # Safety
/// - `project` must be a valid pointer to a SimlinProject
/// - `patch_data` must be a valid pointer to at least `patch_len` bytes
/// - `out_errors` may be null
#[no_mangle]
pub unsafe extern "C" fn simlin_project_apply_patch(
    project: *mut SimlinProject,
    patch_data: *const u8,
    patch_len: usize,
    dry_run: bool,
    allow_errors: bool,
    out_errors: *mut *mut SimlinErrorDetails,
) -> SimlinErrorCode {
    if project.is_null() {
        return SimlinErrorCode::Generic;
    }

    if !out_errors.is_null() {
        *out_errors = ptr::null_mut();
    }

    if patch_len > 0 && patch_data.is_null() {
        return SimlinErrorCode::Generic;
    }

    let patch_slice = if patch_len == 0 {
        &[]
    } else {
        std::slice::from_raw_parts(patch_data, patch_len)
    };

    let patch = match engine::project_io::Patch::decode(patch_slice) {
        Ok(patch) => patch,
        Err(_) => return SimlinErrorCode::ProtobufDecode,
    };

    let mut staged_datamodel = (*project).project.datamodel.clone();
    if let Err(err) = engine::apply_patch(&mut staged_datamodel, &patch) {
        return SimlinErrorCode::from(err.code);
    }

    let staged_project = engine::Project::from(staged_datamodel);

    let mut all_errors = collect_project_errors(&staged_project);
    let sim_error = create_vm(&staged_project, "main").err();
    if let Some(error) = sim_error.clone() {
        all_errors.push(
            ErrorDetailBuilder::new(error.code)
                .message(error.get_details())
                .model_name("main")
                .build(),
        );
    }

    let error_code = if !allow_errors {
        first_error_code(&staged_project, sim_error.as_ref())
    } else {
        None
    };

    if !out_errors.is_null() {
        *out_errors = allocate_error_details(all_errors);
    }

    if let Some(code) = error_code {
        return code;
    }

    if !dry_run {
        (*project).project = staged_project;
    }

    SimlinErrorCode::NoError
}

// Helper function to convert a Rust string to a C string pointer
fn str_to_c_ptr(s: &str) -> *mut c_char {
    CString::new(s).unwrap().into_raw()
}

// Helper function to convert a vector into a C-compatible array
fn vec_to_c_array<T>(vec: Vec<T>) -> (*mut T, usize) {
    if vec.is_empty() {
        return (ptr::null_mut(), 0);
    }
    let count = vec.len();
    let mut boxed = vec.into_boxed_slice();
    let ptr = boxed.as_mut_ptr();
    std::mem::forget(boxed);
    (ptr, count)
}

// Builder for SimlinErrorDetail to reduce boilerplate
struct ErrorDetailBuilder {
    code: SimlinErrorCode,
    message: *mut c_char,
    model_name: *mut c_char,
    variable_name: *mut c_char,
    start_offset: u16,
    end_offset: u16,
}

impl ErrorDetailBuilder {
    fn new(code: ErrorCode) -> Self {
        Self {
            code: SimlinErrorCode::from(code),
            message: ptr::null_mut(),
            model_name: ptr::null_mut(),
            variable_name: ptr::null_mut(),
            start_offset: 0,
            end_offset: 0,
        }
    }

    fn message(mut self, msg: Option<String>) -> Self {
        self.message = msg.as_deref().map(str_to_c_ptr).unwrap_or(ptr::null_mut());
        self
    }

    fn model_name(mut self, name: &str) -> Self {
        self.model_name = str_to_c_ptr(name);
        self
    }

    fn variable_name(mut self, name: &str) -> Self {
        self.variable_name = str_to_c_ptr(name);
        self
    }

    fn offsets(mut self, start: u16, end: u16) -> Self {
        self.start_offset = start;
        self.end_offset = end;
        self
    }

    fn build(self) -> SimlinErrorDetail {
        SimlinErrorDetail {
            code: self.code,
            message: self.message,
            model_name: self.model_name,
            variable_name: self.variable_name,
            start_offset: self.start_offset,
            end_offset: self.end_offset,
        }
    }
}

// Helper function to collect equation errors for a variable
fn collect_equation_errors(
    errors: Vec<EquationError>,
    model_name: &str,
    var_name: &str,
) -> Vec<SimlinErrorDetail> {
    errors
        .into_iter()
        .map(|error| {
            ErrorDetailBuilder::new(error.code)
                .model_name(model_name)
                .variable_name(var_name)
                .offsets(error.start, error.end)
                .build()
        })
        .collect()
}

// Helper function to collect unit errors for a variable
fn collect_unit_errors(
    errors: Vec<UnitError>,
    model_name: &str,
    var_name: &str,
) -> Vec<SimlinErrorDetail> {
    errors
        .into_iter()
        .map(|error| {
            let (code, start, end, message) = match error {
                UnitError::DefinitionError(eq_err, details) => {
                    (eq_err.code, eq_err.start, eq_err.end, details)
                }
                UnitError::ConsistencyError(err_code, loc, details) => {
                    (err_code, loc.start, loc.end, details)
                }
            };
            ErrorDetailBuilder::new(code)
                .message(message)
                .model_name(model_name)
                .variable_name(var_name)
                .offsets(start, end)
                .build()
        })
        .collect()
}

fn collect_project_errors(project: &engine::Project) -> Vec<SimlinErrorDetail> {
    let mut all_errors = Vec::new();

    for error in &project.errors {
        all_errors.push(
            ErrorDetailBuilder::new(error.code)
                .message(error.get_details())
                .build(),
        );
    }

    for (model_name, model) in &project.models {
        if let Some(ref errors) = model.errors {
            for error in errors {
                all_errors.push(
                    ErrorDetailBuilder::new(error.code)
                        .message(error.get_details())
                        .model_name(model_name.as_str())
                        .build(),
                );
            }
        }

        for (var_name, errors) in model.get_variable_errors() {
            all_errors.extend(collect_equation_errors(
                errors,
                model_name.as_str(),
                var_name.as_str(),
            ));
        }

        for (var_name, errors) in model.get_unit_errors() {
            all_errors.extend(collect_unit_errors(
                errors,
                model_name.as_str(),
                var_name.as_str(),
            ));
        }
    }

    all_errors
}

fn gather_error_details(
    project: &engine::Project,
) -> (Vec<SimlinErrorDetail>, Option<engine::Error>) {
    let mut all_errors = collect_project_errors(project);
    let sim_error = create_vm(project, "main").err();

    if let Some(error) = sim_error.clone() {
        all_errors.push(
            ErrorDetailBuilder::new(error.code)
                .message(error.get_details())
                .model_name("main")
                .build(),
        );
    }

    (all_errors, sim_error)
}

fn allocate_error_details(errors: Vec<SimlinErrorDetail>) -> *mut SimlinErrorDetails {
    if errors.is_empty() {
        return ptr::null_mut();
    }

    let (errors_ptr, count) = vec_to_c_array(errors);
    let result = Box::new(SimlinErrorDetails {
        errors: errors_ptr,
        count,
    });
    Box::into_raw(result)
}

fn first_error_code(
    project: &engine::Project,
    sim_error: Option<&engine::Error>,
) -> Option<SimlinErrorCode> {
    if let Some(error) = project.errors.first() {
        return Some(SimlinErrorCode::from(error.code));
    }

    for model in project.models.values() {
        if let Some(errors) = &model.errors {
            if let Some(error) = errors.first() {
                return Some(SimlinErrorCode::from(error.code));
            }
        }

        if model
            .get_variable_errors()
            .values()
            .any(|errors| !errors.is_empty())
        {
            return Some(SimlinErrorCode::VariablesHaveErrors);
        }

        if model
            .get_unit_errors()
            .values()
            .any(|errors| !errors.is_empty())
        {
            return Some(SimlinErrorCode::UnitDefinitionErrors);
        }
    }

    sim_error.map(|error| SimlinErrorCode::from(error.code))
}

/// Get all errors in a project including static analysis and compilation errors
///
/// Returns NULL if no errors exist in the project. This function collects all
/// static errors (equation parsing, unit checking, etc.) and also attempts to
/// compile the "main" model to find any compilation-time errors.
///
/// The caller must free the returned error details using `simlin_free_error_details`.
///
/// # Example Usage (C)
/// ```c
/// SimlinErrorDetails* errors = simlin_project_get_errors(project);
/// if (errors != NULL) {
///     for (size_t i = 0; i < errors->count; i++) {
///         SimlinErrorDetail* error = &errors->errors[i];
///         printf("Error %d", error->code);
///         if (error->model_name != NULL) {
///             printf(" in model %s", error->model_name);
///         }
///         if (error->variable_name != NULL) {
///             printf(" for variable %s", error->variable_name);
///         }
///         printf("\n");
///     }
///     simlin_free_error_details(errors);
/// } else {
///     // Project has no errors and is ready to simulate
/// }
/// ```
///
/// # Safety
/// - `project` must be a valid pointer to a SimlinProject
/// - The returned pointer must be freed with `simlin_free_error_details`
#[no_mangle]
pub unsafe extern "C" fn simlin_project_get_errors(
    project: *mut SimlinProject,
) -> *mut SimlinErrorDetails {
    if project.is_null() {
        return ptr::null_mut();
    }

    let proj = &(*project).project;
    let (all_errors, _) = gather_error_details(proj);

    if all_errors.is_empty() {
        return ptr::null_mut();
    }

    allocate_error_details(all_errors)
}

/// Free error details returned by the API
///
/// This function properly deallocates all memory associated with an error details
/// collection, including all string fields within each error detail.
///
/// # Example Usage (C)
/// ```c
/// SimlinErrorDetails* errors = simlin_project_get_errors(project);
/// // ... use the errors ...
/// simlin_free_error_details(errors); // Always free when done
/// ```
///
/// # Safety
/// - `details` must be a valid pointer returned by simlin_project_get_errors or similar
/// - The pointer must not be used after calling this function
#[no_mangle]
pub unsafe extern "C" fn simlin_free_error_details(details: *mut SimlinErrorDetails) {
    if details.is_null() {
        return;
    }

    let details = Box::from_raw(details);
    if !details.errors.is_null() && details.count > 0 {
        let error_slice = std::slice::from_raw_parts_mut(details.errors, details.count);
        for error in error_slice {
            // Free the message
            if !error.message.is_null() {
                let _ = CString::from_raw(error.message);
            }
            // Free the model name
            if !error.model_name.is_null() {
                let _ = CString::from_raw(error.model_name);
            }
            // Free the variable name
            if !error.variable_name.is_null() {
                let _ = CString::from_raw(error.variable_name);
            }
        }
        let _ = Box::from_raw(std::slice::from_raw_parts_mut(
            details.errors,
            details.count,
        ));
    }
}

/// Free a single error detail
///
/// This function properly deallocates all memory associated with a single error
/// detail, including all string fields.
///
/// # Example Usage (C)
/// ```c
/// SimlinErrorDetail* error = simlin_project_get_simulation_error(project, NULL);
/// if (error != NULL) {
///     // ... use the error ...
///     simlin_free_error_detail(error); // Always free when done
/// }
/// ```
///
/// # Safety
/// - `detail` must be a valid pointer returned by simlin_project_get_simulation_error
/// - The pointer must not be used after calling this function
#[no_mangle]
pub unsafe extern "C" fn simlin_free_error_detail(detail: *mut SimlinErrorDetail) {
    if detail.is_null() {
        return;
    }

    let detail = Box::from_raw(detail);
    // Free the message
    if !detail.message.is_null() {
        let _ = CString::from_raw(detail.message);
    }
    // Free the model name
    if !detail.model_name.is_null() {
        let _ = CString::from_raw(detail.model_name);
    }
    // Free the variable name
    if !detail.variable_name.is_null() {
        let _ = CString::from_raw(detail.variable_name);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine::test_common::TestProject;
    #[test]
    fn test_error_str() {
        unsafe {
            let err_str = simlin_error_str(0);
            assert!(!err_str.is_null());
            let s = CStr::from_ptr(err_str);
            assert_eq!(s.to_str().unwrap(), "no_error");
        }
    }

    fn open_project_from_datamodel(project: &engine::datamodel::Project) -> *mut SimlinProject {
        let pb = engine::serde::serialize(project);
        let mut buf = Vec::new();
        pb.encode(&mut buf).unwrap();
        unsafe {
            let mut err: c_int = 0;
            let proj = simlin_project_open(buf.as_ptr(), buf.len(), &mut err as *mut c_int);
            assert!(!proj.is_null(), "project open failed: {err}");
            assert_eq!(err, engine::ErrorCode::NoError as c_int);
            proj
        }
    }

    fn aux_patch(model: &str, aux: engine::datamodel::Aux) -> Vec<u8> {
        let variable = engine::datamodel::Variable::Aux(aux);
        let aux_pb = match engine::project_io::Variable::from(variable).v.unwrap() {
            engine::project_io::variable::V::Aux(aux) => aux,
            _ => unreachable!(),
        };
        let patch = engine::project_io::Patch {
            ops: vec![engine::project_io::PatchOperation {
                op: Some(engine::project_io::patch_operation::Op::UpsertAux(
                    engine::project_io::UpsertAuxOp {
                        model_name: model.to_string(),
                        aux: Some(aux_pb),
                    },
                )),
            }],
        };
        let mut bytes = Vec::new();
        patch.encode(&mut bytes).unwrap();
        bytes
    }

    #[test]
    fn test_project_apply_patch_commits() {
        let datamodel = TestProject::new("test").build_datamodel();
        let proj = open_project_from_datamodel(&datamodel);

        let aux = engine::datamodel::Aux {
            ident: "new_aux".to_string(),
            equation: engine::datamodel::Equation::Scalar("5".to_string(), None),
            documentation: String::new(),
            units: None,
            gf: None,
            can_be_module_input: false,
            visibility: engine::datamodel::Visibility::Private,
            ai_state: None,
            uid: None,
        };
        let patch_bytes = aux_patch("main", aux);

        unsafe {
            let mut errors: *mut SimlinErrorDetails = ptr::null_mut();
            let code = simlin_project_apply_patch(
                proj,
                patch_bytes.as_ptr(),
                patch_bytes.len(),
                false,
                true,
                &mut errors as *mut *mut SimlinErrorDetails,
            );
            assert_eq!(code, SimlinErrorCode::NoError);
            assert!(errors.is_null());

            let model = (*proj).project.datamodel.get_model("main").unwrap();
            assert!(model.get_variable("new_aux").is_some());

            simlin_project_unref(proj);
        }
    }

    #[test]
    fn test_project_apply_patch_errors_respected() {
        let datamodel = TestProject::new("test").build_datamodel();
        let proj = open_project_from_datamodel(&datamodel);

        let aux = engine::datamodel::Aux {
            ident: "bad_aux".to_string(),
            equation: engine::datamodel::Equation::Scalar(String::new(), None),
            documentation: String::new(),
            units: None,
            gf: None,
            can_be_module_input: false,
            visibility: engine::datamodel::Visibility::Private,
            ai_state: None,
            uid: None,
        };
        let patch_bytes = aux_patch("main", aux);

        unsafe {
            let mut errors: *mut SimlinErrorDetails = ptr::null_mut();
            let code = simlin_project_apply_patch(
                proj,
                patch_bytes.as_ptr(),
                patch_bytes.len(),
                false,
                false,
                &mut errors as *mut *mut SimlinErrorDetails,
            );
            assert_eq!(code, SimlinErrorCode::VariablesHaveErrors);
            assert!(!errors.is_null());
            simlin_free_error_details(errors);

            // Project should remain unchanged
            let model = (*proj).project.datamodel.get_model("main").unwrap();
            assert!(model.get_variable("bad_aux").is_none());

            let mut errors_allow: *mut SimlinErrorDetails = ptr::null_mut();
            let code = simlin_project_apply_patch(
                proj,
                patch_bytes.as_ptr(),
                patch_bytes.len(),
                false,
                true,
                &mut errors_allow as *mut *mut SimlinErrorDetails,
            );
            assert_eq!(code, SimlinErrorCode::NoError);
            assert!(!errors_allow.is_null());
            simlin_free_error_details(errors_allow);

            let model = (*proj).project.datamodel.get_model("main").unwrap();
            assert!(model.get_variable("bad_aux").is_some());

            simlin_project_unref(proj);
        }
    }

    #[test]
    fn test_project_apply_patch_dry_run() {
        let datamodel = TestProject::new("test").build_datamodel();
        let proj = open_project_from_datamodel(&datamodel);

        let aux = engine::datamodel::Aux {
            ident: "dry_aux".to_string(),
            equation: engine::datamodel::Equation::Scalar("3".to_string(), None),
            documentation: String::new(),
            units: None,
            gf: None,
            can_be_module_input: false,
            visibility: engine::datamodel::Visibility::Private,
            ai_state: None,
            uid: None,
        };
        let patch_bytes = aux_patch("main", aux);

        unsafe {
            let mut errors: *mut SimlinErrorDetails = ptr::null_mut();
            let code = simlin_project_apply_patch(
                proj,
                patch_bytes.as_ptr(),
                patch_bytes.len(),
                true,
                true,
                &mut errors as *mut *mut SimlinErrorDetails,
            );
            assert_eq!(code, SimlinErrorCode::NoError);
            assert!(errors.is_null());

            // Dry run should not commit changes
            let model = (*proj).project.datamodel.get_model("main").unwrap();
            assert!(model.get_variable("dry_aux").is_none());

            simlin_project_unref(proj);
        }
    }

    #[test]
    fn test_interactive_set_get() {
        // Load the SIR project fixture
        let pb_path = std::path::Path::new("../../src/engine2/testdata/SIR_project.pb");
        if !pb_path.exists() {
            eprintln!("missing SIR_project.pb fixture; skipping");
            return;
        }
        let data = std::fs::read(pb_path).unwrap();

        unsafe {
            // Open project
            let mut err_code: c_int = 0;
            let proj = simlin_project_open(data.as_ptr(), data.len(), &mut err_code as *mut c_int);
            assert!(!proj.is_null(), "project open failed: {err_code}");

            // Get model
            let model = simlin_project_get_model(proj, std::ptr::null());
            assert!(!model.is_null());

            // Create sim
            let sim = simlin_sim_new(model, false);
            assert!(!sim.is_null());

            // Run to a partial time
            let rc = simlin_sim_run_to(sim, 0.125);
            assert_eq!(rc, engine::ErrorCode::NoError as c_int);

            // Fetch var names from model
            let count = simlin_model_get_var_count(model);
            assert!(count > 0, "expected varcount > 0");
            let mut name_ptrs: Vec<*mut c_char> = vec![std::ptr::null_mut(); count as usize];
            let err = simlin_model_get_var_names(model, name_ptrs.as_mut_ptr(), name_ptrs.len());
            assert_eq!(0, err);

            // Find canonical name that ends with "infectious"
            let mut infectious_name: Option<String> = None;
            for &p in &name_ptrs {
                if p.is_null() {
                    continue;
                }
                let s = std::ffi::CStr::from_ptr(p).to_string_lossy().into_owned();
                // free the CString from get_var_names
                simlin_free_string(p as *mut c_char);
                if s.to_ascii_lowercase().ends_with("infectious") {
                    infectious_name = Some(s);
                }
            }
            let infectious = infectious_name.expect("infectious not found in names");

            // Read current value using canonical name
            let c_infectious = CString::new(infectious.clone()).unwrap();
            let mut out: c_double = 0.0;
            let rc = simlin_sim_get_value(sim, c_infectious.as_ptr(), &mut out as *mut c_double);
            assert_eq!(rc, engine::ErrorCode::NoError as c_int, "get_value rc={rc}");

            // Set to a new value and read it back
            let new_val: f64 = 42.0;
            let rc = simlin_sim_set_value(sim, c_infectious.as_ptr(), new_val as c_double);
            assert_eq!(rc, engine::ErrorCode::NoError as c_int, "set_value rc={rc}");

            let mut out2: c_double = 0.0;
            let rc = simlin_sim_get_value(sim, c_infectious.as_ptr(), &mut out2 as *mut c_double);
            assert_eq!(
                rc,
                engine::ErrorCode::NoError as c_int,
                "get_value(after set) rc={rc}"
            );
            assert!(
                (out2 - new_val).abs() <= 1e-9,
                "expected {new_val} got {out2}"
            );

            // Cleanup
            simlin_sim_unref(sim);
            simlin_model_unref(model);
            simlin_project_unref(proj);
        }
    }

    #[test]
    fn test_set_value_phases() {
        // Load the SIR project fixture
        let pb_path = std::path::Path::new("../../src/engine2/testdata/SIR_project.pb");
        if !pb_path.exists() {
            eprintln!("missing SIR_project.pb fixture; skipping");
            return;
        }
        let data = std::fs::read(pb_path).unwrap();

        unsafe {
            // Open project
            let mut err_code: c_int = 0;
            let proj = simlin_project_open(data.as_ptr(), data.len(), &mut err_code as *mut c_int);
            assert!(!proj.is_null(), "project open failed: {err_code}");

            // Get model
            let model = simlin_project_get_model(proj, std::ptr::null());
            assert!(!model.is_null());

            // Test Phase 1: Set value before first run_to (initial value)
            let sim = simlin_sim_new(model, false);
            assert!(!sim.is_null());

            // Get variable names to find a valid variable
            let count = simlin_model_get_var_count(model);
            let mut name_ptrs: Vec<*mut c_char> = vec![std::ptr::null_mut(); count as usize];
            simlin_model_get_var_names(model, name_ptrs.as_mut_ptr(), name_ptrs.len());

            let mut test_var_name: Option<String> = None;
            for &p in &name_ptrs {
                if p.is_null() {
                    continue;
                }
                let s = std::ffi::CStr::from_ptr(p).to_string_lossy().into_owned();
                simlin_free_string(p as *mut c_char);
                if s.to_ascii_lowercase().ends_with("infectious") {
                    test_var_name = Some(s);
                    break;
                }
            }
            let test_var = test_var_name.expect("test variable not found");
            let c_test_var = CString::new(test_var.clone()).unwrap();

            // Set initial value before any run_to
            let initial_val: f64 = 100.0;
            let rc = simlin_sim_set_value(sim, c_test_var.as_ptr(), initial_val);
            assert_eq!(
                rc,
                engine::ErrorCode::NoError as c_int,
                "set_value before run failed"
            );

            // Verify initial value is set
            let mut out: c_double = 0.0;
            simlin_sim_get_value(sim, c_test_var.as_ptr(), &mut out);
            assert!(
                (out - initial_val).abs() <= 1e-9,
                "initial value not set correctly"
            );

            // Test Phase 2: Set value during simulation (after partial run)
            simlin_sim_run_to(sim, 0.5);
            let during_val: f64 = 200.0;
            let rc = simlin_sim_set_value(sim, c_test_var.as_ptr(), during_val);
            assert_eq!(
                rc,
                engine::ErrorCode::NoError as c_int,
                "set_value during run failed"
            );

            simlin_sim_get_value(sim, c_test_var.as_ptr(), &mut out);
            assert!(
                (out - during_val).abs() <= 1e-9,
                "value during run not set correctly"
            );

            // Test Phase 3: Set value after run_to_end (should fail)
            simlin_sim_run_to_end(sim);
            let rc = simlin_sim_set_value(sim, c_test_var.as_ptr(), 300.0);
            assert_eq!(
                rc,
                engine::ErrorCode::NotSimulatable as c_int,
                "set_value after completion should fail with NotSimulatable"
            );

            // Test setting unknown variable (should fail)
            let unknown = CString::new("unknown_variable_xyz").unwrap();
            let rc = simlin_sim_set_value(sim, unknown.as_ptr(), 999.0);
            assert_eq!(
                rc,
                engine::ErrorCode::UnknownDependency as c_int,
                "set_value for unknown variable should fail with UnknownDependency"
            );

            // Cleanup
            simlin_sim_unref(sim);
            simlin_model_unref(model);
            simlin_project_unref(proj);
        }
    }
    // Model-only protobufs are not supported at the ABI layer; only Projects are accepted.
    #[test]
    fn test_project_lifecycle() {
        // Create a minimal valid protobuf project
        let project = engine::project_io::Project {
            name: "test".to_string(),
            sim_specs: Some(engine::project_io::SimSpecs {
                start: 0.0,
                stop: 10.0,
                dt: Some(engine::project_io::Dt {
                    value: 1.0,
                    is_reciprocal: false,
                }),
                save_step: None,
                sim_method: engine::project_io::SimMethod::Euler as i32,
                time_units: String::new(),
            }),
            models: vec![engine::project_io::Model {
                name: "main".to_string(),
                variables: vec![],
                views: vec![],
                loop_metadata: vec![],
            }],
            dimensions: vec![],
            units: vec![],
            source: None,
        };
        let mut buf = Vec::new();
        project.encode(&mut buf).unwrap();
        unsafe {
            let mut err: c_int = 0;
            let proj = simlin_project_open(buf.as_ptr(), buf.len(), &mut err);
            assert!(!proj.is_null());
            assert_eq!(err, engine::ErrorCode::NoError as c_int);
            // Test reference counting
            simlin_project_ref(proj);
            assert_eq!((*proj).ref_count.load(Ordering::SeqCst), 2);
            simlin_project_unref(proj);
            assert_eq!((*proj).ref_count.load(Ordering::SeqCst), 1);
            simlin_project_unref(proj);
            // Project should be freed now
        }
    }
    #[test]
    fn test_import_xmile() {
        // Load the SIR XMILE model
        let xmile_path = std::path::Path::new("testdata/SIR.stmx");
        if !xmile_path.exists() {
            eprintln!("missing SIR.stmx fixture; skipping");
            return;
        }
        let data = std::fs::read(xmile_path).unwrap();

        unsafe {
            // Import XMILE
            let mut err_code: c_int = 0;
            let proj = simlin_import_xmile(data.as_ptr(), data.len(), &mut err_code as *mut c_int);
            assert!(!proj.is_null(), "import_xmile failed: {err_code}");
            assert_eq!(err_code, engine::ErrorCode::NoError as c_int);

            // Get model and verify we can create a simulation from the imported project
            let model = simlin_project_get_model(proj, std::ptr::null());
            assert!(!model.is_null());

            let sim = simlin_sim_new(model, false);
            assert!(!sim.is_null());

            // Run simulation to verify it's valid
            let rc = simlin_sim_run_to_end(sim);
            assert_eq!(rc, engine::ErrorCode::NoError as c_int);

            // Check we have expected variables
            let var_count = simlin_model_get_var_count(model);
            assert!(var_count > 0);

            // Clean up
            simlin_sim_unref(sim);
            simlin_model_unref(model);
            simlin_project_unref(proj);
        }
    }

    #[test]
    fn test_import_mdl() {
        // Load the SIR MDL model
        let mdl_path = std::path::Path::new("testdata/SIR.mdl");
        if !mdl_path.exists() {
            eprintln!("missing SIR.mdl fixture; skipping");
            return;
        }
        let data = std::fs::read(mdl_path).unwrap();

        unsafe {
            // Import MDL
            let mut err_code: c_int = 0;
            let proj = simlin_import_mdl(data.as_ptr(), data.len(), &mut err_code as *mut c_int);
            assert!(!proj.is_null(), "import_mdl failed: {err_code}");
            assert_eq!(err_code, engine::ErrorCode::NoError as c_int);

            // Get model and verify we can create a simulation from the imported project
            let model = simlin_project_get_model(proj, std::ptr::null());
            assert!(!model.is_null());

            let sim = simlin_sim_new(model, false);
            assert!(!sim.is_null());

            // Run simulation to verify it's valid
            let rc = simlin_sim_run_to_end(sim);
            assert_eq!(rc, engine::ErrorCode::NoError as c_int);

            // Check we have expected variables
            let var_count = simlin_model_get_var_count(model);
            assert!(var_count > 0);

            // Clean up
            simlin_sim_unref(sim);
            simlin_model_unref(model);
            simlin_project_unref(proj);
        }
    }

    #[test]
    fn test_export_xmile() {
        // Load a project from protobuf first
        let pb_path = std::path::Path::new("testdata/SIR_project.pb");
        if !pb_path.exists() {
            eprintln!("missing SIR_project.pb fixture; skipping");
            return;
        }
        let data = std::fs::read(pb_path).unwrap();

        unsafe {
            // Open project
            let mut err_code: c_int = 0;
            let proj = simlin_project_open(data.as_ptr(), data.len(), &mut err_code as *mut c_int);
            assert!(!proj.is_null(), "project open failed: {err_code}");

            // Export to XMILE
            let mut output: *mut u8 = std::ptr::null_mut();
            let mut output_len: usize = 0;
            let rc = simlin_export_xmile(
                proj,
                &mut output as *mut *mut u8,
                &mut output_len as *mut usize,
            );
            assert_eq!(rc, engine::ErrorCode::NoError as c_int);
            assert!(!output.is_null());
            assert!(output_len > 0);

            // Verify the output is valid XMILE by trying to parse it
            let xmile_data = std::slice::from_raw_parts(output, output_len);
            let xmile_str = std::str::from_utf8(xmile_data).unwrap();
            assert!(xmile_str.contains("<?xml"));
            assert!(xmile_str.contains("<xmile"));

            // Clean up
            simlin_free(output);
            simlin_project_unref(proj);
        }
    }

    #[test]
    fn test_import_export_roundtrip() {
        // Load XMILE model
        let xmile_path = std::path::Path::new("testdata/SIR.stmx");
        if !xmile_path.exists() {
            eprintln!("missing SIR.stmx fixture; skipping");
            return;
        }
        let data = std::fs::read(xmile_path).unwrap();

        unsafe {
            // Import XMILE
            let mut err_code: c_int = 0;
            let proj1 = simlin_import_xmile(data.as_ptr(), data.len(), &mut err_code as *mut c_int);
            assert!(!proj1.is_null());

            // Export to XMILE
            let mut output: *mut u8 = std::ptr::null_mut();
            let mut output_len: usize = 0;
            let rc = simlin_export_xmile(
                proj1,
                &mut output as *mut *mut u8,
                &mut output_len as *mut usize,
            );
            assert_eq!(rc, engine::ErrorCode::NoError as c_int);

            // Import the exported XMILE
            let proj2 = simlin_import_xmile(output, output_len, &mut err_code as *mut c_int);
            assert!(!proj2.is_null());

            // Get models and verify both projects can simulate
            let model1 = simlin_project_get_model(proj1, std::ptr::null());
            let model2 = simlin_project_get_model(proj2, std::ptr::null());
            assert!(!model1.is_null());
            assert!(!model2.is_null());

            let sim1 = simlin_sim_new(model1, false);
            let sim2 = simlin_sim_new(model2, false);
            assert!(!sim1.is_null());
            assert!(!sim2.is_null());

            let rc1 = simlin_sim_run_to_end(sim1);
            let rc2 = simlin_sim_run_to_end(sim2);
            assert_eq!(rc1, engine::ErrorCode::NoError as c_int);
            assert_eq!(rc2, engine::ErrorCode::NoError as c_int);

            // Clean up
            simlin_sim_unref(sim1);
            simlin_sim_unref(sim2);
            simlin_model_unref(model1);
            simlin_model_unref(model2);
            simlin_free(output);
            simlin_project_unref(proj1);
            simlin_project_unref(proj2);
        }
    }

    #[test]
    fn test_import_invalid_data() {
        unsafe {
            // Test with null data
            let mut err_code: c_int = 0;
            let proj = simlin_import_xmile(std::ptr::null(), 0, &mut err_code as *mut c_int);
            assert!(proj.is_null());
            assert_ne!(err_code, engine::ErrorCode::NoError as c_int);

            // Test with invalid XML
            let bad_data = b"not xml at all";
            err_code = 0;
            let proj = simlin_import_xmile(
                bad_data.as_ptr(),
                bad_data.len(),
                &mut err_code as *mut c_int,
            );
            assert!(proj.is_null());
            assert_ne!(err_code, engine::ErrorCode::NoError as c_int);

            // Test with invalid MDL
            err_code = 0;
            let proj = simlin_import_mdl(
                bad_data.as_ptr(),
                bad_data.len(),
                &mut err_code as *mut c_int,
            );
            assert!(proj.is_null());
            assert_ne!(err_code, engine::ErrorCode::NoError as c_int);
        }
    }

    #[test]
    fn test_export_null_project() {
        unsafe {
            let mut output: *mut u8 = std::ptr::null_mut();
            let mut output_len: usize = 0;
            let rc = simlin_export_xmile(
                std::ptr::null_mut(),
                &mut output as *mut *mut u8,
                &mut output_len as *mut usize,
            );
            assert_ne!(rc, engine::ErrorCode::NoError as c_int);
            assert!(output.is_null());
        }
    }

    #[test]
    fn test_error_api_with_valid_project() {
        // Create a project with intentional errors
        let project = engine::project_io::Project {
            name: "test_errors".to_string(),
            sim_specs: Some(engine::project_io::SimSpecs {
                start: 0.0,
                stop: 10.0,
                dt: Some(engine::project_io::Dt {
                    value: 1.0,
                    is_reciprocal: false,
                }),
                save_step: None,
                sim_method: engine::project_io::SimMethod::Euler as i32,
                time_units: String::new(),
            }),
            models: vec![engine::project_io::Model {
                name: "main".to_string(),
                variables: vec![
                    // Variable with an equation error (unknown dependency)
                    engine::project_io::Variable {
                        v: Some(engine::project_io::variable::V::Aux(
                            engine::project_io::variable::Aux {
                                ident: "error_var".to_string(),
                                equation: Some(engine::project_io::variable::Equation {
                                    equation: Some(
                                        engine::project_io::variable::equation::Equation::Scalar(
                                            engine::project_io::variable::ScalarEquation {
                                                equation: "unknown_var + 1".to_string(),
                                                initial_equation: None,
                                            },
                                        ),
                                    ),
                                }),
                                documentation: String::new(),
                                units: String::new(),
                                gf: None,
                                can_be_module_input: false,
                                visibility: engine::project_io::variable::Visibility::Private
                                    as i32,
                                uid: 0,
                            },
                        )),
                    },
                    // Variable with bad units
                    engine::project_io::Variable {
                        v: Some(engine::project_io::variable::V::Aux(
                            engine::project_io::variable::Aux {
                                ident: "bad_units_var".to_string(),
                                equation: Some(engine::project_io::variable::Equation {
                                    equation: Some(
                                        engine::project_io::variable::equation::Equation::Scalar(
                                            engine::project_io::variable::ScalarEquation {
                                                equation: "1".to_string(),
                                                initial_equation: None,
                                            },
                                        ),
                                    ),
                                }),
                                documentation: String::new(),
                                units: "bad units here!!!".to_string(),
                                gf: None,
                                can_be_module_input: false,
                                visibility: engine::project_io::variable::Visibility::Private
                                    as i32,
                                uid: 0,
                            },
                        )),
                    },
                ],
                views: vec![],
                loop_metadata: vec![],
            }],
            dimensions: vec![],
            units: vec![],
            source: None,
        };
        let mut buf = Vec::new();
        project.encode(&mut buf).unwrap();

        unsafe {
            let mut err: c_int = 0;
            let proj = simlin_project_open(buf.as_ptr(), buf.len(), &mut err);
            assert!(!proj.is_null());
            assert_eq!(err, engine::ErrorCode::NoError as c_int);

            // Test getting all errors
            let all_errors = simlin_project_get_errors(proj);
            assert!(!all_errors.is_null());
            assert!((*all_errors).count > 0);

            // Verify we can access error details
            let error_slice = std::slice::from_raw_parts((*all_errors).errors, (*all_errors).count);
            let mut found_unknown_dep = false;
            let mut found_bad_units = false;

            for error in error_slice {
                if error.code == SimlinErrorCode::UnknownDependency {
                    found_unknown_dep = true;
                    assert!(!error.variable_name.is_null());
                    let var_name = CStr::from_ptr(error.variable_name).to_str().unwrap();
                    assert_eq!(var_name, "error_var");
                }
                // Bad units will show up as an error during parsing
                if !error.variable_name.is_null() {
                    let var_name = CStr::from_ptr(error.variable_name).to_str().unwrap();
                    if var_name == "bad_units_var" {
                        found_bad_units = true;
                    }
                }
            }

            assert!(
                found_unknown_dep,
                "Should have found unknown dependency error"
            );
            assert!(found_bad_units, "Should have found bad units error");

            // Clean up
            simlin_free_error_details(all_errors);
            simlin_project_unref(proj);
        }
    }

    #[test]
    fn test_error_api_with_compilation_errors() {
        // Create a project with compilation errors
        let project = engine::project_io::Project {
            name: "test_compilation_errors".to_string(),
            sim_specs: Some(engine::project_io::SimSpecs {
                start: 0.0,
                stop: 10.0,
                dt: Some(engine::project_io::Dt {
                    value: 1.0,
                    is_reciprocal: false,
                }),
                save_step: None,
                sim_method: engine::project_io::SimMethod::Euler as i32,
                time_units: String::new(),
            }),
            models: vec![engine::project_io::Model {
                name: "main".to_string(),
                variables: vec![
                    // This will cause a compilation error - circular reference
                    engine::project_io::Variable {
                        v: Some(engine::project_io::variable::V::Aux(
                            engine::project_io::variable::Aux {
                                ident: "a".to_string(),
                                equation: Some(engine::project_io::variable::Equation {
                                    equation: Some(
                                        engine::project_io::variable::equation::Equation::Scalar(
                                            engine::project_io::variable::ScalarEquation {
                                                equation: "b + 1".to_string(),
                                                initial_equation: None,
                                            },
                                        ),
                                    ),
                                }),
                                documentation: String::new(),
                                units: String::new(),
                                gf: None,
                                can_be_module_input: false,
                                visibility: engine::project_io::variable::Visibility::Private
                                    as i32,
                                uid: 0,
                            },
                        )),
                    },
                    engine::project_io::Variable {
                        v: Some(engine::project_io::variable::V::Aux(
                            engine::project_io::variable::Aux {
                                ident: "b".to_string(),
                                equation: Some(engine::project_io::variable::Equation {
                                    equation: Some(
                                        engine::project_io::variable::equation::Equation::Scalar(
                                            engine::project_io::variable::ScalarEquation {
                                                equation: "a + 1".to_string(),
                                                initial_equation: None,
                                            },
                                        ),
                                    ),
                                }),
                                documentation: String::new(),
                                units: String::new(),
                                gf: None,
                                can_be_module_input: false,
                                visibility: engine::project_io::variable::Visibility::Private
                                    as i32,
                                uid: 0,
                            },
                        )),
                    },
                ],
                views: vec![],
                loop_metadata: vec![],
            }],
            dimensions: vec![],
            units: vec![],
            source: None,
        };
        let mut buf = Vec::new();
        project.encode(&mut buf).unwrap();

        unsafe {
            let mut err: c_int = 0;
            let proj = simlin_project_open(buf.as_ptr(), buf.len(), &mut err);
            assert!(!proj.is_null());

            // The project should have compilation errors due to circular reference
            let all_errors = simlin_project_get_errors(proj);
            assert!(!all_errors.is_null());
            assert!((*all_errors).count > 0);

            // Verify we found the compilation error
            let error_slice = std::slice::from_raw_parts((*all_errors).errors, (*all_errors).count);
            let mut found_compilation_error = false;
            for error in error_slice {
                // Circular references or other compilation errors should be present
                if error.code == SimlinErrorCode::CircularDependency
                    || error.code == SimlinErrorCode::BadModelName
                    || error.code == SimlinErrorCode::Generic
                {
                    found_compilation_error = true;
                    break;
                }
            }
            assert!(
                found_compilation_error,
                "Should have found a compilation error"
            );

            // Clean up
            simlin_free_error_details(all_errors);
            simlin_project_unref(proj);
        }
    }

    #[test]
    fn test_error_api_no_errors() {
        // Create a valid project with no errors
        let project = engine::project_io::Project {
            name: "test_no_errors".to_string(),
            sim_specs: Some(engine::project_io::SimSpecs {
                start: 0.0,
                stop: 10.0,
                dt: Some(engine::project_io::Dt {
                    value: 1.0,
                    is_reciprocal: false,
                }),
                save_step: None,
                sim_method: engine::project_io::SimMethod::Euler as i32,
                time_units: String::new(),
            }),
            models: vec![engine::project_io::Model {
                name: "main".to_string(),
                variables: vec![engine::project_io::Variable {
                    v: Some(engine::project_io::variable::V::Aux(
                        engine::project_io::variable::Aux {
                            ident: "time_var".to_string(),
                            equation: Some(engine::project_io::variable::Equation {
                                equation: Some(
                                    engine::project_io::variable::equation::Equation::Scalar(
                                        engine::project_io::variable::ScalarEquation {
                                            equation: "time".to_string(),
                                            initial_equation: None,
                                        },
                                    ),
                                ),
                            }),
                            documentation: String::new(),
                            units: String::new(),
                            gf: None,
                            can_be_module_input: false,
                            visibility: engine::project_io::variable::Visibility::Private as i32,
                            uid: 0,
                        },
                    )),
                }],
                views: vec![],
                loop_metadata: vec![],
            }],
            dimensions: vec![],
            units: vec![],
            source: None,
        };
        let mut buf = Vec::new();
        project.encode(&mut buf).unwrap();

        unsafe {
            let mut err: c_int = 0;
            let proj = simlin_project_open(buf.as_ptr(), buf.len(), &mut err);
            assert!(!proj.is_null());

            // Test that there are no errors (including compilation errors)
            let all_errors = simlin_project_get_errors(proj);
            assert!(all_errors.is_null());

            // Clean up
            simlin_project_unref(proj);
        }
    }

    #[test]
    fn test_error_api_null_safety() {
        unsafe {
            // Test with null project
            let errors = simlin_project_get_errors(ptr::null_mut());
            assert!(errors.is_null());

            // Test free functions with null (should not crash)
            simlin_free_error_details(ptr::null_mut());
            simlin_free_error_detail(ptr::null_mut());
        }
    }

    #[test]
    fn test_error_offsets() {
        // Create a project with an error at a specific location
        let project = engine::project_io::Project {
            name: "test_offsets".to_string(),
            sim_specs: Some(engine::project_io::SimSpecs {
                start: 0.0,
                stop: 10.0,
                dt: Some(engine::project_io::Dt {
                    value: 1.0,
                    is_reciprocal: false,
                }),
                save_step: None,
                sim_method: engine::project_io::SimMethod::Euler as i32,
                time_units: String::new(),
            }),
            models: vec![engine::project_io::Model {
                name: "main".to_string(),
                variables: vec![engine::project_io::Variable {
                    v: Some(engine::project_io::variable::V::Aux(
                        engine::project_io::variable::Aux {
                            ident: "var_with_offset_error".to_string(),
                            equation: Some(engine::project_io::variable::Equation {
                                equation: Some(
                                    engine::project_io::variable::equation::Equation::Scalar(
                                        engine::project_io::variable::ScalarEquation {
                                            equation: "1 + unknown_var_here".to_string(),
                                            initial_equation: None,
                                        },
                                    ),
                                ),
                            }),
                            documentation: String::new(),
                            units: String::new(),
                            gf: None,
                            can_be_module_input: false,
                            visibility: engine::project_io::variable::Visibility::Private as i32,
                            uid: 0,
                        },
                    )),
                }],
                views: vec![],
                loop_metadata: vec![],
            }],
            dimensions: vec![],
            units: vec![],
            source: None,
        };
        let mut buf = Vec::new();
        project.encode(&mut buf).unwrap();

        unsafe {
            let mut err: c_int = 0;
            let proj = simlin_project_open(buf.as_ptr(), buf.len(), &mut err);
            assert!(!proj.is_null());

            let all_errors = simlin_project_get_errors(proj);
            assert!(!all_errors.is_null());
            assert!((*all_errors).count > 0);

            // Check that offsets are set (they should point to "unknown_var_here")
            let error_slice = std::slice::from_raw_parts((*all_errors).errors, (*all_errors).count);
            for error in error_slice {
                if error.code == SimlinErrorCode::UnknownDependency {
                    // The offset should point to the unknown variable reference
                    assert!(
                        error.start_offset > 0 || error.end_offset > 0,
                        "Error offsets should be set for unknown dependency"
                    );
                }
            }

            // Clean up
            simlin_free_error_details(all_errors);
            simlin_project_unref(proj);
        }
    }

    #[test]
    fn test_sim_lifecycle() {
        // Create a minimal valid protobuf project
        let project = engine::project_io::Project {
            name: "test".to_string(),
            sim_specs: Some(engine::project_io::SimSpecs {
                start: 0.0,
                stop: 10.0,
                dt: Some(engine::project_io::Dt {
                    value: 1.0,
                    is_reciprocal: false,
                }),
                save_step: None,
                sim_method: engine::project_io::SimMethod::Euler as i32,
                time_units: String::new(),
            }),
            models: vec![engine::project_io::Model {
                name: "main".to_string(),
                variables: vec![engine::project_io::Variable {
                    v: Some(engine::project_io::variable::V::Aux(
                        engine::project_io::variable::Aux {
                            ident: "time".to_string(),
                            equation: Some(engine::project_io::variable::Equation {
                                equation: Some(
                                    engine::project_io::variable::equation::Equation::Scalar(
                                        engine::project_io::variable::ScalarEquation {
                                            equation: "time".to_string(),
                                            initial_equation: None,
                                        },
                                    ),
                                ),
                            }),
                            documentation: String::new(),
                            units: String::new(),
                            gf: None,
                            can_be_module_input: false,
                            visibility: engine::project_io::variable::Visibility::Private as i32,
                            uid: 0,
                        },
                    )),
                }],
                views: vec![],
                loop_metadata: vec![],
            }],
            dimensions: vec![],
            units: vec![],
            source: None,
        };
        let mut buf = Vec::new();
        project.encode(&mut buf).unwrap();
        unsafe {
            let mut err: c_int = 0;
            let proj = simlin_project_open(buf.as_ptr(), buf.len(), &mut err);
            assert!(!proj.is_null());
            assert_eq!(err, engine::ErrorCode::NoError as c_int);
            let model = simlin_project_get_model(proj, ptr::null());
            assert!(!model.is_null());
            // Project ref count should have increased when model was created
            assert_eq!((*proj).ref_count.load(Ordering::SeqCst), 2);

            // Test model reference counting
            simlin_model_ref(model);
            assert_eq!((*model).ref_count.load(Ordering::SeqCst), 2);
            simlin_model_unref(model);
            assert_eq!((*model).ref_count.load(Ordering::SeqCst), 1);

            let sim = simlin_sim_new(model, false);
            assert!(!sim.is_null());
            // Model ref count should have increased when sim was created
            assert_eq!((*model).ref_count.load(Ordering::SeqCst), 2);

            // Test sim reference counting
            simlin_sim_ref(sim);
            assert_eq!((*sim).ref_count.load(Ordering::SeqCst), 2);
            simlin_sim_unref(sim);
            assert_eq!((*sim).ref_count.load(Ordering::SeqCst), 1);
            simlin_sim_unref(sim);
            // Sim should be freed now, model ref count should decrease
            assert_eq!((*model).ref_count.load(Ordering::SeqCst), 1);

            simlin_model_unref(model);
            // Model should be freed now, project ref count should decrease
            assert_eq!((*proj).ref_count.load(Ordering::SeqCst), 1);

            simlin_project_unref(proj);
        }
    }

    #[test]
    fn test_analyze_get_links() {
        // Create a project with a reinforcing loop using TestProject
        let test_project = TestProject::new("test_links")
            .with_sim_time(0.0, 10.0, 1.0)
            .stock("population", "100", &["births"], &[], None)
            .flow("births", "population * birth_rate", None)
            .aux("birth_rate", "0.02", None);

        // Build the datamodel and serialize to protobuf
        let datamodel_project = test_project.build_datamodel();
        let project = engine::serde::serialize(&datamodel_project);

        let mut buf = Vec::new();
        project.encode(&mut buf).unwrap();

        unsafe {
            let mut err: c_int = 0;
            let proj = simlin_project_open(buf.as_ptr(), buf.len(), &mut err);
            assert!(!proj.is_null());
            assert_eq!(err, engine::ErrorCode::NoError as c_int);

            // Test without LTM enabled - should get structural links only
            let model = simlin_project_get_model(proj, ptr::null());
            assert!(!model.is_null());
            let sim = simlin_sim_new(model, false);
            assert!(!sim.is_null());

            let links = simlin_analyze_get_links(sim);
            assert!(!links.is_null());
            assert!((*links).count > 0, "Should have detected causal links");

            // Verify link structure
            let links_slice = std::slice::from_raw_parts((*links).links, (*links).count);

            // Should have links like:
            // - birth_rate -> births
            // - population -> births
            // - births -> population
            let mut found_rate_to_births = false;
            let mut found_pop_to_births = false;
            let mut found_births_to_pop = false;

            for link in links_slice {
                assert!(!link.from.is_null());
                assert!(!link.to.is_null());

                let from = CStr::from_ptr(link.from).to_str().unwrap();
                let to = CStr::from_ptr(link.to).to_str().unwrap();

                if from == "birth_rate" && to == "births" {
                    found_rate_to_births = true;
                }
                if from == "population" && to == "births" {
                    found_pop_to_births = true;
                }
                if from == "births" && to == "population" {
                    found_births_to_pop = true;
                }

                // Without LTM, scores should be null
                assert!(link.score.is_null(), "Score should be null without LTM");
                assert_eq!(link.score_len, 0, "Score length should be 0 without LTM");
            }

            assert!(
                found_rate_to_births,
                "Should find birth_rate -> births link"
            );
            assert!(found_pop_to_births, "Should find population -> births link");
            assert!(found_births_to_pop, "Should find births -> population link");

            simlin_free_links(links);

            // Now test with LTM enabled
            // Create new sim with LTM enabled
            let model_ltm = simlin_project_get_model(proj, ptr::null());
            assert!(!model_ltm.is_null());
            let sim_ltm = simlin_sim_new(model_ltm, true);
            assert!(!sim_ltm.is_null());

            // Run simulation to generate score data
            let rc = simlin_sim_run_to_end(sim_ltm);
            assert_eq!(rc, engine::ErrorCode::NoError as c_int);

            // Get links with scores
            let links_with_scores = simlin_analyze_get_links(sim_ltm);
            assert!(!links_with_scores.is_null());
            assert!((*links_with_scores).count > 0);

            let links_slice =
                std::slice::from_raw_parts((*links_with_scores).links, (*links_with_scores).count);

            // Verify that scores are now populated
            for link in links_slice {
                let from = CStr::from_ptr(link.from).to_str().unwrap();
                let to = CStr::from_ptr(link.to).to_str().unwrap();

                // Links in the feedback loop should have scores
                if (from == "births" && to == "population")
                    || (from == "population" && to == "births")
                {
                    assert!(
                        !link.score.is_null(),
                        "Feedback loop links should have scores"
                    );
                    assert!(
                        link.score_len > 0,
                        "Score length should be > 0 for feedback links"
                    );

                    // Check that scores contain reasonable values
                    let scores = std::slice::from_raw_parts(link.score, link.score_len);
                    for &score in scores {
                        assert!(score.is_finite(), "Score should be finite");
                    }
                }
            }

            simlin_free_links(links_with_scores);

            // Clean up
            simlin_sim_unref(sim);
            simlin_sim_unref(sim_ltm);
            simlin_model_unref(model);
            simlin_model_unref(model_ltm);
            simlin_project_unref(proj);
        }
    }

    #[test]
    fn test_analyze_get_links_no_loops() {
        // Create a project with no feedback loops
        let test_project = TestProject::new("test_no_loops")
            .with_sim_time(0.0, 10.0, 1.0)
            .aux("input", "10", None)
            .aux("output", "input * 2", None);

        // Build the datamodel and serialize to protobuf
        let datamodel_project = test_project.build_datamodel();
        let project = engine::serde::serialize(&datamodel_project);

        let mut buf = Vec::new();
        project.encode(&mut buf).unwrap();

        unsafe {
            let mut err: c_int = 0;
            let proj = simlin_project_open(buf.as_ptr(), buf.len(), &mut err);
            assert!(!proj.is_null());

            let model = simlin_project_get_model(proj, ptr::null());
            assert!(!model.is_null());
            let sim = simlin_sim_new(model, false);
            assert!(!sim.is_null());

            let links = simlin_analyze_get_links(sim);
            assert!(!links.is_null());

            // Should still find the causal link from input to output
            assert!((*links).count > 0, "Should find input -> output link");

            let links_slice = std::slice::from_raw_parts((*links).links, (*links).count);
            let mut found_link = false;
            for link in links_slice {
                let from = CStr::from_ptr(link.from).to_str().unwrap();
                let to = CStr::from_ptr(link.to).to_str().unwrap();

                if from == "input" && to == "output" {
                    found_link = true;
                    // Non-loop links will have Unknown polarity since we don't analyze them
                    assert_eq!(link.polarity, SimlinLinkPolarity::Unknown);
                }
            }
            assert!(found_link, "Should find input -> output link");

            simlin_free_links(links);
            simlin_sim_unref(sim);
            simlin_model_unref(model);
            simlin_project_unref(proj);
        }
    }

    #[test]
    fn test_analyze_get_links_null_safety() {
        unsafe {
            // Test with null sim
            let links = simlin_analyze_get_links(ptr::null_mut());
            assert!(links.is_null());

            // Test free with null (should not crash)
            simlin_free_links(ptr::null_mut());
        }
    }

    #[test]
    fn test_analyze_get_relative_loop_score_renamed() {
        // Create a project with a reinforcing loop
        let test_project = TestProject::new("test_renamed")
            .with_sim_time(0.0, 10.0, 1.0)
            .stock("population", "100", &["births"], &[], None)
            .flow("births", "population * 0.02", None);

        let datamodel_project = test_project.build_datamodel();
        let project = engine::serde::serialize(&datamodel_project);

        let mut buf = Vec::new();
        project.encode(&mut buf).unwrap();

        unsafe {
            let mut err: c_int = 0;
            let proj = simlin_project_open(buf.as_ptr(), buf.len(), &mut err);
            assert!(!proj.is_null());

            // Create simulation with LTM enabled

            let model = simlin_project_get_model(proj, ptr::null());
            assert!(!model.is_null());
            let sim = simlin_sim_new(model, true); // Enable LTM for relative loop scores
            assert!(!sim.is_null());

            // Run simulation
            let rc = simlin_sim_run_to_end(sim);
            assert_eq!(rc, engine::ErrorCode::NoError as c_int);

            // Get loops to find loop ID
            let loops = simlin_analyze_get_loops(proj);
            assert!(!loops.is_null());
            assert!((*loops).count > 0);

            let loop_slice = std::slice::from_raw_parts((*loops).loops, (*loops).count);
            let loop_id = CStr::from_ptr(loop_slice[0].id).to_str().unwrap();

            // Test renamed function
            let step_count = simlin_sim_get_stepcount(sim);
            let mut scores = vec![0.0; step_count as usize];

            let loop_id_c = CString::new(loop_id).unwrap();
            let rc = simlin_analyze_get_relative_loop_score(
                sim,
                loop_id_c.as_ptr(),
                scores.as_mut_ptr(),
                scores.len(),
            );
            assert_eq!(rc, 0, "Should successfully get relative loop scores");

            // Verify scores are reasonable
            // Since there's only one loop, relative score should be 1.0
            for score in &scores {
                assert!(score.is_finite());
                assert_eq!(*score, 1.0, "Single loop should have relative score of 1.0");
            }

            simlin_free_loops(loops);
            simlin_sim_unref(sim);
            simlin_model_unref(model);
            simlin_project_unref(proj);
        }
    }

    #[test]
    fn test_project_serialize() {
        // Create a project with some content
        let test_project = TestProject::new("test_serialize")
            .with_sim_time(0.0, 10.0, 1.0)
            .stock("population", "100", &["births"], &["deaths"], None)
            .flow("births", "population * birth_rate", None)
            .flow("deaths", "population * death_rate", None)
            .aux("birth_rate", "0.02", None)
            .aux("death_rate", "0.01", None);

        let datamodel_project = test_project.build_datamodel();
        let original_pb = engine::serde::serialize(&datamodel_project);

        let mut buf = Vec::new();
        original_pb.encode(&mut buf).unwrap();

        unsafe {
            // Open the project
            let mut err: c_int = 0;
            let proj = simlin_project_open(buf.as_ptr(), buf.len(), &mut err);
            assert!(!proj.is_null());
            assert_eq!(err, engine::ErrorCode::NoError as c_int);

            // Serialize it back out
            let mut output: *mut u8 = std::ptr::null_mut();
            let mut output_len: usize = 0;
            let rc = simlin_project_serialize(
                proj,
                &mut output as *mut *mut u8,
                &mut output_len as *mut usize,
            );
            assert_eq!(rc, engine::ErrorCode::NoError as c_int);
            assert!(!output.is_null());
            assert!(output_len > 0);

            // Verify we can open the serialized project
            let proj2 = simlin_project_open(output, output_len, &mut err);
            assert!(!proj2.is_null());
            assert_eq!(err, engine::ErrorCode::NoError as c_int);

            // Get models and create simulations from both projects and verify they work identically
            let model1 = simlin_project_get_model(proj, ptr::null());
            let model2 = simlin_project_get_model(proj2, ptr::null());
            assert!(!model1.is_null());
            assert!(!model2.is_null());

            let sim1 = simlin_sim_new(model1, false);
            let sim2 = simlin_sim_new(model2, false);
            assert!(!sim1.is_null());
            assert!(!sim2.is_null());

            // Run both simulations
            let rc1 = simlin_sim_run_to_end(sim1);
            let rc2 = simlin_sim_run_to_end(sim2);
            assert_eq!(rc1, engine::ErrorCode::NoError as c_int);
            assert_eq!(rc2, engine::ErrorCode::NoError as c_int);

            // Check they have same number of variables and steps
            let var_count1 = simlin_model_get_var_count(model1);
            let var_count2 = simlin_model_get_var_count(model2);
            assert_eq!(var_count1, var_count2);

            let step_count1 = simlin_sim_get_stepcount(sim1);
            let step_count2 = simlin_sim_get_stepcount(sim2);
            assert_eq!(step_count1, step_count2);

            // Clean up
            simlin_free(output);
            simlin_sim_unref(sim1);
            simlin_sim_unref(sim2);
            simlin_model_unref(model1);
            simlin_model_unref(model2);
            simlin_project_unref(proj);
            simlin_project_unref(proj2);
        }
    }

    #[test]
    fn test_project_serialize_with_ltm() {
        // Create a project with a loop
        let test_project = TestProject::new("test_serialize_ltm")
            .with_sim_time(0.0, 10.0, 1.0)
            .stock("stock", "100", &["inflow"], &[], None)
            .flow("inflow", "stock * 0.1", None);

        let datamodel_project = test_project.build_datamodel();
        let original_pb = engine::serde::serialize(&datamodel_project);

        let mut buf = Vec::new();
        original_pb.encode(&mut buf).unwrap();

        unsafe {
            let mut err: c_int = 0;
            let proj = simlin_project_open(buf.as_ptr(), buf.len(), &mut err);
            assert!(!proj.is_null());

            // LTM will be enabled when creating simulation

            // Serialize the project (should NOT include LTM variables)
            let mut output: *mut u8 = std::ptr::null_mut();
            let mut output_len: usize = 0;
            let rc = simlin_project_serialize(
                proj,
                &mut output as *mut *mut u8,
                &mut output_len as *mut usize,
            );
            assert_eq!(rc, engine::ErrorCode::NoError as c_int);

            // Open the serialized project
            let proj2 = simlin_project_open(output, output_len, &mut err);
            assert!(!proj2.is_null());

            // Create sims from both
            let model1 = simlin_project_get_model(proj, ptr::null());
            let model2 = simlin_project_get_model(proj2, ptr::null());
            assert!(!model1.is_null());
            assert!(!model2.is_null());

            let sim1 = simlin_sim_new(model1, true); // Has LTM
            let sim2 = simlin_sim_new(model2, false); // No LTM

            // Run both
            simlin_sim_run_to_end(sim1);
            simlin_sim_run_to_end(sim2);

            // Both original models should have the same number of variables
            // (they're from the same serialized project without LTM augmentation)
            let var_count1 = simlin_model_get_var_count(model1);
            let var_count2 = simlin_model_get_var_count(model2);
            assert_eq!(
                var_count1, var_count2,
                "Models from serialized projects should have same variable count"
            );

            // Clean up
            simlin_free(output);
            simlin_sim_unref(sim1);
            simlin_sim_unref(sim2);
            simlin_model_unref(model1);
            simlin_model_unref(model2);
            simlin_project_unref(proj);
            simlin_project_unref(proj2);
        }
    }

    #[test]
    fn test_project_serialize_null_safety() {
        unsafe {
            // Test with null project
            let mut output: *mut u8 = std::ptr::null_mut();
            let mut output_len: usize = 0;
            let rc = simlin_project_serialize(
                ptr::null_mut(),
                &mut output as *mut *mut u8,
                &mut output_len as *mut usize,
            );
            assert_ne!(rc, engine::ErrorCode::NoError as c_int);
            assert!(output.is_null());

            // Test with null output pointer
            let project = engine::project_io::Project {
                name: "test".to_string(),
                sim_specs: Some(engine::project_io::SimSpecs {
                    start: 0.0,
                    stop: 10.0,
                    dt: Some(engine::project_io::Dt {
                        value: 1.0,
                        is_reciprocal: false,
                    }),
                    save_step: None,
                    sim_method: engine::project_io::SimMethod::Euler as i32,
                    time_units: String::new(),
                }),
                models: vec![engine::project_io::Model {
                    name: "main".to_string(),
                    variables: vec![],
                    views: vec![],
                    loop_metadata: vec![],
                }],
                dimensions: vec![],
                units: vec![],
                source: None,
            };
            let mut buf = Vec::new();
            project.encode(&mut buf).unwrap();

            let mut err: c_int = 0;
            let proj = simlin_project_open(buf.as_ptr(), buf.len(), &mut err);
            assert!(!proj.is_null());

            let rc = simlin_project_serialize(proj, ptr::null_mut(), &mut output_len as *mut usize);
            assert_ne!(rc, engine::ErrorCode::NoError as c_int);

            // Test with null output_len pointer
            let rc = simlin_project_serialize(proj, &mut output as *mut *mut u8, ptr::null_mut());
            assert_ne!(rc, engine::ErrorCode::NoError as c_int);

            simlin_project_unref(proj);
        }
    }

    #[test]
    fn test_model_functions() {
        // Create a project with multiple models
        let project = engine::project_io::Project {
            name: "test_multi_model".to_string(),
            sim_specs: Some(engine::project_io::SimSpecs {
                start: 0.0,
                stop: 10.0,
                dt: Some(engine::project_io::Dt {
                    value: 1.0,
                    is_reciprocal: false,
                }),
                save_step: None,
                sim_method: engine::project_io::SimMethod::Euler as i32,
                time_units: String::new(),
            }),
            models: vec![
                engine::project_io::Model {
                    name: "model1".to_string(),
                    variables: vec![
                        engine::project_io::Variable {
                            v: Some(engine::project_io::variable::V::Aux(
                                engine::project_io::variable::Aux {
                                    ident: "var1".to_string(),
                                    equation: Some(engine::project_io::variable::Equation {
                                        equation: Some(
                                            engine::project_io::variable::equation::Equation::Scalar(
                                                engine::project_io::variable::ScalarEquation {
                                                    equation: "1".to_string(),
                                                    initial_equation: None,
                                                },
                                            ),
                                        ),
                                    }),
                                    documentation: String::new(),
                                    units: String::new(),
                                    gf: None,
                                    can_be_module_input: false,
                                    visibility: engine::project_io::variable::Visibility::Private as i32,
                                    uid: 0,
                                },
                            )),
                        },
                        engine::project_io::Variable {
                            v: Some(engine::project_io::variable::V::Aux(
                                engine::project_io::variable::Aux {
                                    ident: "var2".to_string(),
                                    equation: Some(engine::project_io::variable::Equation {
                                        equation: Some(
                                            engine::project_io::variable::equation::Equation::Scalar(
                                                engine::project_io::variable::ScalarEquation {
                                                    equation: "var1 * 2".to_string(),
                                                    initial_equation: None,
                                                },
                                            ),
                                        ),
                                    }),
                                    documentation: String::new(),
                                    units: String::new(),
                                    gf: None,
                                    can_be_module_input: false,
                                    visibility: engine::project_io::variable::Visibility::Private as i32,
                                    uid: 0,
                                },
                            )),
                        },
                    ],
                    views: vec![],
                    loop_metadata: vec![],
                },
                engine::project_io::Model {
                    name: "model2".to_string(),
                    variables: vec![
                        engine::project_io::Variable {
                            v: Some(engine::project_io::variable::V::Stock(
                                engine::project_io::variable::Stock {
                                    ident: "stock".to_string(),
                                    equation: Some(engine::project_io::variable::Equation {
                                        equation: Some(
                                            engine::project_io::variable::equation::Equation::Scalar(
                                                engine::project_io::variable::ScalarEquation {
                                                    equation: "100".to_string(),
                                                    initial_equation: None,
                                                },
                                            ),
                                        ),
                                    }),
                                    documentation: String::new(),
                                    units: String::new(),
                                    inflows: vec!["inflow".to_string()],
                                    outflows: vec![],
                                    non_negative: false,
                                    can_be_module_input: false,
                                    visibility: engine::project_io::variable::Visibility::Private as i32,
                                    uid: 0,
                                },
                            )),
                        },
                        engine::project_io::Variable {
                            v: Some(engine::project_io::variable::V::Flow(
                                engine::project_io::variable::Flow {
                                    ident: "inflow".to_string(),
                                    equation: Some(engine::project_io::variable::Equation {
                                        equation: Some(
                                            engine::project_io::variable::equation::Equation::Scalar(
                                                engine::project_io::variable::ScalarEquation {
                                                    equation: "rate * stock".to_string(),
                                                    initial_equation: None,
                                                },
                                            ),
                                        ),
                                    }),
                                    documentation: String::new(),
                                    units: String::new(),
                                    gf: None,
                                    non_negative: false,
                                    can_be_module_input: false,
                                    visibility: engine::project_io::variable::Visibility::Private as i32,
                                    uid: 0,
                                },
                            )),
                        },
                        engine::project_io::Variable {
                            v: Some(engine::project_io::variable::V::Aux(
                                engine::project_io::variable::Aux {
                                    ident: "rate".to_string(),
                                    equation: Some(engine::project_io::variable::Equation {
                                        equation: Some(
                                            engine::project_io::variable::equation::Equation::Scalar(
                                                engine::project_io::variable::ScalarEquation {
                                                    equation: "0.1".to_string(),
                                                    initial_equation: None,
                                                },
                                            ),
                                        ),
                                    }),
                                    documentation: String::new(),
                                    units: String::new(),
                                    gf: None,
                                    can_be_module_input: false,
                                    visibility: engine::project_io::variable::Visibility::Private as i32,
                                    uid: 0,
                                },
                            )),
                        },
                    ],
                    views: vec![],
                    loop_metadata: vec![],
                },
            ],
            dimensions: vec![],
            units: vec![],
            source: None,
        };

        let mut buf = Vec::new();
        project.encode(&mut buf).unwrap();

        unsafe {
            let mut err: c_int = 0;
            let proj = simlin_project_open(buf.as_ptr(), buf.len(), &mut err);
            assert!(!proj.is_null());
            assert_eq!(err, engine::ErrorCode::NoError as c_int);

            // Test simlin_project_get_model_count
            let model_count = simlin_project_get_model_count(proj);
            assert_eq!(model_count, 2, "Should have 2 models");

            // Test simlin_project_get_model_names
            let mut model_names: Vec<*mut c_char> = vec![ptr::null_mut(); 2];
            let count = simlin_project_get_model_names(proj, model_names.as_mut_ptr(), 2);
            assert_eq!(count, 2);

            let mut names = Vec::new();
            for name_ptr in &model_names {
                assert!(!name_ptr.is_null());
                let name = CStr::from_ptr(*name_ptr).to_string_lossy().into_owned();
                names.push(name.clone());
                simlin_free_string(*name_ptr);
            }
            assert!(names.contains(&"model1".to_string()));
            assert!(names.contains(&"model2".to_string()));

            // Test simlin_project_get_model with specific name
            let model1_name = CString::new("model1").unwrap();
            let model1 = simlin_project_get_model(proj, model1_name.as_ptr());
            assert!(!model1.is_null());
            assert_eq!((*model1).model_name, "model1");

            // Test simlin_project_get_model with null (should get first model)
            let model_default = simlin_project_get_model(proj, ptr::null());
            assert!(!model_default.is_null());
            assert_eq!((*model_default).model_name, "model1");

            // Test simlin_project_get_model with non-existent name (should get first model)
            let bad_name = CString::new("nonexistent").unwrap();
            let model_fallback = simlin_project_get_model(proj, bad_name.as_ptr());
            assert!(!model_fallback.is_null());
            assert_eq!((*model_fallback).model_name, "model1");

            // Test simlin_model_get_var_count
            let model2_name = CString::new("model2").unwrap();
            let model2 = simlin_project_get_model(proj, model2_name.as_ptr());
            assert!(!model2.is_null());

            let var_count = simlin_model_get_var_count(model2);
            assert!(
                var_count >= 3,
                "model2 should have at least 3 variables (stock, inflow, rate)"
            );

            // Test simlin_model_get_var_names
            let mut var_names: Vec<*mut c_char> = vec![ptr::null_mut(); var_count as usize];
            let count =
                simlin_model_get_var_names(model2, var_names.as_mut_ptr(), var_count as usize);
            assert_eq!(count, var_count);

            let mut var_name_strings = Vec::new();
            for name_ptr in &var_names {
                assert!(!name_ptr.is_null());
                let name = CStr::from_ptr(*name_ptr).to_string_lossy().into_owned();
                var_name_strings.push(name.clone());
                simlin_free_string(*name_ptr);
            }
            assert!(var_name_strings.contains(&"stock".to_string()));
            assert!(var_name_strings.contains(&"inflow".to_string()));
            assert!(var_name_strings.contains(&"rate".to_string()));
            // time may or may not be included depending on compilation

            // Test simlin_model_get_links
            let links = simlin_model_get_links(model2);
            assert!(!links.is_null());
            assert!((*links).count > 0, "Should have causal links");

            // Verify link structure
            let links_slice = std::slice::from_raw_parts((*links).links, (*links).count);
            let mut found_rate_to_inflow = false;
            let mut found_stock_to_inflow = false;
            let mut found_inflow_to_stock = false;

            for link in links_slice {
                assert!(!link.from.is_null());
                assert!(!link.to.is_null());

                let from = CStr::from_ptr(link.from).to_str().unwrap();
                let to = CStr::from_ptr(link.to).to_str().unwrap();

                if from == "rate" && to == "inflow" {
                    found_rate_to_inflow = true;
                }
                if from == "stock" && to == "inflow" {
                    found_stock_to_inflow = true;
                }
                if from == "inflow" && to == "stock" {
                    found_inflow_to_stock = true;
                }

                // Model-level links should not have scores
                assert!(link.score.is_null());
                assert_eq!(link.score_len, 0);
            }

            assert!(found_rate_to_inflow, "Should find rate -> inflow link");
            assert!(found_stock_to_inflow, "Should find stock -> inflow link");
            assert!(found_inflow_to_stock, "Should find inflow -> stock link");

            simlin_free_links(links);

            // Clean up
            simlin_model_unref(model1);
            simlin_model_unref(model2);
            simlin_model_unref(model_default);
            simlin_model_unref(model_fallback);
            simlin_project_unref(proj);
        }
    }

    #[test]
    fn test_model_null_safety() {
        unsafe {
            // Test null project
            let count = simlin_project_get_model_count(ptr::null_mut());
            assert_eq!(count, 0);

            let mut names: [*mut c_char; 2] = [ptr::null_mut(); 2];
            let count = simlin_project_get_model_names(ptr::null_mut(), names.as_mut_ptr(), 2);
            assert_eq!(count, engine::ErrorCode::Generic as c_int);

            let model = simlin_project_get_model(ptr::null_mut(), ptr::null());
            assert!(model.is_null());

            // Test null model
            simlin_model_ref(ptr::null_mut());
            simlin_model_unref(ptr::null_mut());

            let count = simlin_model_get_var_count(ptr::null_mut());
            assert_eq!(count, -1);

            let mut names: [*mut c_char; 2] = [ptr::null_mut(); 2];
            let count = simlin_model_get_var_names(ptr::null_mut(), names.as_mut_ptr(), 2);
            assert_eq!(count, engine::ErrorCode::Generic as c_int);

            let links = simlin_model_get_links(ptr::null_mut());
            assert!(links.is_null());

            // Test null sim creation
            let sim = simlin_sim_new(ptr::null_mut(), false);
            assert!(sim.is_null());
        }
    }

    #[test]
    fn test_ltm_enabled_sim() {
        // Create a project with a feedback loop
        let test_project = TestProject::new("test_ltm")
            .with_sim_time(0.0, 10.0, 1.0)
            .stock("population", "100", &["births"], &[], None)
            .flow("births", "population * 0.02", None);

        let datamodel_project = test_project.build_datamodel();
        let project = engine::serde::serialize(&datamodel_project);

        let mut buf = Vec::new();
        project.encode(&mut buf).unwrap();

        unsafe {
            let mut err: c_int = 0;
            let proj = simlin_project_open(buf.as_ptr(), buf.len(), &mut err);
            assert!(!proj.is_null());
            assert_eq!(err, engine::ErrorCode::NoError as c_int);

            let model = simlin_project_get_model(proj, ptr::null());
            assert!(!model.is_null());

            // Create simulation with LTM enabled
            let sim_ltm = simlin_sim_new(model, true);
            assert!(!sim_ltm.is_null());

            // Run simulation
            let rc = simlin_sim_run_to_end(sim_ltm);
            assert_eq!(rc, engine::ErrorCode::NoError as c_int);

            // Create another sim without LTM
            let sim_no_ltm = simlin_sim_new(model, false);
            assert!(!sim_no_ltm.is_null());

            // Run this one too
            let rc = simlin_sim_run_to_end(sim_no_ltm);
            assert_eq!(rc, engine::ErrorCode::NoError as c_int);

            // Clean up
            simlin_sim_unref(sim_ltm);
            simlin_sim_unref(sim_no_ltm);
            simlin_model_unref(model);
            simlin_project_unref(proj);
        }
    }

    #[test]
    fn test_get_incoming_links() {
        // Create a project with a flow that depends on a rate and a stock using TestProject
        let test_project = TestProject::new("test")
            .with_sim_time(0.0, 10.0, 1.0)
            .stock("Stock", "100", &["flow"], &[], None)
            .flow("flow", "rate * Stock", None)
            .aux("rate", "0.5", None);

        // Build the datamodel and serialize to protobuf
        let datamodel_project = test_project.build_datamodel();
        let project = engine::serde::serialize(&datamodel_project);

        let mut buf = Vec::new();
        project.encode(&mut buf).unwrap();

        unsafe {
            let mut err: c_int = 0;
            let proj = simlin_project_open(buf.as_ptr(), buf.len(), &mut err);
            assert!(!proj.is_null());
            assert_eq!(err, engine::ErrorCode::NoError as c_int);

            let model = simlin_project_get_model(proj, ptr::null());
            assert!(!model.is_null());
            let sim = simlin_sim_new(model, false);
            assert!(!sim.is_null());

            // Test getting incoming links for the flow
            let flow_name = CString::new("flow").unwrap();

            // Test 1: Query the number of dependencies with max=0
            let count = simlin_model_get_incoming_links(
                model,
                flow_name.as_ptr(),
                ptr::null_mut(), // result can be null when max=0
                0,
            );
            assert_eq!(count, 2, "Expected 2 dependencies for flow when querying");

            // Test 2: Try with insufficient array size (should return error)
            let mut small_links: [*mut c_char; 1] = [ptr::null_mut(); 1];
            let count = simlin_model_get_incoming_links(
                model,
                flow_name.as_ptr(),
                small_links.as_mut_ptr(),
                1, // Only room for 1, but there are 2 dependencies
            );
            assert_eq!(
                count,
                engine::ErrorCode::Generic as c_int,
                "Expected Generic error when array too small"
            );

            // Test 3: Proper usage - query then allocate
            let count =
                simlin_model_get_incoming_links(model, flow_name.as_ptr(), ptr::null_mut(), 0);
            assert_eq!(count, 2);

            // Allocate exact size needed
            let mut links = vec![ptr::null_mut::<c_char>(); count as usize];
            let count2 = simlin_model_get_incoming_links(
                model,
                flow_name.as_ptr(),
                links.as_mut_ptr(),
                count as usize,
            );
            assert_eq!(
                count2, count,
                "Should return same count when array is exact size"
            );

            // Collect the dependency names
            let mut dep_names = Vec::new();
            for link in links.iter().take(count2 as usize) {
                assert!(!link.is_null());
                let dep_name = CStr::from_ptr(*link).to_string_lossy().into_owned();
                dep_names.push(dep_name);
                simlin_free_string(*link);
            }

            // Check that we got both "rate" and "stock" (canonicalized to lowercase)
            assert!(
                dep_names.contains(&"rate".to_string()),
                "Missing 'rate' dependency"
            );
            assert!(
                dep_names.contains(&"stock".to_string()),
                "Missing 'stock' dependency"
            );

            // Test getting incoming links for rate (should have none since it's a constant)
            let rate_name = CString::new("rate").unwrap();
            let count =
                simlin_model_get_incoming_links(model, rate_name.as_ptr(), ptr::null_mut(), 0);
            assert_eq!(count, 0, "Expected 0 dependencies for rate");

            // Test getting incoming links for stock (initial value is constant, so no deps)
            let stock_name = CString::new("Stock").unwrap();
            let count =
                simlin_model_get_incoming_links(model, stock_name.as_ptr(), ptr::null_mut(), 0);
            assert_eq!(
                count, 0,
                "Expected 0 dependencies for Stock's initial value"
            );

            // Test error cases
            // Non-existent variable
            let nonexistent = CString::new("nonexistent").unwrap();
            let count =
                simlin_model_get_incoming_links(model, nonexistent.as_ptr(), ptr::null_mut(), 0);
            assert_eq!(
                count,
                engine::ErrorCode::DoesNotExist as c_int,
                "Expected DoesNotExist error code for non-existent variable"
            );

            // Null pointer checks
            let count = simlin_model_get_incoming_links(
                ptr::null_mut(),
                flow_name.as_ptr(),
                ptr::null_mut(),
                0,
            );
            assert_eq!(
                count,
                engine::ErrorCode::Generic as c_int,
                "Expected Generic error code for null model"
            );

            let count = simlin_model_get_incoming_links(model, ptr::null(), ptr::null_mut(), 0);
            assert_eq!(
                count,
                engine::ErrorCode::Generic as c_int,
                "Expected Generic error code for null var_name"
            );

            // Test that result being null with max > 0 is an error
            let count =
                simlin_model_get_incoming_links(model, flow_name.as_ptr(), ptr::null_mut(), 10);
            assert_eq!(
                count,
                engine::ErrorCode::Generic as c_int,
                "Expected Generic error code for null result with max > 0"
            );

            // Clean up
            simlin_sim_unref(sim);
            simlin_model_unref(model);
            simlin_project_unref(proj);
        }
    }

    #[test]
    fn test_project_add_model() {
        use prost::Message;

        // Create a minimal project with just one model
        let project = engine::project_io::Project {
            name: "test_project".to_string(),
            sim_specs: Some(engine::project_io::SimSpecs {
                start: 0.0,
                stop: 100.0,
                dt: Some(engine::project_io::Dt {
                    value: 0.25,
                    is_reciprocal: false,
                }),
                save_step: None,
                sim_method: engine::project_io::SimMethod::Euler as i32,
                time_units: String::new(),
            }),
            models: vec![engine::project_io::Model {
                name: "main".to_string(),
                variables: vec![],
                views: vec![],
                loop_metadata: vec![],
            }],
            dimensions: vec![],
            units: vec![],
            source: None,
        };
        let mut buf = Vec::new();
        project.encode(&mut buf).unwrap();

        unsafe {
            // Open the project
            let mut err: c_int = 0;
            let proj = simlin_project_open(buf.as_ptr(), buf.len(), &mut err);
            assert!(!proj.is_null());
            assert_eq!(err, engine::ErrorCode::NoError as c_int);

            // Verify initial model count
            let initial_count = simlin_project_get_model_count(proj);
            assert_eq!(initial_count, 1);

            // Test adding a model
            let model_name = CString::new("new_model").unwrap();
            let rc = simlin_project_add_model(proj, model_name.as_ptr());
            assert_eq!(rc, engine::ErrorCode::NoError as c_int);

            // Verify model count increased
            let new_count = simlin_project_get_model_count(proj);
            assert_eq!(new_count, 2);

            // Verify we can get the new model
            let new_model = simlin_project_get_model(proj, model_name.as_ptr());
            assert!(!new_model.is_null());
            assert_eq!((*new_model).model_name, "new_model");

            // Verify the new model can be used to create a simulation
            let sim = simlin_sim_new(new_model, false);
            assert!(!sim.is_null());

            // Clean up
            simlin_sim_unref(sim);
            simlin_model_unref(new_model);

            // Test adding another model
            let model_name2 = CString::new("another_model").unwrap();
            let rc = simlin_project_add_model(proj, model_name2.as_ptr());
            assert_eq!(rc, engine::ErrorCode::NoError as c_int);

            // Verify model count
            let final_count = simlin_project_get_model_count(proj);
            assert_eq!(final_count, 3);

            // Test adding duplicate model name (should fail)
            let duplicate_name = CString::new("new_model").unwrap();
            let rc = simlin_project_add_model(proj, duplicate_name.as_ptr());
            assert_eq!(rc, engine::ErrorCode::DuplicateVariable as c_int);

            // Model count should not have changed
            let count_after_dup = simlin_project_get_model_count(proj);
            assert_eq!(count_after_dup, 3);

            // Clean up
            simlin_project_unref(proj);
        }
    }

    #[test]
    fn test_project_add_model_null_safety() {
        unsafe {
            // Test with null project
            let model_name = CString::new("test").unwrap();
            let rc = simlin_project_add_model(ptr::null_mut(), model_name.as_ptr());
            assert_eq!(rc, engine::ErrorCode::Generic as c_int);

            // Create a valid project for other null tests
            let project = engine::project_io::Project {
                name: "test".to_string(),
                sim_specs: Some(engine::project_io::SimSpecs {
                    start: 0.0,
                    stop: 10.0,
                    dt: Some(engine::project_io::Dt {
                        value: 1.0,
                        is_reciprocal: false,
                    }),
                    save_step: None,
                    sim_method: engine::project_io::SimMethod::Euler as i32,
                    time_units: String::new(),
                }),
                models: vec![],
                dimensions: vec![],
                units: vec![],
                source: None,
            };
            let mut buf = Vec::new();
            project.encode(&mut buf).unwrap();

            let mut err: c_int = 0;
            let proj = simlin_project_open(buf.as_ptr(), buf.len(), &mut err);
            assert!(!proj.is_null());

            // Test with null model name
            let rc = simlin_project_add_model(proj, ptr::null());
            assert_eq!(rc, engine::ErrorCode::Generic as c_int);

            // Test with empty model name
            let empty_name = CString::new("").unwrap();
            let rc = simlin_project_add_model(proj, empty_name.as_ptr());
            assert_eq!(rc, engine::ErrorCode::Generic as c_int);

            // Clean up
            simlin_project_unref(proj);
        }
    }
}
