// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Extension-driven format dispatch from raw file contents to
//! `datamodel::Project`.
//!
//! Phase 5 will consolidate this with `simlin-mcp`'s `open_project` (which
//! also handles content-based JSON-shape detection for SD-AI input). For
//! now the surface is minimal: one dispatcher per `ProjectFormat`. The
//! reverse direction (project -> canonical JSON) belongs to
//! `ProjectDoc::current_state_as_json_string`, which every read path goes
//! through so the doc stays the source of truth. Note: `.mdl` is parsed
//! via the native Rust parser (`open_vensim`), not the xmutil C++ path —
//! see Phase 1 note 4 in the implementation plan.

use std::io::Cursor;

use simlin_engine::datamodel;
use simlin_engine::json;

use crate::registry::ProjectFormat;

/// Errors raised while turning raw file contents into a `datamodel::Project`.
#[derive(Debug)]
pub enum ParseError {
    /// The format-specific parser failed. The string carries a human-readable
    /// description; the underlying error type isn't exposed because callers
    /// only need to render it in HTTP responses.
    Format(String),
    /// `serde_json` failed to deserialize an `.sd.json` payload.
    Json(serde_json::Error),
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ParseError::Format(msg) => write!(f, "parse error: {msg}"),
            ParseError::Json(e) => write!(f, "json parse error: {e}"),
        }
    }
}

impl std::error::Error for ParseError {}

/// Parse `contents` into a `datamodel::Project` using `format` to pick the
/// right backend. The `path` is currently informational only (used by future
/// data-provider hooks for `GET DIRECT *` resolution); Phase 1 doesn't read
/// any sibling files itself.
pub fn parse_to_datamodel(
    _path: &std::path::Path,
    format: ProjectFormat,
    contents: &str,
) -> Result<datamodel::Project, ParseError> {
    match format {
        ProjectFormat::Stmx | ProjectFormat::Xmile => {
            let mut reader = Cursor::new(contents.as_bytes());
            simlin_engine::open_xmile(&mut reader)
                .map_err(|e| ParseError::Format(format!("XMILE: {e:?}")))
        }
        ProjectFormat::Mdl => simlin_engine::open_vensim(contents)
            .map_err(|e| ParseError::Format(format!("Vensim MDL: {e:?}"))),
        ProjectFormat::SdJson => {
            let json_project: json::Project =
                serde_json::from_str(contents).map_err(ParseError::Json)?;
            Ok(json_project.into())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sd_json_parses_into_a_datamodel_project() {
        let input = r#"{"name":"x","simSpecs":{"startTime":0,"endTime":10,"dt":"1","method":"euler"},"models":[{"name":"main"}]}"#;
        let project = parse_to_datamodel(
            std::path::Path::new("x.sd.json"),
            ProjectFormat::SdJson,
            input,
        )
        .expect("parses");
        assert_eq!(project.name, "x");
        assert_eq!(project.models.len(), 1);
        assert_eq!(project.models[0].name, "main");
    }

    #[test]
    fn invalid_sd_json_returns_error() {
        let input = "not actually json";
        let result = parse_to_datamodel(
            std::path::Path::new("bad.sd.json"),
            ProjectFormat::SdJson,
            input,
        );
        assert!(matches!(result, Err(ParseError::Json(_))));
    }

    #[test]
    fn invalid_xmile_returns_format_error() {
        // Truncated/malformed XML must fail at the parse layer; the leniency
        // of the XMILE schema (it accepts unknown root elements quietly) means
        // we have to feed actually-broken XML to confirm the error wiring.
        let result = parse_to_datamodel(
            std::path::Path::new("bad.stmx"),
            ProjectFormat::Stmx,
            "<unclosed",
        );
        assert!(
            matches!(result, Err(ParseError::Format(_))),
            "expected Format error, got {result:?}"
        );
    }
}
