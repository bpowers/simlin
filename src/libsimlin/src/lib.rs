// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.
use prost::Message;
use simlin_engine::ltm::{detect_loops, LoopPolarity};
use simlin_engine::{self as engine, canonicalize, serde, Vm};
use std::alloc::{alloc, dealloc, Layout};
use std::ffi::{CStr, CString};
use std::io::BufReader;
use std::os::raw::{c_char, c_double, c_int};
use std::ptr;
use std::sync::atomic::{AtomicUsize, Ordering};

mod ffi;
pub use ffi::{SimlinLoop, SimlinLoopPolarity, SimlinLoops};

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
    ltm_project: Option<engine::Project>,
    ref_count: AtomicUsize,
}
/// Opaque simulation structure
pub struct SimlinSim {
    project: *const SimlinProject,
    model_name: String,
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
        ltm_project: None,
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
/// Enables LTM (Loops That Matter) analysis on a project
///
/// # Safety
/// - `project` must be a valid pointer to a SimlinProject
#[no_mangle]
pub unsafe extern "C" fn simlin_project_enable_ltm(project: *mut SimlinProject) -> c_int {
    if project.is_null() {
        return engine::ErrorCode::Generic as c_int;
    }
    let proj = &mut *project;
    // If LTM is already enabled, return success
    if proj.ltm_project.is_some() {
        return engine::ErrorCode::NoError as c_int;
    }
    // Create LTM-augmented project
    match proj.project.clone().with_ltm() {
        Ok(ltm_proj) => {
            proj.ltm_project = Some(ltm_proj);
            engine::ErrorCode::NoError as c_int
        }
        Err(e) => e.code as c_int,
    }
}
/// Creates a new simulation context
///
/// # Safety
/// - `project` must be a valid pointer to a SimlinProject
/// - `model_name` may be null (uses default model)
#[no_mangle]
pub unsafe extern "C" fn simlin_sim_new(
    project: *mut SimlinProject,
    model_name: *const c_char,
) -> *mut SimlinSim {
    if project.is_null() {
        return ptr::null_mut();
    }
    let mut model_name = if model_name.is_null() {
        "main".to_string()
    } else {
        match CStr::from_ptr(model_name).to_str() {
            Ok(s) => s.to_string(),
            Err(_) => return ptr::null_mut(),
        }
    };
    // Increment project reference count
    (*project).ref_count.fetch_add(1, Ordering::SeqCst);
    let mut sim = Box::new(SimlinSim {
        project,
        model_name: model_name.clone(),
        vm: None,
        results: None,
        ref_count: AtomicUsize::new(1),
    });
    // Initialize the VM - use LTM project if available
    let proj_to_use = if let Some(ref ltm_proj) = (*project).ltm_project {
        ltm_proj
    } else {
        &(*project).project
    };
    // Resolve model name: if requested model is not present, but there is exactly one
    // user model in the datamodel, use that as the root.
    if proj_to_use.datamodel.get_model(&model_name).is_none()
        && proj_to_use.datamodel.models.len() == 1
    {
        model_name = proj_to_use.datamodel.models[0].name.clone();
    }
    let compiler = engine::Simulation::new(proj_to_use, &model_name);
    if let Ok(compiler) = compiler {
        if let Ok(compiled) = compiler.compile() {
            if let Ok(vm) = Vm::new(compiled) {
                sim.vm = Some(vm);
            }
        }
    }
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
        // Decrement project reference count
        simlin_project_unref(sim.project as *mut SimlinProject);
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
/// Gets the number of variables in the model
///
/// # Safety
/// - `sim` must be a valid pointer to a SimlinSim
#[no_mangle]
pub unsafe extern "C" fn simlin_sim_get_varcount(sim: *mut SimlinSim) -> c_int {
    if sim.is_null() {
        return -1;
    }
    let sim = &*sim;
    if let Some(ref results) = sim.results {
        return results.offsets.len() as c_int;
    }
    if let Some(ref vm) = sim.vm {
        return vm.names_as_strs().len() as c_int;
    }
    -1
}
/// Gets the variable names
///
/// # Safety
/// - `sim` must be a valid pointer to a SimlinSim
/// - `result` must be a valid pointer to an array of at least `max` char pointers
/// - The returned strings are owned by the simulation and must not be freed
#[no_mangle]
pub unsafe extern "C" fn simlin_sim_get_varnames(
    sim: *mut SimlinSim,
    result: *mut *const c_char,
    max: usize,
) -> c_int {
    if sim.is_null() || result.is_null() {
        return engine::ErrorCode::Generic as c_int;
    }
    let sim = &mut *sim;
    if let Some(ref results) = sim.results {
        let count = std::cmp::min(results.offsets.len(), max);
        for (i, name) in results.offsets.keys().take(count).enumerate() {
            let c_string = CString::new(name.as_str()).unwrap();
            *result.add(i) = c_string.into_raw() as *const c_char;
        }
        return 0; // Return 0 for success
    }
    if let Some(ref vm) = sim.vm {
        let names = vm.names_as_strs();
        let count = std::cmp::min(names.len(), max);
        for (i, name) in names.iter().take(count).enumerate() {
            let c_string = CString::new(name.as_str()).unwrap();
            *result.add(i) = c_string.into_raw() as *const c_char;
        }
        return 0; // Return 0 for success
    }
    engine::ErrorCode::Generic as c_int
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
    // Re-create the VM - use LTM project if available
    let proj_to_use = if let Some(ref ltm_proj) = (*sim.project).ltm_project {
        ltm_proj
    } else {
        &(*sim.project).project
    };
    let compiler = engine::Simulation::new(proj_to_use, &sim.model_name);
    match compiler {
        Ok(compiler) => match compiler.compile() {
            Ok(compiled) => match Vm::new(compiled) {
                Ok(vm) => {
                    sim.vm = Some(vm);
                    engine::ErrorCode::NoError as c_int
                }
                Err(e) => e.code as c_int,
            },
            Err(e) => e.code as c_int,
        },
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
    // Allow setting only when results exist; mutate the last saved value.
    if let Some(ref mut vm) = sim.vm {
        // Set current value in VM current timestep
        if let Some(off) = vm.get_offset(&canon_name) {
            vm.set_value_now(off, val);
            return engine::ErrorCode::NoError as c_int;
        }
        return engine::ErrorCode::Generic as c_int;
    } else if let Some(ref mut results) = sim.results {
        // Prefer exact canonical match; fall back to suffix match
        let found_off = results.offsets.get(&canon_name).copied();
        if let Some(off) = found_off {
            if results.step_count == 0 {
                return engine::ErrorCode::Generic as c_int;
            }
            let idx = (results.step_count - 1) * results.step_size + off;
            if let Some(slot) = results.data.get_mut(idx) {
                *slot = val;
                return engine::ErrorCode::NoError as c_int;
            }
        }
        return engine::ErrorCode::Generic as c_int;
    }
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
/// - `s` must be a valid pointer returned by simlin_sim_get_varnames
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
                ltm_project: None,
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
                ltm_project: None,
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
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_error_str() {
        unsafe {
            let err_str = simlin_error_str(0);
            assert!(!err_str.is_null());
            let s = CStr::from_ptr(err_str);
            assert_eq!(s.to_str().unwrap(), "no_error");
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

            // Create sim
            let sim = simlin_sim_new(proj, std::ptr::null());
            assert!(!sim.is_null());

            // Run to a partial time
            let rc = simlin_sim_run_to(sim, 0.125);
            assert_eq!(rc, engine::ErrorCode::NoError as c_int);

            // Fetch var names (from VM when no results yet)
            let count = simlin_sim_get_varcount(sim);
            assert!(count > 0, "expected varcount > 0");
            let mut name_ptrs: Vec<*const c_char> = vec![std::ptr::null(); count as usize];
            let err = simlin_sim_get_varnames(sim, name_ptrs.as_mut_ptr(), name_ptrs.len());
            assert_eq!(0, err);

            // Find canonical name that ends with "infectious"
            let mut infectious_name: Option<String> = None;
            for &p in &name_ptrs {
                if p.is_null() {
                    continue;
                }
                let s = std::ffi::CStr::from_ptr(p).to_string_lossy().into_owned();
                // free the leaked CString from get_varnames
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

            // Verify we can create a simulation from the imported project
            let sim = simlin_sim_new(proj, std::ptr::null());
            assert!(!sim.is_null());

            // Run simulation to verify it's valid
            let rc = simlin_sim_run_to_end(sim);
            assert_eq!(rc, engine::ErrorCode::NoError as c_int);

            // Check we have expected variables
            let var_count = simlin_sim_get_varcount(sim);
            assert!(var_count > 0);

            // Clean up
            simlin_sim_unref(sim);
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

            // Verify we can create a simulation from the imported project
            let sim = simlin_sim_new(proj, std::ptr::null());
            assert!(!sim.is_null());

            // Run simulation to verify it's valid
            let rc = simlin_sim_run_to_end(sim);
            assert_eq!(rc, engine::ErrorCode::NoError as c_int);

            // Check we have expected variables
            let var_count = simlin_sim_get_varcount(sim);
            assert!(var_count > 0);

            // Clean up
            simlin_sim_unref(sim);
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

            // Verify both projects can simulate
            let sim1 = simlin_sim_new(proj1, std::ptr::null());
            let sim2 = simlin_sim_new(proj2, std::ptr::null());
            assert!(!sim1.is_null());
            assert!(!sim2.is_null());

            let rc1 = simlin_sim_run_to_end(sim1);
            let rc2 = simlin_sim_run_to_end(sim2);
            assert_eq!(rc1, engine::ErrorCode::NoError as c_int);
            assert_eq!(rc2, engine::ErrorCode::NoError as c_int);

            // Clean up
            simlin_sim_unref(sim1);
            simlin_sim_unref(sim2);
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
                        },
                    )),
                }],
                views: vec![],
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
            let sim = simlin_sim_new(proj, ptr::null());
            assert!(!sim.is_null());
            // Project ref count should have increased
            assert_eq!((*proj).ref_count.load(Ordering::SeqCst), 2);
            // Test reference counting
            simlin_sim_ref(sim);
            assert_eq!((*sim).ref_count.load(Ordering::SeqCst), 2);
            simlin_sim_unref(sim);
            assert_eq!((*sim).ref_count.load(Ordering::SeqCst), 1);
            simlin_sim_unref(sim);
            // Sim should be freed now, project ref count should decrease
            assert_eq!((*proj).ref_count.load(Ordering::SeqCst), 1);
            simlin_project_unref(proj);
        }
    }
}
