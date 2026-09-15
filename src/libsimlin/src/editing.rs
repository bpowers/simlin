// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Diagram editing FFI: hit testing, taps, drag gestures, and the patches a
//! delete and a rename imply.
//!
//! A host draws a model's scene (`simlin_project_render_scene`) and routes its
//! input through these entry points, which plan against the model's first
//! stock-and-flow view as it is when a tap lands or a drag begins
//! (`simlin_engine::editing`). Every edit comes back as a JSON project patch the
//! host applies with `simlin_project_apply_patch`, so an edit lands through the
//! one patch path with its validation and error collection; a view edit's model
//! operations are derived when it applies (`ModelOperation::EditView`).

use serde::Serialize;
use simlin_engine::diagram::SceneElement;
use simlin_engine::{self as engine, datamodel, editing};
use std::ffi::CStr;
use std::os::raw::c_char;
use std::ptr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use crate::ffi_error::SimlinError;
use crate::{
    clear_out_error, require_model, store_anyhow_error, store_error, write_bytes_to_ffi_output,
    SimlinErrorCode, SimlinModel,
};

/// Which part of an element a hit lands on.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SimlinHitPart {
    /// The element itself: a shape, a flow's pipe or valve, a link's line.
    Body = 0,
    /// A flow's sink end, or a link's arrowhead.
    Arrowhead = 1,
    /// A flow's source end.
    Source = 2,
    /// The element's name label.
    Label = 3,
}

impl From<editing::HitPart> for SimlinHitPart {
    fn from(part: editing::HitPart) -> Self {
        match part {
            editing::HitPart::Body => SimlinHitPart::Body,
            editing::HitPart::Arrowhead => SimlinHitPart::Arrowhead,
            editing::HitPart::Source => SimlinHitPart::Source,
            editing::HitPart::Label => SimlinHitPart::Label,
        }
    }
}

impl From<SimlinHitPart> for editing::HitPart {
    fn from(part: SimlinHitPart) -> Self {
        match part {
            SimlinHitPart::Body => editing::HitPart::Body,
            SimlinHitPart::Arrowhead => editing::HitPart::Arrowhead,
            SimlinHitPart::Source => editing::HitPart::Source,
            SimlinHitPart::Label => editing::HitPart::Label,
        }
    }
}

/// The tool a host's toolbar arms.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SimlinTool {
    None = 0,
    Stock = 1,
    Flow = 2,
    Aux = 3,
    Link = 4,
    Module = 5,
}

impl From<SimlinTool> for Option<editing::Tool> {
    fn from(tool: SimlinTool) -> Self {
        match tool {
            SimlinTool::None => None,
            SimlinTool::Stock => Some(editing::Tool::Stock),
            SimlinTool::Flow => Some(editing::Tool::Flow),
            SimlinTool::Aux => Some(editing::Tool::Aux),
            SimlinTool::Link => Some(editing::Tool::Link),
            SimlinTool::Module => Some(editing::Tool::Module),
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SimlinPointerKind {
    Touch = 0,
    Pencil = 1,
    Mouse = 2,
}

impl From<SimlinPointerKind> for editing::PointerKind {
    fn from(kind: SimlinPointerKind) -> Self {
        match kind {
            SimlinPointerKind::Touch => editing::PointerKind::Touch,
            SimlinPointerKind::Pencil => editing::PointerKind::Pencil,
            SimlinPointerKind::Mouse => editing::PointerKind::Mouse,
        }
    }
}

/// A press, resolved by the host: where it landed (model coordinates), what it
/// hit (`simlin_model_hit_test`), the armed tool, the selection before it, and
/// the pointer that made it.
#[repr(C)]
pub struct SimlinPress {
    pub x: f64,
    pub y: f64,
    /// Whether the press landed on an element; `hit_uid` and `hit_part` are
    /// read only when it did.
    pub has_hit: bool,
    pub hit_uid: i32,
    pub hit_part: SimlinHitPart,
    pub tool: SimlinTool,
    /// The selection before the press: `selection_len` uids (NULL when zero).
    pub selection: *const i32,
    pub selection_len: usize,
    /// A selection modifier (Shift or Command) is held.
    pub toggle: bool,
    pub pointer: SimlinPointerKind,
    /// How far outside a drop target the pointer may be and still land on it,
    /// in model units (the host's touch slop divided by the zoom).
    pub target_slop: f64,
}

/// A live drag over the view as it was when the drag began.
pub struct SimlinGesture {
    state: Mutex<GestureState>,
    model_name: Arc<String>,
    ref_count: AtomicUsize,
}

struct GestureState {
    session: editing::GestureSession,
    /// The last frame's JSON, reused across frames so a drag allocates no
    /// buffer per frame.
    frame: Vec<u8>,
}

fn ffi_error(code: SimlinErrorCode, message: impl Into<String>) -> SimlinError {
    SimlinError::new(code).with_message(message.into())
}

unsafe fn press_of(press: *const SimlinPress) -> Result<editing::Press, SimlinError> {
    if press.is_null() {
        return Err(ffi_error(
            SimlinErrorCode::Generic,
            "press must not be NULL",
        ));
    }
    let p = &*press;
    let selection = match (p.selection.is_null(), p.selection_len) {
        (_, 0) => Vec::new(),
        (true, _) => {
            return Err(ffi_error(
                SimlinErrorCode::Generic,
                "selection must not be NULL when selection_len is non-zero",
            ))
        }
        (false, len) => std::slice::from_raw_parts(p.selection, len).to_vec(),
    };
    Ok(editing::Press {
        point: editing::Point::new(p.x, p.y),
        hit: p.has_hit.then_some(editing::Hit {
            uid: p.hit_uid,
            part: p.hit_part.into(),
        }),
        tool: p.tool.into(),
        selection,
        toggle: p.toggle,
        pointer: p.pointer.into(),
        target_slop: p.target_slop,
    })
}

/// The model's first stock-and-flow view, indexed for planning.
fn base_view(
    project: &datamodel::Project,
    model_name: &str,
) -> Result<editing::BaseView, SimlinError> {
    first_view(project, model_name).map(|(model, view)| editing::BaseView::new(model, view))
}

fn kind_name(kind: Option<editing::GestureKind>) -> Option<&'static str> {
    Some(match kind? {
        editing::GestureKind::MoveSelection => "moveSelection",
        editing::GestureKind::SlideValve => "slideValve",
        editing::GestureKind::OffsetSegment => "offsetSegment",
        editing::GestureKind::FlowEndpoint => "flowEndpoint",
        editing::GestureKind::LinkEndpoint => "linkEndpoint",
        editing::GestureKind::LinkArc => "linkArc",
        editing::GestureKind::CreateFlow => "createFlow",
        editing::GestureKind::CreateLink => "createLink",
        editing::GestureKind::CreateElement => "createElement",
        editing::GestureKind::Label => "label",
        editing::GestureKind::RubberBand => "rubberBand",
    })
}

fn commit_name(commit: editing::CommitKind) -> &'static str {
    match commit {
        editing::CommitKind::None => "none",
        editing::CommitKind::Edit => "edit",
        editing::CommitKind::Select => "select",
    }
}

#[derive(Serialize)]
struct TargetJson {
    uid: i32,
    valid: bool,
}

/// One frame of a drag, as `simlin_gesture_frame` writes it.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FrameJson<'a> {
    kind: Option<&'static str>,
    commit: &'static str,
    selection: &'a [i32],
    target: Option<TargetJson>,
    handoff: Option<i32>,
    details: bool,
    label: &'a str,
    hidden: &'a [i32],
    elements: &'a [SceneElement],
}

#[derive(Serialize)]
#[serde(tag = "type", content = "payload", rename_all = "camelCase")]
enum OpJson {
    EditView {
        index: u32,
        upsert: Vec<engine::json::ViewElement>,
        remove: Vec<i32>,
    },
    RenameVariable {
        from: String,
        to: String,
    },
}

#[derive(Serialize)]
struct ModelPatchJson<'a> {
    name: &'a str,
    ops: Vec<OpJson>,
}

/// A project patch in the `simlin_project_apply_patch` format.
#[derive(Serialize)]
struct PatchJson<'a> {
    models: [ModelPatchJson<'a>; 1],
}

fn edit_patch<'a>(model_name: &'a str, edit: editing::ViewEdit) -> PatchJson<'a> {
    PatchJson {
        models: [ModelPatchJson {
            name: model_name,
            ops: vec![OpJson::EditView {
                index: 0,
                upsert: edit.upsert.into_iter().map(Into::into).collect(),
                remove: edit.remove,
            }],
        }],
    }
}

/// The end of a gesture or a tap, as `simlin_gesture_commit` and
/// `simlin_model_plan_tap` write it.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CommitJson<'a> {
    kind: Option<&'static str>,
    commit: &'static str,
    selection: &'a [i32],
    handoff: Option<i32>,
    details: bool,
    label: &'a str,
    patch: Option<PatchJson<'a>>,
}

fn commit_json<'a>(
    kind: Option<editing::GestureKind>,
    plan: &'a editing::Plan,
    edit: Option<editing::ViewEdit>,
    model_name: &'a str,
) -> Result<Vec<u8>, SimlinError> {
    let json = CommitJson {
        kind: kind_name(kind),
        commit: commit_name(plan.commit),
        selection: &plan.selection,
        handoff: plan.handoff,
        details: plan.details,
        label: plan.label,
        patch: edit.map(|edit| edit_patch(model_name, edit)),
    };
    serde_json::to_vec(&json)
        .map_err(|e| ffi_error(SimlinErrorCode::Generic, format!("serializing a plan: {e}")))
}

unsafe fn require_outputs(
    out_buf: *mut *mut u8,
    out_len: *mut usize,
    out_error: *mut *mut SimlinError,
) -> bool {
    if out_buf.is_null() || out_len.is_null() {
        store_error(
            out_error,
            ffi_error(SimlinErrorCode::Generic, "output pointers must not be NULL"),
        );
        return false;
    }
    *out_buf = ptr::null_mut();
    *out_len = 0;
    true
}

/// The element and part of the model's diagram that `(x, y)` (model
/// coordinates) lands on, decided in `simlin_engine::editing::HitIndex`'s tiers:
/// a body firmly holding the point, else an end handle within reach, else a
/// label holding the point, else the nearest drawing within `tolerance` model
/// units (the host's touch slop divided by the zoom). Writes `*out_hit = false`
/// when nothing drawn is within reach. Refuses a model with no stock-and-flow
/// view with `DoesNotExist`, as every editing entry point does.
///
/// The view's index is built by the first hit test after the project changes
/// and reused until it changes again (`ProjectContents`), so a hover at display
/// rate costs in proportion to what is near the point.
///
/// # Safety
/// - `model` must be a valid pointer to a SimlinModel
/// - `out_hit`, `out_uid` and `out_part` must be valid, non-null pointers
/// - `out_error` may be null
#[no_mangle]
pub unsafe extern "C" fn simlin_model_hit_test(
    model: *mut SimlinModel,
    x: f64,
    y: f64,
    tolerance: f64,
    out_hit: *mut bool,
    out_uid: *mut i32,
    out_part: *mut SimlinHitPart,
    out_error: *mut *mut SimlinError,
) {
    clear_out_error(out_error);
    if out_hit.is_null() || out_uid.is_null() || out_part.is_null() {
        store_error(
            out_error,
            ffi_error(SimlinErrorCode::Generic, "output pointers must not be NULL"),
        );
        return;
    }
    *out_hit = false;
    *out_uid = 0;
    *out_part = SimlinHitPart::Body;
    let model_ref = match require_model(model) {
        Ok(m) => m,
        Err(err) => {
            store_anyhow_error(out_error, err);
            return;
        }
    };
    let project_ref = &*model_ref.project;
    let mut contents = project_ref.datamodel.lock().unwrap();
    let model_name = model_ref.model_name.as_str();
    // The scene draws a viewless model through a transient layout, which a hit
    // could land on but no edit could change: refused here as the planners
    // refuse it.
    if let Err(err) = first_view(&contents, model_name) {
        store_error(out_error, err);
        return;
    }
    match contents.hit_index(model_name) {
        Ok(index) => {
            if let Some(hit) = index.hit(editing::Point::new(x, y), tolerance) {
                *out_hit = true;
                *out_uid = hit.uid;
                *out_part = hit.part.into();
            }
        }
        // The model and its view exist, so what is left to fail is internal.
        Err(message) => store_error(out_error, ffi_error(SimlinErrorCode::Generic, message)),
    }
}

/// Plan a tap. Writes a JSON object to a buffer the caller frees with
/// `simlin_free`: `commit` (`"none"`, `"edit"` or `"select"`), `selection` (what
/// the host adopts), `handoff` (a uid whose name editor opens once the edit
/// lands, or null), `details` (the tap opens the element's details), `label`
/// (the edit's name for history), and `patch` (the patch to apply for an
/// `"edit"`, else null).
///
/// # Safety
/// - `model` must be a valid pointer to a SimlinModel
/// - `press` must be a valid pointer to a SimlinPress
/// - `out_buf` and `out_len` must be valid, non-null pointers
/// - `out_error` may be null
#[no_mangle]
pub unsafe extern "C" fn simlin_model_plan_tap(
    model: *mut SimlinModel,
    press: *const SimlinPress,
    out_buf: *mut *mut u8,
    out_len: *mut usize,
    out_error: *mut *mut SimlinError,
) {
    clear_out_error(out_error);
    if !require_outputs(out_buf, out_len, out_error) {
        return;
    }
    let model_ref = match require_model(model) {
        Ok(m) => m,
        Err(err) => {
            store_anyhow_error(out_error, err);
            return;
        }
    };
    let press = match press_of(press) {
        Ok(p) => p,
        Err(err) => {
            store_error(out_error, err);
            return;
        }
    };
    let bytes = {
        let project_ref = &*model_ref.project;
        let datamodel = project_ref.datamodel.lock().unwrap();
        let base = match base_view(&datamodel, model_ref.model_name.as_str()) {
            Ok(base) => base,
            Err(err) => {
                store_error(out_error, err);
                return;
            }
        };
        let plan = editing::plan_tap(&base, &press);
        commit_json(None, &plan, plan.edit(&base), model_ref.model_name.as_str())
    };
    match bytes {
        Ok(bytes) => {
            write_bytes_to_ffi_output(&bytes, out_buf, out_len, out_error, "a tap plan");
        }
        Err(err) => store_error(out_error, err),
    }
}

/// Begin a drag. Returns the gesture, released with `simlin_gesture_unref`; or
/// NULL, with `*out_error` set when the call failed and with no error when the
/// press starts no drag (a finger on the empty canvas, which pans). The gesture
/// plans against the view as it is now: a host that applies an edit while a
/// drag is live ends the drag.
///
/// # Safety
/// - `model` must be a valid pointer to a SimlinModel
/// - `press` must be a valid pointer to a SimlinPress
/// - `out_error` may be null
#[no_mangle]
pub unsafe extern "C" fn simlin_gesture_begin(
    model: *mut SimlinModel,
    press: *const SimlinPress,
    out_error: *mut *mut SimlinError,
) -> *mut SimlinGesture {
    clear_out_error(out_error);
    let model_ref = match require_model(model) {
        Ok(m) => m,
        Err(err) => {
            store_anyhow_error(out_error, err);
            return ptr::null_mut();
        }
    };
    let press = match press_of(press) {
        Ok(p) => p,
        Err(err) => {
            store_error(out_error, err);
            return ptr::null_mut();
        }
    };
    let base = {
        let project_ref = &*model_ref.project;
        let datamodel = project_ref.datamodel.lock().unwrap();
        match base_view(&datamodel, model_ref.model_name.as_str()) {
            Ok(base) => base,
            Err(err) => {
                store_error(out_error, err);
                return ptr::null_mut();
            }
        }
    };
    match editing::begin_drag(base, press) {
        Some(session) => Box::into_raw(Box::new(SimlinGesture {
            state: Mutex::new(GestureState {
                session,
                frame: Vec::new(),
            }),
            model_name: model_ref.model_name.clone(),
            ref_count: AtomicUsize::new(1),
        })),
        None => ptr::null_mut(),
    }
}

unsafe fn require_gesture<'a>(
    gesture: *mut SimlinGesture,
) -> Result<&'a SimlinGesture, SimlinError> {
    if gesture.is_null() {
        Err(ffi_error(
            SimlinErrorCode::Generic,
            "gesture must not be NULL",
        ))
    } else {
        Ok(&*gesture)
    }
}

/// Plan the frame with the pointer at `(x, y)` (model coordinates). Writes a
/// JSON object to a buffer the gesture owns, valid until the next call on this
/// gesture or its release: `kind` (the gesture, null while a press on a pipe has
/// not yet moved), `commit`, `selection`, `target` (a drop target `{uid, valid}`
/// to highlight, or null), `handoff`, `details`, `label`, `hidden` (uids of base
/// scene elements the frame does not draw), and `elements` (scene elements, the
/// `simlin_project_render_scene` contract, drawn among the base scene at their
/// layers: a link's arc runs between the centers of what it connects, and the
/// elements above it hide its ends).
///
/// # Safety
/// - `gesture` must be a valid pointer to a SimlinGesture
/// - `out_buf` and `out_len` must be valid, non-null pointers
/// - `out_error` may be null
#[no_mangle]
pub unsafe extern "C" fn simlin_gesture_frame(
    gesture: *mut SimlinGesture,
    x: f64,
    y: f64,
    out_buf: *mut *const u8,
    out_len: *mut usize,
    out_error: *mut *mut SimlinError,
) {
    clear_out_error(out_error);
    if out_buf.is_null() || out_len.is_null() {
        store_error(
            out_error,
            ffi_error(SimlinErrorCode::Generic, "output pointers must not be NULL"),
        );
        return;
    }
    *out_buf = ptr::null();
    *out_len = 0;
    let gesture = match require_gesture(gesture) {
        Ok(g) => g,
        Err(err) => {
            store_error(out_error, err);
            return;
        }
    };
    let mut state = gesture.state.lock().unwrap();
    let GestureState { session, frame } = &mut *state;
    let plan = session.frame(editing::Point::new(x, y));
    let preview = editing::preview(session.base(), &plan);
    let json = FrameJson {
        kind: kind_name(session.kind()),
        commit: commit_name(plan.commit),
        selection: &plan.selection,
        target: plan.target.map(|t| TargetJson {
            uid: t.uid,
            valid: t.valid,
        }),
        handoff: plan.handoff,
        details: plan.details,
        label: plan.label,
        hidden: &preview.hidden,
        elements: &preview.elements,
    };
    frame.clear();
    if let Err(e) = serde_json::to_writer(&mut *frame, &json) {
        store_error(
            out_error,
            ffi_error(
                SimlinErrorCode::Generic,
                format!("serializing a frame: {e}"),
            ),
        );
        return;
    }
    *out_buf = frame.as_ptr();
    *out_len = frame.len();
}

/// Plan the release with the pointer at `(x, y)`: the frame the preview showed
/// there. Writes the same JSON object as `simlin_model_plan_tap` to a buffer the
/// caller frees with `simlin_free`, with `patch` null when the release changes
/// nothing (an invalid drop, a drag back to where it started).
///
/// # Safety
/// - `gesture` must be a valid pointer to a SimlinGesture
/// - `out_buf` and `out_len` must be valid, non-null pointers
/// - `out_error` may be null
#[no_mangle]
pub unsafe extern "C" fn simlin_gesture_commit(
    gesture: *mut SimlinGesture,
    x: f64,
    y: f64,
    out_buf: *mut *mut u8,
    out_len: *mut usize,
    out_error: *mut *mut SimlinError,
) {
    clear_out_error(out_error);
    if !require_outputs(out_buf, out_len, out_error) {
        return;
    }
    let gesture = match require_gesture(gesture) {
        Ok(g) => g,
        Err(err) => {
            store_error(out_error, err);
            return;
        }
    };
    let bytes = {
        let mut state = gesture.state.lock().unwrap();
        let plan = state.session.frame(editing::Point::new(x, y));
        let edit = plan.edit(state.session.base());
        commit_json(
            state.session.kind(),
            &plan,
            edit,
            gesture.model_name.as_str(),
        )
    };
    match bytes {
        Ok(bytes) => {
            write_bytes_to_ffi_output(&bytes, out_buf, out_len, out_error, "a gesture commit");
        }
        Err(err) => store_error(out_error, err),
    }
}

/// Increment a gesture's reference count.
///
/// # Safety
/// - `gesture` must be a valid pointer to a SimlinGesture, or NULL
#[no_mangle]
pub unsafe extern "C" fn simlin_gesture_ref(gesture: *mut SimlinGesture) {
    if !gesture.is_null() {
        (*gesture).ref_count.fetch_add(1, Ordering::SeqCst);
    }
}

/// Decrement a gesture's reference count, releasing it at zero.
///
/// # Safety
/// - `gesture` must be a valid pointer to a SimlinGesture, or NULL
#[no_mangle]
pub unsafe extern "C" fn simlin_gesture_unref(gesture: *mut SimlinGesture) {
    if gesture.is_null() {
        return;
    }
    if (*gesture).ref_count.fetch_sub(1, Ordering::SeqCst) == 1 {
        std::sync::atomic::fence(Ordering::SeqCst);
        drop(Box::from_raw(gesture));
    }
}

/// The patch deleting `selection` (`selection_len` uids) from the model's
/// diagram: the selected elements, the clouds of removed flows, the aliases of
/// removed elements, every link touching a removed element, and a new cloud at
/// every surviving flow end attached to a removed element; applying it deletes
/// the variables and updates the stock lists. Written to a buffer the caller
/// frees with `simlin_free`.
///
/// # Safety
/// - `model` must be a valid pointer to a SimlinModel
/// - `selection` must point to `selection_len` uids, or be NULL when it is zero
/// - `out_buf` and `out_len` must be valid, non-null pointers
/// - `out_error` may be null
#[no_mangle]
pub unsafe extern "C" fn simlin_model_plan_delete(
    model: *mut SimlinModel,
    selection: *const i32,
    selection_len: usize,
    out_buf: *mut *mut u8,
    out_len: *mut usize,
    out_error: *mut *mut SimlinError,
) {
    clear_out_error(out_error);
    if !require_outputs(out_buf, out_len, out_error) {
        return;
    }
    let model_ref = match require_model(model) {
        Ok(m) => m,
        Err(err) => {
            store_anyhow_error(out_error, err);
            return;
        }
    };
    if selection.is_null() && selection_len > 0 {
        store_error(
            out_error,
            ffi_error(
                SimlinErrorCode::Generic,
                "selection must not be NULL when selection_len is non-zero",
            ),
        );
        return;
    }
    let selection: &[i32] = if selection_len == 0 {
        &[]
    } else {
        std::slice::from_raw_parts(selection, selection_len)
    };
    let model_name = model_ref.model_name.as_str();
    let result = {
        let project_ref = &*model_ref.project;
        let datamodel = project_ref.datamodel.lock().unwrap();
        first_view(&datamodel, model_name).map(|(_, view)| editing::plan_delete(view, selection))
    };
    let edit = match result {
        Ok(edit) => edit,
        Err(err) => {
            store_error(out_error, err);
            return;
        }
    };
    write_patch(&edit_patch(model_name, edit), out_buf, out_len, out_error);
}

/// The patch renaming the variable `old_name` to `new_name` (as typed, stored
/// verbatim): its elements relabeled on the model's diagram when it has any --
/// applying the edit then renames the variable -- else a direct rename. Written
/// to a buffer the caller frees with `simlin_free`.
///
/// # Safety
/// - `model` must be a valid pointer to a SimlinModel
/// - `old_name` and `new_name` must be valid NUL-terminated UTF-8 strings
/// - `out_buf` and `out_len` must be valid, non-null pointers
/// - `out_error` may be null
#[no_mangle]
pub unsafe extern "C" fn simlin_model_plan_rename(
    model: *mut SimlinModel,
    old_name: *const c_char,
    new_name: *const c_char,
    out_buf: *mut *mut u8,
    out_len: *mut usize,
    out_error: *mut *mut SimlinError,
) {
    clear_out_error(out_error);
    if !require_outputs(out_buf, out_len, out_error) {
        return;
    }
    let model_ref = match require_model(model) {
        Ok(m) => m,
        Err(err) => {
            store_anyhow_error(out_error, err);
            return;
        }
    };
    let (old_name, new_name) = match (c_str(old_name), c_str(new_name)) {
        (Ok(o), Ok(n)) => (o, n),
        (Err(err), _) | (_, Err(err)) => {
            store_error(out_error, err);
            return;
        }
    };
    let model_name = model_ref.model_name.as_str();
    let result = {
        let project_ref = &*model_ref.project;
        let datamodel = project_ref.datamodel.lock().unwrap();
        first_view(&datamodel, model_name)
            .map(|(_, view)| editing::plan_rename(view, old_name, new_name))
    };
    let edit = match result {
        Ok(edit) => edit,
        Err(err) => {
            store_error(out_error, err);
            return;
        }
    };
    let patch = if edit.is_empty() {
        PatchJson {
            models: [ModelPatchJson {
                name: model_name,
                ops: vec![OpJson::RenameVariable {
                    from: old_name.to_string(),
                    to: new_name.to_string(),
                }],
            }],
        }
    } else {
        edit_patch(model_name, edit)
    };
    write_patch(&patch, out_buf, out_len, out_error);
}

unsafe fn c_str<'a>(s: *const c_char) -> Result<&'a str, SimlinError> {
    if s.is_null() {
        return Err(ffi_error(SimlinErrorCode::Generic, "name must not be NULL"));
    }
    CStr::from_ptr(s)
        .to_str()
        .map_err(|_| ffi_error(SimlinErrorCode::Generic, "name is not valid UTF-8"))
}

/// The model and its first stock-and-flow view, the diagram every editing entry
/// point plans against: `BadModelName` for a missing model, `DoesNotExist` for a
/// model with no view.
fn first_view<'a>(
    project: &'a datamodel::Project,
    model_name: &str,
) -> Result<(&'a datamodel::Model, &'a datamodel::StockFlow), SimlinError> {
    let model = project.get_model(model_name).ok_or_else(|| {
        ffi_error(
            SimlinErrorCode::BadModelName,
            format!("model '{model_name}' not found"),
        )
    })?;
    match model.views.first() {
        Some(datamodel::View::StockFlow(view)) => Ok((model, view)),
        None => Err(ffi_error(
            SimlinErrorCode::DoesNotExist,
            format!("model '{model_name}' has no stock-and-flow view"),
        )),
    }
}

unsafe fn write_patch(
    patch: &PatchJson<'_>,
    out_buf: *mut *mut u8,
    out_len: *mut usize,
    out_error: *mut *mut SimlinError,
) {
    match serde_json::to_vec(patch) {
        Ok(bytes) => {
            write_bytes_to_ffi_output(&bytes, out_buf, out_len, out_error, "a patch");
        }
        Err(e) => store_error(
            out_error,
            ffi_error(
                SimlinErrorCode::Generic,
                format!("serializing a patch: {e}"),
            ),
        ),
    }
}
