// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_double, c_int};
use std::ptr;
use std::sync::atomic::{AtomicUsize, Ordering};

use prost::Message;
use simlin_engine::{self as engine, canonicalize, serde, Vm};

/// Error codes matching the C API specification
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SimlinError {
    NoError = 0,
    NoMem = -1,
    BadFile = -2,
    Unspecified = -3,
    BadXml = -4,
    BadLex = -5,
    Eof = -6,
    Circular = -7,
}

impl From<engine::Error> for SimlinError {
    fn from(err: engine::Error) -> Self {
        use engine::ErrorCode;
        match err.code {
            ErrorCode::XmlDeserialization => SimlinError::BadXml,
            ErrorCode::InvalidToken | ErrorCode::UnrecognizedToken | ErrorCode::ExtraToken => {
                SimlinError::BadLex
            }
            ErrorCode::UnrecognizedEof => SimlinError::Eof,
            ErrorCode::CircularDependency => SimlinError::Circular,
            _ => SimlinError::Unspecified,
        }
    }
}

/// Opaque project structure
pub struct SimlinProject {
    project: engine::Project,
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

/// Returns a string representation of an error code
#[no_mangle]
pub extern "C" fn simlin_error_str(err: c_int) -> *const c_char {
    let err_str = match err {
        0 => "no error\0",
        -1 => "out of memory\0",
        -2 => "bad file\0",
        -3 => "unspecified error\0",
        -4 => "bad XML\0",
        -5 => "lexer error\0",
        -6 => "unexpected end of file\0",
        -7 => "circular dependency\0",
        _ => "unknown error\0",
    };

    // These strings are static and safe to return as const pointers
    err_str.as_ptr() as *const c_char
}

/// Opens a project from protobuf data
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
            *err = SimlinError::Unspecified as c_int;
        }
        return ptr::null_mut();
    }

    let slice = std::slice::from_raw_parts(data, len);

    let project = match engine::project_io::Project::decode(slice) {
        Ok(pb_project) => serde::deserialize(pb_project),
        Err(_) => {
            if !err.is_null() {
                *err = SimlinError::BadFile as c_int;
            }
            return ptr::null_mut();
        }
    };

    let boxed = Box::new(SimlinProject {
        project: project.into(),
        ref_count: AtomicUsize::new(1),
    });

    if !err.is_null() {
        *err = SimlinError::NoError as c_int;
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
        (*project).ref_count.fetch_add(1, Ordering::Relaxed);
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

    let prev_count = (*project).ref_count.fetch_sub(1, Ordering::Release);
    if prev_count == 1 {
        std::sync::atomic::fence(Ordering::Acquire);
        let _ = Box::from_raw(project);
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

    let model_name = if model_name.is_null() {
        "main".to_string()
    } else {
        match CStr::from_ptr(model_name).to_str() {
            Ok(s) => s.to_string(),
            Err(_) => return ptr::null_mut(),
        }
    };

    // Increment project reference count
    (*project).ref_count.fetch_add(1, Ordering::Relaxed);

    let mut sim = Box::new(SimlinSim {
        project,
        model_name: model_name.clone(),
        vm: None,
        results: None,
        ref_count: AtomicUsize::new(1),
    });

    // Initialize the VM
    let compiler = engine::Simulation::new(&(*project).project, &model_name);
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
        (*sim).ref_count.fetch_add(1, Ordering::Relaxed);
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

    let prev_count = (*sim).ref_count.fetch_sub(1, Ordering::Release);
    if prev_count == 1 {
        std::sync::atomic::fence(Ordering::Acquire);
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
        return SimlinError::Unspecified as c_int;
    }

    let sim = &mut *sim;

    if let Some(ref mut vm) = sim.vm {
        match vm.run_to(time) {
            Ok(_) => SimlinError::NoError as c_int,
            Err(e) => SimlinError::from(e) as c_int,
        }
    } else {
        SimlinError::Unspecified as c_int
    }
}

/// Runs the simulation to completion
///
/// # Safety
/// - `sim` must be a valid pointer to a SimlinSim
#[no_mangle]
pub unsafe extern "C" fn simlin_sim_run_to_end(sim: *mut SimlinSim) -> c_int {
    if sim.is_null() {
        return SimlinError::Unspecified as c_int;
    }

    let sim = &mut *sim;

    if let Some(mut vm) = sim.vm.take() {
        match vm.run_to_end() {
            Ok(_) => {
                sim.results = Some(vm.into_results());
                SimlinError::NoError as c_int
            }
            Err(e) => {
                sim.vm = Some(vm);
                SimlinError::from(e) as c_int
            }
        }
    } else if sim.results.is_some() {
        // Already ran to completion
        SimlinError::NoError as c_int
    } else {
        SimlinError::Unspecified as c_int
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
        results.offsets.len() as c_int
    } else if let Some(ref _vm) = sim.vm {
        // TODO: Need to get variable count from VM or project
        // For now, return -1 to indicate not available
        -1
    } else {
        -1
    }
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
        return -1;
    }

    let sim = &mut *sim;

    if let Some(ref results) = sim.results {
        let count = std::cmp::min(results.offsets.len(), max);

        // We need to store CStrings somewhere that will outlive this function
        // This is a design challenge - we need a way to manage string lifetimes
        // For now, we'll leak the memory (not ideal, but safe)
        for (i, name) in results.offsets.keys().take(count).enumerate() {
            let c_string = CString::new(name.as_str()).unwrap();
            *result.add(i) = c_string.into_raw() as *const c_char;
        }

        count as c_int
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
        return SimlinError::Unspecified as c_int;
    }

    let sim = &mut *sim;

    // Clear results
    sim.results = None;

    // Re-create the VM
    let project = &(*sim.project).project;
    let compiler = engine::Simulation::new(project, &sim.model_name);

    match compiler {
        Ok(compiler) => match compiler.compile() {
            Ok(compiled) => match Vm::new(compiled) {
                Ok(vm) => {
                    sim.vm = Some(vm);
                    SimlinError::NoError as c_int
                }
                Err(e) => SimlinError::from(e) as c_int,
            },
            Err(e) => SimlinError::from(e) as c_int,
        },
        Err(e) => SimlinError::from(e) as c_int,
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
        return SimlinError::Unspecified as c_int;
    }

    let sim = &*sim;

    let name = match CStr::from_ptr(name).to_str() {
        Ok(s) => canonicalize(s),
        Err(_) => return SimlinError::Unspecified as c_int,
    };

    if let Some(ref _vm) = sim.vm {
        // Get current value from VM
        // TODO: Need to implement value getter in VM
        SimlinError::Unspecified as c_int
    } else if let Some(ref results) = sim.results {
        // Get final value from results
        if let Some(&offset) = results.offsets.get(&name) {
            if let Some(last_row) = results.iter().next_back() {
                *result = last_row[offset];
                SimlinError::NoError as c_int
            } else {
                SimlinError::Unspecified as c_int
            }
        } else {
            SimlinError::Unspecified as c_int
        }
    } else {
        SimlinError::Unspecified as c_int
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
    _val: c_double,
) -> c_int {
    if sim.is_null() || name.is_null() {
        return SimlinError::Unspecified as c_int;
    }

    let sim = &mut *sim;

    let _name = match CStr::from_ptr(name).to_str() {
        Ok(s) => canonicalize(s),
        Err(_) => return SimlinError::Unspecified as c_int,
    };

    if let Some(ref mut _vm) = sim.vm {
        // TODO: Need to implement value setter in VM
        SimlinError::Unspecified as c_int
    } else {
        SimlinError::Unspecified as c_int
    }
}

/// Gets a time series for a variable
///
/// # Safety
/// - `sim` must be a valid pointer to a SimlinSim
/// - `name` must be a valid C string
/// - `results` must be a valid pointer to an array of at least `len` doubles
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
            count as c_int
        } else {
            -1
        }
    } else {
        -1
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_str() {
        unsafe {
            let err_str = simlin_error_str(0);
            assert!(!err_str.is_null());
            let s = CStr::from_ptr(err_str);
            assert_eq!(s.to_str().unwrap(), "no error");

            let err_str = simlin_error_str(-1);
            assert!(!err_str.is_null());
            let s = CStr::from_ptr(err_str);
            assert_eq!(s.to_str().unwrap(), "out of memory");
        }
    }

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
            assert_eq!(err, 0);

            // Test reference counting
            simlin_project_ref(proj);
            assert_eq!((*proj).ref_count.load(Ordering::Relaxed), 2);

            simlin_project_unref(proj);
            assert_eq!((*proj).ref_count.load(Ordering::Relaxed), 1);

            simlin_project_unref(proj);
            // Project should be freed now
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
            assert_eq!(err, 0);

            let sim = simlin_sim_new(proj, ptr::null());
            assert!(!sim.is_null());

            // Project ref count should have increased
            assert_eq!((*proj).ref_count.load(Ordering::Relaxed), 2);

            // Test reference counting
            simlin_sim_ref(sim);
            assert_eq!((*sim).ref_count.load(Ordering::Relaxed), 2);

            simlin_sim_unref(sim);
            assert_eq!((*sim).ref_count.load(Ordering::Relaxed), 1);

            simlin_sim_unref(sim);
            // Sim should be freed now, project ref count should decrease
            assert_eq!((*proj).ref_count.load(Ordering::Relaxed), 1);

            simlin_project_unref(proj);
        }
    }
}
