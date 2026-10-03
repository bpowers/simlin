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
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use simlin_engine::tools;

use crate::ffi_error::SimlinError;
use crate::lock_order::{OrderedMutex, Rank};
use crate::{
    clear_out_error, require_model, store_anyhow_error, store_error, write_bytes_to_ffi_output,
    SimlinErrorCode, SimlinModel, SimlinResults,
};

/// One agent's work on one model: the evidence ids it has been given, what it
/// last read, and the runs it made (`simlin_engine::tools::Session`), over the
/// model it was made for.
pub struct SimlinToolSession {
    /// Counted: the session keeps its model, and through it the project, alive.
    model: *const SimlinModel,
    session: OrderedMutex<tools::Session>,
    ref_count: AtomicUsize,
    /// How many times the session's calls have been cancelled
    /// (`simlin_tool_session_cancel`). A call reads it as it begins, before it
    /// waits for the session, and is cancelled once it reads anything else:
    /// a cancel covers exactly the calls that began before it, however many
    /// cancels race.
    cancellations: AtomicU64,
    /// How many calls have begun on the session, the ones waiting for it
    /// included: what a test waits on before it cancels them.
    #[cfg(test)]
    calls_begun: AtomicU64,
}

#[cfg(test)]
impl SimlinToolSession {
    /// How many calls have begun on the session, the one waiting for it
    /// included.
    pub(crate) fn calls_begun(&self) -> u64 {
        self.calls_begun.load(Ordering::SeqCst)
    }
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
        session: OrderedMutex::new(Rank::Session, tools::Session::new(&model_ref.model_name)),
        ref_count: AtomicUsize::new(1),
        cancellations: AtomicU64::new(0),
        #[cfg(test)]
        calls_begun: AtomicU64::new(0),
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
/// One entry point, two lock footprints, chosen by the tool's effect
/// (`simlin_engine::tools::ToolName::effect`) before any lock is taken.
///
/// A tool that reads answers the project as it is when the call starts, at
/// that revision. It holds the session for the call, and the project's
/// datamodel only while it takes the contents it answers from (shared, not
/// copied) and their revision: it then answers under the database lock alone,
/// so a host's hit tests, planners and revision reads, which lock only the
/// datamodel, never wait behind an analysis. Any other entry point that waits
/// for the database meanwhile is the person's work, or an edit: an edit
/// landing (`simlin_project_apply_patch`, an undo's
/// `simlin_project_replace_contents`, a call of a tool that edits), a
/// simulation (`simlin_sim_new`), a read of the diagnostics and the others
/// `SimlinProject::waiting_for_db` lists, which hold the datamodel while they
/// wait and so keep those readers waiting with them, and a query that takes
/// the database alone (an equation's rendering, the model's links). So the
/// call stops for it between units of its work (a slice of a simulation, a
/// stage of an analysis) and answers a refusal with `"interrupted": true`
/// that kept nothing: the entry point waits at most one unit. A host that
/// retries by itself does so once that work is done -- after an edit, at the
/// next revision -- and never in a loop against a project that stays busy.
///
/// A tool that edits (`edit_model`) is a writer, as
/// `simlin_project_apply_patch` is: it holds the session, the datamodel and
/// the database for the whole call, so nothing lands between its gate and its
/// edit, and when the gate passes it makes the edit before it returns
/// (`simlin_engine::tools::ToolOutput::edited`), the database synced to the
/// edited project and the contents replaced by it under both locks, which
/// advances the revision. An edit its gate refuses is a refusal, with
/// `out_is_error` set; it and an edit that changes nothing leave the project
/// and its revision as they were. It stops for no other work (it is the work
/// others wait for), and counts itself among the waiters a
/// reading call stops for from before it waits for the session: a call on the
/// same session holds the session for the whole call, and one on another the
/// database, so either stops at its next checkpoint rather than keep the edit
/// waiting for the rest of it.
///
/// A call its host cancels (`simlin_tool_session_cancel`) stops at its
/// checkpoints and answers a refusal with `"cancelled": true`, which no host
/// retries.
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
    // The host's mistake, told before any lock is taken: the tool's effect
    // is what says which locks the call takes.
    let unknown_tool = |unknown: tools::UnknownTool| {
        store_error(
            out_error,
            SimlinError::new(SimlinErrorCode::DoesNotExist).with_message(unknown.to_string()),
        );
    };
    let Some(tool) = tools::ToolName::from_name(name) else {
        unknown_tool(tools::UnknownTool(name.to_string()));
        return;
    };

    // Read before the call waits for the session, so a cancel made while it
    // waits covers it.
    let cancellations = &session_ref.cancellations;
    let began_at = cancellations.load(Ordering::SeqCst);
    #[cfg(test)]
    session_ref.calls_begun.fetch_add(1, Ordering::SeqCst);
    let cancelled = || cancellations.load(Ordering::SeqCst) != began_at;
    let project = &*(*session_ref.model).project;
    let answered = match tool.effect() {
        tools::ToolEffect::Read => answer_read(session_ref, project, name, input, &cancelled),
        tools::ToolEffect::Edit => answer_edit(session_ref, project, name, input, &cancelled),
    };
    let output = match answered {
        Ok(output) => output,
        Err(unknown) => {
            unknown_tool(unknown);
            return;
        }
    };
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

/// Answer a call of a tool that reads: the session held for the call, and
/// the project's contents and database taken as a reader takes them
/// ([`snapshot`]), so the call answers under the database alone and stops for
/// work that waits for it.
fn answer_read(
    session: &SimlinToolSession,
    project: &crate::SimlinProject,
    name: &str,
    input: &str,
    cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<tools::ToolOutput, tools::UnknownTool> {
    let mut tool_session = session.session.lock().unwrap();
    // A call cancelled while it waited for the session answers now, before it
    // waits for the database, which another session's call may hold.
    if cancelled() {
        return Ok(tools::ToolOutput::cancelled());
    }
    let (contents, revision, mut db) = snapshot(project);
    #[cfg(test)]
    crate::invoke_db_section_test_hook(project);
    let waiting = || project.is_waited_on();
    let workspace = tools::Workspace {
        project: &contents,
        db: &mut db,
        revision,
        waiting: Some(&waiting),
        cancelled: Some(cancelled),
    };
    let output = tool_session.call(workspace, name, input)?;
    // Only a tool that edits hands back a project, and this path makes no
    // edit: one handed back here would be dropped.
    debug_assert!(
        output.edited.is_none(),
        "{name} reads, and handed back an edited project"
    );
    Ok(output)
}

/// Answer a call of a tool that edits, and make its edit: the session, the
/// datamodel and the database held for the whole call, in that order, as an
/// edit landing holds them, so the gate judges the very contents the edit
/// then replaces.
fn answer_edit(
    session: &SimlinToolSession,
    project: &crate::SimlinProject,
    name: &str,
    input: &str,
    cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<tools::ToolOutput, tools::UnknownTool> {
    // Counted until it holds the database, which no reading call then does.
    let waiting = project.count_waiting();
    let mut tool_session = session.session.lock().unwrap();
    // A call cancelled while it waited for the session answers now, before it
    // waits for the project, whose database another session's call may hold.
    if cancelled() {
        return Ok(tools::ToolOutput::cancelled());
    }
    let mut datamodel = project.datamodel.lock().unwrap();
    let mut db = project.lock_db_with(&datamodel);
    drop(waiting);
    let revision = datamodel.revision();
    // It is the work other calls stop for, so it stops for none.
    let workspace = tools::Workspace {
        project: &datamodel,
        db: &mut db,
        revision,
        waiting: None,
        cancelled: Some(cancelled),
    };
    let mut output = tool_session.call(workspace, name, input)?;
    // The gate staged on the database and restored it, so it is synced to
    // the contents the call was given. The edit: the database synced to the
    // project the call's edit leaves, then the contents replaced by it, which
    // advances the revision.
    if let Some(edited) = output.edited.take() {
        db.sync(&edited);
        datamodel.replace(std::sync::Arc::new(edited));
    }
    Ok(output)
}

/// Cancel the session's tool calls under way -- the one answering and any
/// waiting for the session -- as a host does when what they were for is gone,
/// such as the window whose analysis a call runs: each stops at its next
/// checkpoint, between units of its work, or, still waiting for the session,
/// as it gets it, before it waits for the database, and answers a refusal
/// with `"cancelled": true` that kept nothing, which a host does not retry. A
/// call made after this returns runs as usual. It returns at once, without
/// waiting for the calls to stop, and takes no lock, so any thread may make
/// it, one inside a call included. It cancels only `simlin_tool_session_call`,
/// a call of a tool that edits included, which is cancelled before its edit
/// is made or not at all: a host's own reads of the session's runs go on. A
/// NULL `session` is a no-op.
///
/// # Safety
/// - `session` must be a valid pointer to a SimlinToolSession, or NULL
#[no_mangle]
pub unsafe extern "C" fn simlin_tool_session_cancel(session: *mut SimlinToolSession) {
    if let Some(session) = session.as_ref() {
        session.cancellations.fetch_add(1, Ordering::SeqCst);
    }
}

/// Forget the session's run named `name` -- its series and its plan -- as a
/// host does when the person discards it: no tool reads it again, the listing
/// leaves it out, and a run made from it keeps what it changed. Whether the
/// session had it goes to `out_forgotten`, which may be NULL; a run it never
/// had is no error. `"current"`, the model as it is, is refused with
/// `Generic`. Locks the session only.
///
/// # Safety
/// - `session` must be a valid pointer to a SimlinToolSession
/// - `name` must be a valid C string
/// - `out_forgotten` must be a valid pointer or NULL
#[no_mangle]
pub unsafe extern "C" fn simlin_tool_session_forget_run(
    session: *mut SimlinToolSession,
    name: *const c_char,
    out_forgotten: *mut bool,
    out_error: *mut *mut SimlinError,
) {
    clear_out_error(out_error);
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
                .with_message("run name pointer must not be NULL"),
        );
        return;
    }
    let Ok(name) = CStr::from_ptr(name).to_str() else {
        store_error(
            out_error,
            SimlinError::new(SimlinErrorCode::Generic).with_message("run name is not valid UTF-8"),
        );
        return;
    };
    let forgotten = session_ref.session.lock().unwrap().forget_run(name);
    match forgotten {
        Ok(forgotten) => {
            if !out_forgotten.is_null() {
                *out_forgotten = forgotten;
            }
        }
        Err(reason) => store_error(
            out_error,
            SimlinError::new(SimlinErrorCode::Generic).with_message(reason),
        ),
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
/// did. A view edit changes nothing an agent read, so it is no change here,
/// and what the session's own edits left is the agent's work, not news to it.
/// What a host tells an agent about the person's work before its next turn.
/// A project that no longer has the session's model fails with
/// `DoesNotExist`, as the session's tool calls refuse it.
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
    let changes = match changes {
        Ok(changes) => changes,
        Err(reason) => {
            store_error(
                out_error,
                SimlinError::new(SimlinErrorCode::DoesNotExist).with_message(reason),
            );
            return;
        }
    };
    let json = serde_json::to_vec(&changes).expect("a change report serializes");
    write_bytes_to_ffi_output(&json, out_buf, out_len, out_error, "a change report");
}

/// The results of the session's run named `name` -- `"current"` for the model
/// as it is at the project's current revision, or a run an experiment made --
/// as a standalone results handle the caller releases with
/// `simlin_results_unref`: every saved series, for a host to chart, where the
/// tools answer an agent with summaries. The revision the run was made at goes
/// to `out_revision`, and whether the model has changed since (its diagrams
/// aside) to `out_stale`; either may be NULL. NULL with `DoesNotExist` when
/// the session has no such run, and with the reason when the model does not
/// simulate. A read that must simulate stops, as a tool call does, for an
/// edit or a simulation that waits for the project, and keeps nothing: NULL
/// with `Interrupted`, for a host that reads it again once that work is done.
/// A run the session keeps the results of is read from the contents alone,
/// so it waits for no work on the database, another session's call among it.
/// The handle holds a copy of the run's series.
///
/// # Safety
/// - `session` must be a valid pointer to a SimlinToolSession
/// - `name` must be a valid C string
/// - `out_revision` and `out_stale` must be valid pointers or NULL
#[no_mangle]
pub unsafe extern "C" fn simlin_tool_session_get_run(
    session: *mut SimlinToolSession,
    name: *const c_char,
    out_revision: *mut u64,
    out_stale: *mut bool,
    out_error: *mut *mut SimlinError,
) -> *mut SimlinResults {
    clear_out_error(out_error);
    let session_ref = match require_session(session) {
        Ok(s) => s,
        Err(err) => {
            store_error(out_error, err);
            return ptr::null_mut();
        }
    };
    if name.is_null() {
        store_error(
            out_error,
            SimlinError::new(SimlinErrorCode::Generic)
                .with_message("run name pointer must not be NULL"),
        );
        return ptr::null_mut();
    }
    let Ok(name) = CStr::from_ptr(name).to_str() else {
        store_error(
            out_error,
            SimlinError::new(SimlinErrorCode::Generic).with_message("run name is not valid UTF-8"),
        );
        return ptr::null_mut();
    };

    let project = &*(*session_ref.model).project;
    let mut tool_session = session_ref.session.lock().unwrap();
    // A run the session keeps is read from the contents alone, so the read
    // waits for no one's work on the database (another session's call among
    // it); only a run that must be made takes the database.
    let (contents, revision) = {
        let datamodel = project.datamodel.lock().unwrap();
        (datamodel.shared(), datamodel.revision())
    };
    let results = match tool_session.kept_run_results(&contents, revision, name) {
        Some(kept) => Ok(kept),
        None => {
            drop(contents);
            let (contents, revision, mut db) = snapshot(project);
            let waiting = || project.is_waited_on();
            let workspace = tools::Workspace {
                project: &contents,
                db: &mut db,
                revision,
                waiting: Some(&waiting),
                cancelled: None,
            };
            tool_session.run_results(workspace, name)
        }
    };
    drop(tool_session);
    match results {
        Ok(run) => {
            if !out_revision.is_null() {
                *out_revision = run.revision;
            }
            if !out_stale.is_null() {
                *out_stale = run.stale;
            }
            Box::into_raw(Box::new(SimlinResults {
                results: run.results,
                ref_count: AtomicUsize::new(1),
            }))
        }
        Err(unavailable) => {
            let code = if unavailable.interrupted {
                SimlinErrorCode::Interrupted
            } else {
                SimlinErrorCode::DoesNotExist
            };
            store_error(
                out_error,
                SimlinError::new(code).with_message(unavailable.reason),
            );
            ptr::null_mut()
        }
    }
}

/// Write the session's named runs, oldest first, as UTF-8 JSON to a buffer the
/// caller frees with `simlin_free`: `[{"name", "revision", "stale", "gone",
/// "from", "changes", "specs"}]`, where `stale` says the model has changed
/// since the run (its diagrams aside), `gone` that a stale run's series are
/// no longer kept, `from` names the run it started from, and `changes` and
/// `specs` are everything it changed from the model, exactly as it ran: each
/// `{"variable", "value" | "elements" | "equation", "tableDropped"?,
/// "fromTime"?}`, and the specs it set (`start`, `stop`, `dt`, `method`).
/// The run `"current"`, the model as it is, is always there and is not
/// listed. What a host's run list, chart picker and "run again" read. Locks
/// the session and, to take the contents, the datamodel; never the database.
///
/// # Safety
/// - `session` must be a valid pointer to a SimlinToolSession
/// - `out_buf` and `out_len` must be valid pointers
#[no_mangle]
pub unsafe extern "C" fn simlin_tool_session_list_runs(
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
    let mut tool_session = session_ref.session.lock().unwrap();
    // The listing reads the contents alone, shared and released before it is
    // made: it waits for no one's work on the database, and builds none.
    let (contents, revision) = {
        let datamodel = project.datamodel.lock().unwrap();
        (datamodel.shared(), datamodel.revision())
    };
    let runs = tool_session.runs(&contents, revision);
    drop(tool_session);
    let json = serde_json::to_vec(&runs).expect("a run listing serializes");
    write_bytes_to_ffi_output(&json, out_buf, out_len, out_error, "a run listing");
}
