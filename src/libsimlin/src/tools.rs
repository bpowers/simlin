// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! The agent tool surface (`simlin_engine::tools`) for native hosts: the tool
//! catalog, and a session per model that answers tool calls with JSON.
//!
//! A host bridges each catalog entry into its agent framework and forwards the
//! agent's calls to `simlin_tool_session_call`. A call's domain failures (an
//! unknown variable, input the tool's schema does not allow) come back as
//! output with `out_is_error` set, for the agent to read and repair; only the
//! host's own misuse (a NULL pointer, a tool the catalog does not list) is a
//! `SimlinError`.

use std::ffi::CStr;
use std::os::raw::c_char;
use std::ptr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use simlin_engine::tools;

use crate::ffi_error::SimlinError;
use crate::{
    clear_out_error, require_model, store_anyhow_error, store_error, write_bytes_to_ffi_output,
    SimlinErrorCode, SimlinModel,
};

#[cfg(test)]
type ToolTestHook = std::sync::Arc<dyn Fn(&crate::SimlinProject) + Send + Sync + 'static>;

#[cfg(test)]
static TOOL_TEST_HOOK: std::sync::Mutex<Option<ToolTestHook>> = std::sync::Mutex::new(None);

/// Held by each installed hook's guard, so tests that install one run one at
/// a time: there is one hook for every call.
#[cfg(test)]
static TOOL_TEST_HOOK_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Runs `hook` inside every tool call made while the guard lives, at the point
/// a call holds the database and has released the datamodel: what a test of
/// the call's locks waits at.
#[cfg(test)]
pub(crate) fn install_tool_test_hook(hook: ToolTestHook) -> ToolTestHookGuard {
    let lock = TOOL_TEST_HOOK_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    *TOOL_TEST_HOOK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(hook);
    ToolTestHookGuard { _lock: lock }
}

#[cfg(test)]
pub(crate) struct ToolTestHookGuard {
    _lock: std::sync::MutexGuard<'static, ()>,
}

#[cfg(test)]
impl Drop for ToolTestHookGuard {
    fn drop(&mut self) {
        *TOOL_TEST_HOOK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
    }
}

#[cfg(test)]
fn invoke_tool_test_hook(project: &crate::SimlinProject) {
    let hook = TOOL_TEST_HOOK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    if let Some(hook) = hook {
        hook(project);
    }
}

/// One agent's work on one model: the evidence ids it has been given and what
/// it last read (`simlin_engine::tools::Session`), over the model it was made
/// for.
pub struct SimlinToolSession {
    /// Counted: the session keeps its model, and through it the project, alive.
    model: *const SimlinModel,
    session: Mutex<tools::Session>,
    ref_count: AtomicUsize,
}

/// Write the tool catalog -- every tool's name, description, effect, and the
/// JSON Schema of its input and of its output -- as UTF-8 JSON to a buffer the
/// caller frees with `simlin_free`: `{"tools": [{"name", "description",
/// "effect", "inputSchema", "outputSchema"}, ...]}`.
///
/// # Safety
/// - `out_buf` and `out_len` must be valid pointers
#[no_mangle]
pub unsafe extern "C" fn simlin_tools_describe(
    out_buf: *mut *mut u8,
    out_len: *mut usize,
    out_error: *mut *mut SimlinError,
) {
    clear_out_error(out_error);
    if out_buf.is_null() || out_len.is_null() {
        store_error(
            out_error,
            SimlinError::new(SimlinErrorCode::Generic)
                .with_message("output pointers must not be NULL"),
        );
        return;
    }
    write_bytes_to_ffi_output(
        tools::catalog_json().as_bytes(),
        out_buf,
        out_len,
        out_error,
        "the tool catalog",
    );
}

/// Make a tool session over `model`, which it keeps alive until the session's
/// last reference is dropped with `simlin_tool_session_unref`.
///
/// # Safety
/// - `model` must be a valid pointer to a SimlinModel
#[no_mangle]
pub unsafe extern "C" fn simlin_tool_session_new(
    model: *mut SimlinModel,
    out_error: *mut *mut SimlinError,
) -> *mut SimlinToolSession {
    clear_out_error(out_error);
    let model_ref = match require_model(model) {
        Ok(m) => m,
        Err(err) => {
            store_anyhow_error(out_error, err);
            return ptr::null_mut();
        }
    };
    crate::model_ref(model);
    Box::into_raw(Box::new(SimlinToolSession {
        model: model_ref as *const SimlinModel,
        session: Mutex::new(tools::Session::new(&model_ref.model_name)),
        ref_count: AtomicUsize::new(1),
    }))
}

/// Increment a tool session's reference count.
///
/// # Safety
/// - `session` must be a valid pointer to a SimlinToolSession, or NULL
#[no_mangle]
pub unsafe extern "C" fn simlin_tool_session_ref(session: *mut SimlinToolSession) {
    if !session.is_null() {
        (*session).ref_count.fetch_add(1, Ordering::SeqCst);
    }
}

/// Decrement a tool session's reference count, releasing it (and its reference
/// to its model) at zero.
///
/// # Safety
/// - `session` must be a valid pointer to a SimlinToolSession, or NULL
#[no_mangle]
pub unsafe extern "C" fn simlin_tool_session_unref(session: *mut SimlinToolSession) {
    if session.is_null() {
        return;
    }
    if (*session).ref_count.fetch_sub(1, Ordering::SeqCst) == 1 {
        std::sync::atomic::fence(Ordering::SeqCst);
        let session = Box::from_raw(session);
        crate::model_unref(session.model as *mut SimlinModel);
    }
}

unsafe fn require_session<'a>(
    session: *mut SimlinToolSession,
) -> Result<&'a SimlinToolSession, SimlinError> {
    if session.is_null() {
        Err(SimlinError::new(SimlinErrorCode::Generic)
            .with_message("tool session pointer must not be NULL"))
    } else {
        Ok(&*session)
    }
}

/// Answer a call of the tool named `name` with `input` (`input_len` bytes of
/// UTF-8 JSON; empty means `{}`), writing the output JSON to a buffer the
/// caller frees with `simlin_free` and whether it is a refusal to
/// `out_is_error`. A refusal names the rule the call broke and the repair,
/// for the agent to read: a host hands it back as the tool's result, never as
/// an exception that ends the agent's turn.
///
/// A call reads the project as it is when the call starts, at that revision.
/// It holds the session for the call, and the project's datamodel only while
/// it takes the contents it answers from (shared, not copied) and their
/// revision: it then answers under the database lock alone, so a host's hit
/// tests, planners and revision reads, which lock only the datamodel, never
/// wait behind an analysis. An entry point that holds the datamodel and waits
/// for the database meanwhile -- an edit landing (`simlin_project_apply_patch`,
/// or an undo's `simlin_project_replace_contents`), a simulation
/// (`simlin_sim_new`), a read of the diagnostics, the others
/// `SimlinProject::waiting_for_db` lists -- keeps those readers waiting with
/// it, so the call stops for it between units of its work (a simulation, a
/// stage of an analysis) and answers a refusal with `"interrupted": true` that
/// kept nothing: the entry point waits at most one unit. A host that retries
/// by itself does so once that work is done -- after an edit, at the next
/// revision -- and never in a loop against a project that stays busy.
///
/// # Safety
/// - `session` must be a valid pointer to a SimlinToolSession
/// - `name` must be a valid C string
/// - `input` must point to `input_len` bytes, or be NULL when it is zero
/// - `out_buf`, `out_len` and `out_is_error` must be valid pointers
#[no_mangle]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn simlin_tool_session_call(
    session: *mut SimlinToolSession,
    name: *const c_char,
    input: *const u8,
    input_len: usize,
    out_buf: *mut *mut u8,
    out_len: *mut usize,
    out_is_error: *mut bool,
    out_error: *mut *mut SimlinError,
) {
    clear_out_error(out_error);
    if out_buf.is_null() || out_len.is_null() || out_is_error.is_null() {
        store_error(
            out_error,
            SimlinError::new(SimlinErrorCode::Generic)
                .with_message("output pointers must not be NULL"),
        );
        return;
    }
    let session_ref = match require_session(session) {
        Ok(s) => s,
        Err(err) => {
            store_error(out_error, err);
            return;
        }
    };
    if name.is_null() {
        store_error(
            out_error,
            SimlinError::new(SimlinErrorCode::Generic)
                .with_message("tool name pointer must not be NULL"),
        );
        return;
    }
    let Ok(name) = CStr::from_ptr(name).to_str() else {
        store_error(
            out_error,
            SimlinError::new(SimlinErrorCode::Generic).with_message("tool name is not valid UTF-8"),
        );
        return;
    };
    if input.is_null() && input_len > 0 {
        store_error(
            out_error,
            SimlinError::new(SimlinErrorCode::Generic)
                .with_message("input pointer must not be NULL when input_len > 0"),
        );
        return;
    }
    let input_bytes = if input_len == 0 {
        &[][..]
    } else {
        std::slice::from_raw_parts(input, input_len)
    };
    let Ok(input) = std::str::from_utf8(input_bytes) else {
        store_error(
            out_error,
            SimlinError::new(SimlinErrorCode::Generic).with_message("input is not valid UTF-8"),
        );
        return;
    };

    let project = &*(*session_ref.model).project;
    let mut tool_session = session_ref.session.lock().unwrap();
    let (contents, revision, mut db) = snapshot(project);
    #[cfg(test)]
    invoke_tool_test_hook(project);
    let waiting = || project.is_waited_on();
    let workspace = tools::Workspace {
        project: &contents,
        db: &mut db,
        revision,
        waiting: Some(&waiting),
    };
    let output = match tool_session.call(workspace, name, input) {
        Ok(output) => output,
        Err(unknown) => {
            store_error(
                out_error,
                SimlinError::new(SimlinErrorCode::DoesNotExist).with_message(unknown.to_string()),
            );
            return;
        }
    };
    drop(db);
    drop(tool_session);
    if write_bytes_to_ffi_output(
        output.json.as_bytes(),
        out_buf,
        out_len,
        out_error,
        "a tool's output",
    ) {
        *out_is_error = output.is_error;
    }
}

/// The project's contents and revision, taken under the datamodel lock, and
/// its database, locked, with the datamodel released: what a tool entry point
/// answers from. The contents are the project's own, shared, so taking them
/// copies nothing; an edit that lands meanwhile copies them first, as any edit
/// of shared contents does. The database stays synced to them while it is
/// held, since a sync needs its lock. The project-wide order, datamodel then
/// database, is kept, and the first call on a project builds its database
/// from the contents it locked. When another holder has the database --
/// another session's call, or a host's query that holds only the database --
/// the call waits for it with the datamodel released, so the datamodel's
/// readers answer meanwhile and an edit that comes waits for the database
/// counted, which a call holding it stops for; then it takes the contents
/// again.
fn snapshot(
    project: &crate::SimlinProject,
) -> (
    std::sync::Arc<simlin_engine::datamodel::Project>,
    u64,
    crate::DbLock<'_>,
) {
    loop {
        let datamodel = project.datamodel.lock().unwrap();
        if let Some(db) = project.try_lock_db_for_call(&datamodel) {
            return (datamodel.shared(), datamodel.revision(), db);
        }
        drop(datamodel);
        project.wait_for_db();
    }
}

/// Write what changed in the session's model since the session's last
/// `read_model` -- variables added, removed and changed (with which fields),
/// and whether the sim specs changed -- as UTF-8 JSON to a buffer the caller
/// frees with `simlin_free`, or `null` before the first read and when nothing
/// did. A view edit changes nothing an agent read, so it is no change here.
/// What a host tells an agent about the person's work before its next turn.
///
/// # Safety
/// - `session` must be a valid pointer to a SimlinToolSession
/// - `out_buf` and `out_len` must be valid pointers
#[no_mangle]
pub unsafe extern "C" fn simlin_tool_session_get_changes(
    session: *mut SimlinToolSession,
    out_buf: *mut *mut u8,
    out_len: *mut usize,
    out_error: *mut *mut SimlinError,
) {
    clear_out_error(out_error);
    if out_buf.is_null() || out_len.is_null() {
        store_error(
            out_error,
            SimlinError::new(SimlinErrorCode::Generic)
                .with_message("output pointers must not be NULL"),
        );
        return;
    }
    let session_ref = match require_session(session) {
        Ok(s) => s,
        Err(err) => {
            store_error(out_error, err);
            return;
        }
    };
    let project = &*(*session_ref.model).project;
    let tool_session = session_ref.session.lock().unwrap();
    let datamodel = project.datamodel.lock().unwrap();
    let changes = tool_session.changes_since_read(&datamodel, datamodel.revision());
    drop(datamodel);
    drop(tool_session);
    let json = serde_json::to_vec(&changes).expect("a change report serializes");
    write_bytes_to_ffi_output(&json, out_buf, out_len, out_error, "a change report");
}
