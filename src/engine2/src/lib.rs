// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_double, c_int};
use std::ptr;
use std::sync::atomic::{AtomicUsize, Ordering};

use prost::Message;
use simlin_engine::ltm::{detect_loops, LoopPolarity};
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

/// Loop polarity for C API
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SimlinLoopPolarity {
    Reinforcing = 0,
    Balancing = 1,
}

/// A single feedback loop
#[repr(C)]
pub struct SimlinLoop {
    pub id: *mut c_char,
    pub variables: *mut *mut c_char,
    pub var_count: usize,
    pub polarity: SimlinLoopPolarity,
}

/// List of loops returned by analysis
#[repr(C)]
pub struct SimlinLoops {
    pub loops: *mut SimlinLoop,
    pub count: usize,
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
        ltm_project: None,
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

/// Enables LTM (Loops That Matter) analysis on a project
///
/// # Safety
/// - `project` must be a valid pointer to a SimlinProject
#[no_mangle]
pub unsafe extern "C" fn simlin_project_enable_ltm(project: *mut SimlinProject) -> c_int {
    if project.is_null() {
        return SimlinError::Unspecified as c_int;
    }

    let proj = &mut *project;

    // If LTM is already enabled, return success
    if proj.ltm_project.is_some() {
        return SimlinError::NoError as c_int;
    }

    // Create LTM-augmented project
    match proj.project.clone().with_ltm() {
        Ok(ltm_proj) => {
            proj.ltm_project = Some(ltm_proj);
            SimlinError::NoError as c_int
        }
        Err(_) => SimlinError::Unspecified as c_int,
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

    // Initialize the VM - use LTM project if available
    let proj_to_use = if let Some(ref ltm_proj) = (*project).ltm_project {
        ltm_proj
    } else {
        &(*project).project
    };

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
            count as c_int
        } else {
            // Variable not found - project might not have LTM enabled
            -1
        }
    } else {
        -1
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
