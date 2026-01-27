// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Vensim view/sketch parsing.
//!
//! This module parses the sketch section of Vensim MDL files, which follows
//! the equation section and starts with `\\\---///`.

pub mod convert;
pub mod elements;
pub mod processing;
pub mod types;

pub use convert::build_views;
pub use types::{
    VensimComment, VensimConnector, VensimElement, VensimValve, VensimVariable, VensimView,
    ViewError, ViewHeader, ViewVersion,
};

use elements::parse_element_line;

/// Parse all views from the sketch section of an MDL file.
///
/// The source should be the remaining content after the equations section.
/// It may start with the `\\\---///` marker or have some leading content.
///
/// Returns an empty vector if no views are found.
pub fn parse_views(source: &str) -> Result<Vec<VensimView>, ViewError> {
    let mut parser = ViewSectionParser::new(source);
    parser.parse_all()
}

/// Parser for the view/sketch section of an MDL file.
struct ViewSectionParser<'a> {
    lines: std::iter::Peekable<std::str::Lines<'a>>,
    line_number: usize,
    /// Set to true when the previous element consumed the next line (scratch_name).
    pending_scratch_name: bool,
    /// The last comment element that needs its text set from the next line.
    pending_scratch_comment: Option<(i32, VensimComment)>,
}

impl<'a> ViewSectionParser<'a> {
    /// Create a new view section parser.
    fn new(source: &'a str) -> Self {
        ViewSectionParser {
            lines: source.lines().peekable(),
            line_number: 0,
            pending_scratch_name: false,
            pending_scratch_comment: None,
        }
    }

    /// Read the next line, incrementing the line number.
    fn read_line(&mut self) -> Option<&'a str> {
        let line = self.lines.next();
        if line.is_some() {
            self.line_number += 1;
        }
        line
    }

    /// Peek at the next line without consuming it.
    fn peek_line(&mut self) -> Option<&&'a str> {
        self.lines.peek()
    }

    /// Skip to the sketch section start.
    ///
    /// The sketch section may start with:
    /// 1. The `\\\---///` marker (if not already consumed by EquationReader)
    /// 2. A version line (V300 or V364) directly
    ///
    /// This function handles both cases by looking for either the marker
    /// or the version line. If we find a version line directly, we don't
    /// consume it (the caller will parse it).
    fn skip_to_sketch_start(&mut self) -> bool {
        while let Some(line) = self.peek_line() {
            let line = *line;
            // Found sketch marker - consume it and return
            if line.starts_with("\\\\\\---///") {
                self.read_line();
                return true;
            }
            // Found version line - don't consume, we're at sketch content
            if line.starts_with("V300") || line.starts_with("V364") {
                return true;
            }
            // Skip other lines (blank, leftover equation content, etc.)
            self.read_line();
        }
        false
    }

    /// Parse all views until `///---\\\` or EOF.
    fn parse_all(&mut self) -> Result<Vec<VensimView>, ViewError> {
        let mut views = Vec::new();

        // Skip to the first sketch marker
        if !self.skip_to_sketch_start() {
            return Ok(views);
        }

        // Parse views in a loop
        loop {
            // Check for end marker or EOF
            match self.peek_line() {
                None => break,
                Some(line) if line.starts_with("///---\\\\\\") => break,
                _ => {}
            }

            // Parse version line
            let version = self.parse_version()?;

            // Parse title line - check if we have one
            let title = match self.peek_line() {
                Some(line) if line.starts_with('*') => {
                    let title_line = self.read_line().ok_or(ViewError::UnexpectedEndOfInput)?;
                    title_line[1..].to_string()
                }
                Some(line) if line.starts_with("///---\\\\\\") => {
                    // No title, hit end marker - empty view section
                    break;
                }
                Some(line) if line.starts_with('$') => {
                    // Font line without title - use default title
                    "View".to_string()
                }
                Some(line) if line.chars().next().is_some_and(|c| c.is_ascii_digit()) => {
                    // Element line without title - use default title
                    "View".to_string()
                }
                _ => {
                    // Something else - skip until we find view content or end
                    break;
                }
            };

            // Skip font line if present (we ignore PPI values per xmutil)
            if let Some(line) = self.peek_line()
                && line.starts_with('$')
            {
                self.read_line();
            }

            // Create view with header
            let header = ViewHeader { version, title };
            let mut view = VensimView::new(header);

            // Parse elements
            self.parse_elements(&mut view)?;

            views.push(view);

            // Check if we hit another sketch marker (multi-view)
            if let Some(line) = self.peek_line()
                && line.starts_with("\\\\\\---///")
            {
                self.read_line(); // consume the marker
            }
        }

        Ok(views)
    }

    /// Parse the version line (V300 or V364).
    fn parse_version(&mut self) -> Result<ViewVersion, ViewError> {
        let line = self.read_line().ok_or(ViewError::UnexpectedEndOfInput)?;

        if line.starts_with("V300 ") || line == "V300" {
            Ok(ViewVersion::V300)
        } else if line.starts_with("V364 ") || line == "V364" {
            Ok(ViewVersion::V364)
        } else {
            Err(ViewError::InvalidVersion(line.to_string()))
        }
    }

    /// Parse elements until we hit a non-element line.
    ///
    /// End-of-view detection is tricky: we stop when we see a line that
    /// doesn't start with a digit, UNLESS the previous element had
    /// scratch_name set (in which case we consume that line as text).
    fn parse_elements(&mut self, view: &mut VensimView) -> Result<(), ViewError> {
        loop {
            // Handle pending scratch name from previous comment
            if self.pending_scratch_name {
                if let Some((uid, mut comment)) = self.pending_scratch_comment.take() {
                    if let Some(text_line) = self.read_line() {
                        comment.text = text_line.to_string();
                    }
                    view.insert(uid, VensimElement::Comment(comment));
                }
                self.pending_scratch_name = false;
            }

            // Check next line
            let line = match self.peek_line() {
                None => break,
                Some(line) => *line,
            };

            // Check for end-of-view markers
            if line.starts_with("///---\\\\\\") || line.starts_with("\\\\\\---///") {
                break;
            }

            // Check if line starts with digit (element line)
            let first_char = line.chars().next().unwrap_or(' ');
            if !first_char.is_ascii_digit() {
                break;
            }

            // Consume and parse the line
            let line = self.read_line().ok_or(ViewError::UnexpectedEndOfInput)?;

            match parse_element_line(line) {
                Ok(Some((element, scratch_name))) => {
                    if scratch_name {
                        // Extract the comment and defer insertion
                        if let VensimElement::Comment(comment) = element {
                            self.pending_scratch_name = true;
                            self.pending_scratch_comment = Some((comment.uid, comment));
                        }
                    } else {
                        view.insert(element.uid(), element);
                    }
                }
                Ok(None) => {
                    // Ignored element type (e.g., type 30)
                }
                Err(e) => return Err(e),
            }
        }

        // Handle any remaining pending scratch comment
        if let Some((uid, comment)) = self.pending_scratch_comment.take() {
            view.insert(uid, VensimElement::Comment(comment));
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_empty_source() {
        let views = parse_views("").unwrap();
        assert!(views.is_empty());
    }

    #[test]
    fn test_parse_no_sketch_section() {
        let source = "some random content\nwithout any sketch markers";
        let views = parse_views(source).unwrap();
        assert!(views.is_empty());
    }

    #[test]
    fn test_parse_simple_view() {
        let source = r#"\\\---/// Sketch information
V300  Do not put anything below this section
*View 1
$192-192-192,0,Helvetica|10|B|0-0-0|0-0-0|0-0-0|-1--1--1|-1--1--1|96,96,100,0
10,1,Test Variable,100,200,40,20,3,3,0,0,0,0,0,0
///---\\\
"#;
        let views = parse_views(source).unwrap();
        assert_eq!(views.len(), 1);
        assert_eq!(views[0].title(), "View 1");

        let elem = views[0].get(1).expect("Element 1 should exist");
        if let VensimElement::Variable(v) = elem {
            assert_eq!(v.name, "Test Variable");
            assert_eq!(v.x, 100);
            assert_eq!(v.y, 200);
        } else {
            panic!("Expected Variable element");
        }
    }

    #[test]
    fn test_parse_invalid_version() {
        let source = r#"\\\---///
V999 Unknown version
*View 1
"#;
        let result = parse_views(source);
        assert!(matches!(result, Err(ViewError::InvalidVersion(_))));
    }

    #[test]
    fn test_parse_scratch_name_comment() {
        // Comment with scratch_name: bits=4 means next line has text
        let source = r#"\\\---///
V300
*View 1
$font
12,1,0,100,100,15,15,5,4,0,0,-1,0,0,0
B
///---\\\
"#;
        let views = parse_views(source).unwrap();
        assert_eq!(views.len(), 1);

        let elem = views[0].get(1).expect("Element 1 should exist");
        if let VensimElement::Comment(c) = elem {
            assert_eq!(c.text, "B");
        } else {
            panic!("Expected Comment element");
        }
    }

    #[test]
    fn test_parse_multiple_elements() {
        let source = r#"\\\---///
V300
*Test View
$font
10,1,Variable A,100,100,40,20,3,3,0,0,0,0,0,0
10,2,Variable B,200,200,40,20,3,3,0,0,0,0,0,0
1,3,1,2,0,0,0,0,0,0,0,-1--1--1,,1|(150,150)|
///---\\\
"#;
        let views = parse_views(source).unwrap();
        assert_eq!(views.len(), 1);

        assert!(views[0].get(1).is_some());
        assert!(views[0].get(2).is_some());
        assert!(views[0].get(3).is_some());

        if let Some(VensimElement::Connector(c)) = views[0].get(3) {
            assert_eq!(c.from_uid, 1);
            assert_eq!(c.to_uid, 2);
        }
    }

    #[test]
    fn test_parse_valve_and_flow() {
        let source = r#"\\\---///
V300
*View 1
$font
11,1,444,100,100,6,8,34,3,0,0,1,0,0,0
10,2,Flow Rate,100,120,40,20,40,3,0,0,-1,0,0,0
///---\\\
"#;
        let views = parse_views(source).unwrap();
        assert_eq!(views.len(), 1);

        if let Some(VensimElement::Valve(v)) = views[0].get(1) {
            assert!(v.attached);
        } else {
            panic!("Expected Valve element");
        }

        if let Some(VensimElement::Variable(v)) = views[0].get(2) {
            assert!(v.attached);
            assert_eq!(v.name, "Flow Rate");
        } else {
            panic!("Expected Variable element");
        }
    }

    #[test]
    fn test_parse_sir_model_view() {
        // Partial content from SIR.mdl
        let source = r#"\\\---/// Sketch information - do not modify anything except names
V300  Do not put anything below this section - it will be ignored
*View 1
$192-192-192,0,Helvetica|10|B|0-0-0|0-0-0|0-0-0|-1--1--1|-1--1--1|96,96,100,0
10,1,Susceptible Population S,162,192,40,20,3,3,0,0,0,0,0,0
10,2,Infectious Population I,428,190,40,20,3,3,0,0,0,0,0,0
1,3,5,2,4,0,0,22,0,0,0,-1--1--1,,1|(344,191)|
1,4,5,1,100,0,0,22,0,0,0,-1--1--1,,1|(245,191)|
11,5,444,295,191,6,8,34,3,0,0,1,0,0,0
10,6,Infection Rate,295,228,40,29,40,3,0,0,-1,0,0,0
12,13,0,232,218,15,15,5,4,0,0,-1,0,0,0
B
///---\\\
"#;
        let views = parse_views(source).unwrap();
        assert_eq!(views.len(), 1);
        assert_eq!(views[0].title(), "View 1");

        // Check stocks
        if let Some(VensimElement::Variable(v)) = views[0].get(1) {
            assert_eq!(v.name, "Susceptible Population S");
            assert!(!v.is_ghost);
        }

        // Check valve
        if let Some(VensimElement::Valve(v)) = views[0].get(5) {
            assert!(v.attached);
        }

        // Check flow
        if let Some(VensimElement::Variable(v)) = views[0].get(6) {
            assert_eq!(v.name, "Infection Rate");
            assert!(v.attached);
        }

        // Check scratch_name comment
        if let Some(VensimElement::Comment(c)) = views[0].get(13) {
            assert_eq!(c.text, "B");
        }

        // Check connector
        if let Some(VensimElement::Connector(c)) = views[0].get(3) {
            assert_eq!(c.from_uid, 5);
            assert_eq!(c.to_uid, 2);
            assert_eq!(c.control_point, (344, 191));
        }
    }

    #[test]
    fn test_parse_ghost_variable() {
        let source = r#"\\\---///
V300
*View 1
$font
10,1,Contact Rate c,249,523,40,20,8,2,0,3,-1,0,0,0,128-128-128,0-0-0,|12|B|128-128-128
///---\\\
"#;
        let views = parse_views(source).unwrap();

        if let Some(VensimElement::Variable(v)) = views[0].get(1) {
            assert!(v.is_ghost); // bits=2 means bit 0 is not set
        } else {
            panic!("Expected Variable element");
        }
    }

    #[test]
    fn test_parse_view_with_leading_content() {
        // Source may have content before the sketch marker
        let source = r#"some equation stuff
~ units ~|
\\\---///
V300
*Test
$font
10,1,X,100,100,40,20,3,3,0,0,0,0,0,0
///---\\\
"#;
        let views = parse_views(source).unwrap();
        assert_eq!(views.len(), 1);
        assert!(views[0].get(1).is_some());
    }
}
