use crate::{SimlinErrorCode, SimlinErrorKind, SimlinUnitErrorKind};
use std::ffi::CString;
use std::os::raw::c_char;
use std::ptr;

/// Transcode strings for FFI consumption by stripping interior NUL bytes.
fn sanitize_for_c(value: String) -> CString {
    match CString::new(value) {
        Ok(cstring) => cstring,
        Err(err) => {
            let mut bytes = err.into_vec();
            bytes.retain(|b| *b != 0);
            CString::new(bytes).expect("sanitised string must not contain NUL")
        }
    }
}

/// Builder-friendly structure describing a detailed error.
#[derive(Debug, Clone)]
pub struct ErrorDetail {
    pub code: SimlinErrorCode,
    pub message: Option<String>,
    pub model_name: Option<String>,
    pub variable_name: Option<String>,
    pub start_offset: u16,
    pub end_offset: u16,
    pub kind: SimlinErrorKind,
    pub unit_error_kind: SimlinUnitErrorKind,
}

impl ErrorDetail {
    pub fn new(code: SimlinErrorCode) -> Self {
        Self {
            code,
            message: None,
            model_name: None,
            variable_name: None,
            start_offset: 0,
            end_offset: 0,
            kind: SimlinErrorKind::default(),
            unit_error_kind: SimlinUnitErrorKind::default(),
        }
    }
}

#[derive(Debug)]
struct OwnedDetail {
    code: SimlinErrorCode,
    message: Option<CString>,
    model_name: Option<CString>,
    variable_name: Option<CString>,
    start_offset: u16,
    end_offset: u16,
    kind: SimlinErrorKind,
    unit_error_kind: SimlinUnitErrorKind,
}

impl OwnedDetail {
    fn from(detail: ErrorDetail) -> Self {
        Self {
            code: detail.code,
            message: detail.message.map(sanitize_for_c),
            model_name: detail.model_name.map(sanitize_for_c),
            variable_name: detail.variable_name.map(sanitize_for_c),
            start_offset: detail.start_offset,
            end_offset: detail.end_offset,
            kind: detail.kind,
            unit_error_kind: detail.unit_error_kind,
        }
    }

    fn as_ffi(&self) -> crate::SimlinErrorDetail {
        crate::SimlinErrorDetail {
            code: self.code,
            message: self
                .message
                .as_ref()
                .map_or(ptr::null(), |v| v.as_ptr() as *const c_char),
            model_name: self
                .model_name
                .as_ref()
                .map_or(ptr::null(), |v| v.as_ptr() as *const c_char),
            variable_name: self
                .variable_name
                .as_ref()
                .map_or(ptr::null(), |v| v.as_ptr() as *const c_char),
            start_offset: self.start_offset,
            end_offset: self.end_offset,
            kind: self.kind,
            unit_error_kind: self.unit_error_kind,
        }
    }
}

/// Rich error object passed across the FFI boundary.
pub struct SimlinError {
    code: SimlinErrorCode,
    message: Option<CString>,
    details: Vec<OwnedDetail>,
    ffi_details: Vec<crate::SimlinErrorDetail>,
}

impl SimlinError {
    pub fn new(code: SimlinErrorCode) -> Self {
        Self {
            code,
            message: None,
            details: Vec::new(),
            ffi_details: Vec::new(),
        }
    }

    pub fn with_message(mut self, message: impl Into<String>) -> Self {
        self.set_message(Some(message.into()));
        self
    }

    pub fn clear_message(&mut self) {
        self.message = None;
    }

    pub fn set_message(&mut self, message: Option<String>) {
        self.message = message.map(sanitize_for_c);
    }

    pub fn push_detail(&mut self, detail: ErrorDetail) {
        self.details.push(OwnedDetail::from(detail));
    }

    pub fn extend_details<I>(&mut self, details: I)
    where
        I: IntoIterator<Item = ErrorDetail>,
    {
        self.details
            .extend(details.into_iter().map(OwnedDetail::from));
    }

    fn materialise_details(&mut self) {
        self.ffi_details = self.details.iter().map(OwnedDetail::as_ffi).collect();
    }

    pub fn code(&self) -> SimlinErrorCode {
        self.code
    }

    pub fn message_ptr(&self) -> *const c_char {
        self.message
            .as_ref()
            .map_or(ptr::null(), |msg| msg.as_ptr() as *const c_char)
    }

    pub fn detail_count(&self) -> usize {
        self.details.len()
    }

    pub fn details_ptr(&self) -> *const crate::SimlinErrorDetail {
        if self.ffi_details.is_empty() {
            ptr::null()
        } else {
            self.ffi_details.as_ptr()
        }
    }

    pub fn detail_at(&self, index: usize) -> *const crate::SimlinErrorDetail {
        if index >= self.ffi_details.len() {
            ptr::null()
        } else {
            &self.ffi_details[index] as *const crate::SimlinErrorDetail
        }
    }

    pub fn into_raw(mut self) -> *mut Self {
        self.materialise_details();
        Box::into_raw(Box::new(self))
    }

    /// # Safety
    ///
    /// The pointer must have been created via `into_raw` and must not have been freed already.
    /// After calling this function, the pointer is invalid and must not be used again.
    pub unsafe fn from_raw(ptr: *mut Self) -> Box<Self> {
        Box::from_raw(ptr)
    }
}

/// Wrapper error type that can be embedded inside anyhow chains.
#[derive(Debug, Clone)]
pub struct FfiError {
    pub code: SimlinErrorCode,
    pub message: Option<String>,
    pub details: Vec<ErrorDetail>,
}

impl FfiError {
    pub fn new(code: SimlinErrorCode) -> Self {
        Self {
            code,
            message: None,
            details: Vec::new(),
        }
    }

    pub fn with_message(mut self, message: impl Into<String>) -> Self {
        self.message = Some(message.into());
        self
    }

    pub fn with_detail(mut self, detail: ErrorDetail) -> Self {
        self.details.push(detail);
        self
    }

    pub fn with_details<I>(mut self, details: I) -> Self
    where
        I: IntoIterator<Item = ErrorDetail>,
    {
        self.details.extend(details);
        self
    }

    pub fn into_simlin_error(self) -> SimlinError {
        let mut error = SimlinError::new(self.code);
        error.set_message(self.message);
        error.extend_details(self.details);
        error
    }
}

impl Default for FfiError {
    fn default() -> Self {
        Self::new(SimlinErrorCode::Generic)
    }
}

impl std::fmt::Display for FfiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(message) = &self.message {
            write!(f, "{message}")
        } else {
            write!(f, "{:?}", self.code)
        }
    }
}

impl std::error::Error for FfiError {}
