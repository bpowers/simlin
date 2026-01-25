// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Intermediate AST types for Vensim view/sketch parsing.
//!
//! These types represent the parsed Vensim sketch data before conversion
//! to `datamodel::View` structures.

/// Version of the Vensim sketch format.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViewVersion {
    V300,
    V364,
}

/// Header information for a view.
#[derive(Clone, Debug)]
pub struct ViewHeader {
    pub version: ViewVersion,
    pub title: String,
}

/// A variable element in the view (type 10).
/// Represents stocks, flows, and auxiliaries.
#[derive(Clone, Debug)]
pub struct VensimVariable {
    pub uid: i32,
    pub name: String,
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    /// Whether this variable is attached to a valve (shape bit 5).
    /// For flows, this indicates the variable is connected to a valve element.
    pub attached: bool,
    /// Whether this is a ghost (alias) of another element.
    /// Ghost = !(bits & 1), so bits & 1 == 0 means ghost.
    pub is_ghost: bool,
}

/// A valve element in the view (type 11).
/// Valves represent the flow control point and always precede their
/// associated flow variable in the element list.
#[derive(Clone, Debug)]
pub struct VensimValve {
    pub uid: i32,
    pub name: String,
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    /// Whether this valve is attached to a flow (shape bit 5).
    pub attached: bool,
}

/// A comment element in the view (type 12).
/// Comments include text annotations and clouds (flow boundaries).
#[derive(Clone, Debug)]
pub struct VensimComment {
    pub uid: i32,
    pub text: String,
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    /// If true, the actual text content was on the next line (scratch_name).
    /// This is set when bits & (1 << 2) is true.
    pub scratch_name: bool,
}

/// A connector element in the view (type 1).
/// Connectors represent causal links between variables.
#[derive(Clone, Debug)]
pub struct VensimConnector {
    pub uid: i32,
    pub from_uid: i32,
    pub to_uid: i32,
    /// Polarity: Some('+') for positive, Some('-') for negative, None if unspecified.
    pub polarity: Option<char>,
    /// Control point for curved connectors. (0, 0) indicates a straight line.
    pub control_point: (i32, i32),
}

/// A parsed view element.
#[derive(Clone, Debug)]
pub enum VensimElement {
    Variable(VensimVariable),
    Valve(VensimValve),
    Comment(VensimComment),
    Connector(VensimConnector),
}

impl VensimElement {
    /// Get the UID of this element.
    pub fn uid(&self) -> i32 {
        match self {
            VensimElement::Variable(v) => v.uid,
            VensimElement::Valve(v) => v.uid,
            VensimElement::Comment(c) => c.uid,
            VensimElement::Connector(c) => c.uid,
        }
    }

    /// Get the x coordinate of this element.
    pub fn x(&self) -> i32 {
        match self {
            VensimElement::Variable(v) => v.x,
            VensimElement::Valve(v) => v.x,
            VensimElement::Comment(c) => c.x,
            VensimElement::Connector(c) => c.control_point.0,
        }
    }

    /// Get the y coordinate of this element.
    pub fn y(&self) -> i32 {
        match self {
            VensimElement::Variable(v) => v.y,
            VensimElement::Valve(v) => v.y,
            VensimElement::Comment(c) => c.y,
            VensimElement::Connector(c) => c.control_point.1,
        }
    }

    /// Get the width of this element (0 for connectors).
    pub fn width(&self) -> i32 {
        match self {
            VensimElement::Variable(v) => v.width,
            VensimElement::Valve(v) => v.width,
            VensimElement::Comment(c) => c.width,
            VensimElement::Connector(_) => 0,
        }
    }

    /// Get the height of this element (0 for connectors).
    pub fn height(&self) -> i32 {
        match self {
            VensimElement::Variable(v) => v.height,
            VensimElement::Valve(v) => v.height,
            VensimElement::Comment(c) => c.height,
            VensimElement::Connector(_) => 0,
        }
    }

    /// Set the x coordinate of this element.
    pub fn set_x(&mut self, x: i32) {
        match self {
            VensimElement::Variable(v) => v.x = x,
            VensimElement::Valve(v) => v.x = x,
            VensimElement::Comment(c) => c.x = x,
            VensimElement::Connector(c) => c.control_point.0 = x,
        }
    }

    /// Set the y coordinate of this element.
    pub fn set_y(&mut self, y: i32) {
        match self {
            VensimElement::Variable(v) => v.y = y,
            VensimElement::Valve(v) => v.y = y,
            VensimElement::Comment(c) => c.y = y,
            VensimElement::Connector(c) => c.control_point.1 = y,
        }
    }

    /// Set the width of this element (no-op for connectors).
    pub fn set_width(&mut self, width: i32) {
        match self {
            VensimElement::Variable(v) => v.width = width,
            VensimElement::Valve(v) => v.width = width,
            VensimElement::Comment(c) => c.width = width,
            VensimElement::Connector(_) => {}
        }
    }

    /// Set the height of this element (no-op for connectors).
    pub fn set_height(&mut self, height: i32) {
        match self {
            VensimElement::Variable(v) => v.height = height,
            VensimElement::Valve(v) => v.height = height,
            VensimElement::Comment(c) => c.height = height,
            VensimElement::Connector(_) => {}
        }
    }
}

/// A parsed Vensim view with all its elements.
#[derive(Clone, Debug)]
pub struct VensimView {
    pub header: ViewHeader,
    /// Elements indexed by UID. None entries represent missing UIDs.
    pub elements: Vec<Option<VensimElement>>,
    /// UID offset for multi-view composition.
    pub uid_offset: i32,
}

impl VensimView {
    /// Create a new view with the given header.
    pub fn new(header: ViewHeader) -> Self {
        VensimView {
            header,
            elements: Vec::new(),
            uid_offset: 0,
        }
    }

    /// Get the title of this view.
    pub fn title(&self) -> &str {
        &self.header.title
    }

    /// Set the title of this view.
    pub fn set_title(&mut self, title: String) {
        self.header.title = title;
    }

    /// Get an element by UID.
    pub fn get(&self, uid: i32) -> Option<&VensimElement> {
        if uid < 0 {
            return None;
        }
        self.elements.get(uid as usize).and_then(|e| e.as_ref())
    }

    /// Get a mutable reference to an element by UID.
    pub fn get_mut(&mut self, uid: i32) -> Option<&mut VensimElement> {
        if uid < 0 {
            return None;
        }
        self.elements.get_mut(uid as usize).and_then(|e| e.as_mut())
    }

    /// Insert an element at the given UID.
    /// Expands the elements vector if necessary.
    pub fn insert(&mut self, uid: i32, element: VensimElement) {
        if uid < 0 {
            return;
        }
        let idx = uid as usize;
        if idx >= self.elements.len() {
            self.elements.resize(idx + 25, None);
        }
        self.elements[idx] = Some(element);
    }

    /// Iterate over all present elements.
    pub fn iter(&self) -> impl Iterator<Item = &VensimElement> {
        self.elements.iter().filter_map(|e| e.as_ref())
    }

    /// Iterate over all present elements with their UIDs.
    pub fn iter_with_uids(&self) -> impl Iterator<Item = (i32, &VensimElement)> {
        self.elements
            .iter()
            .enumerate()
            .filter_map(|(uid, e)| e.as_ref().map(|elem| (uid as i32, elem)))
    }

    /// Get the maximum x coordinate in this view.
    pub fn max_x(&self, default: i32) -> i32 {
        self.iter().map(|e| e.x()).max().unwrap_or(default)
    }

    /// Get the maximum y coordinate in this view.
    pub fn max_y(&self, default: i32) -> i32 {
        self.iter().map(|e| e.y()).max().unwrap_or(default)
    }

    /// Get the minimum x coordinate in this view.
    pub fn min_x(&self) -> Option<i32> {
        self.iter().map(|e| e.x()).min()
    }

    /// Get the minimum y coordinate in this view.
    pub fn min_y(&self) -> Option<i32> {
        self.iter().map(|e| e.y()).min()
    }
}

/// Errors that can occur during view parsing.
#[derive(Debug)]
pub enum ViewError {
    /// Invalid or unrecognized version string.
    InvalidVersion(String),
    /// Parse error at a specific line.
    ParseError { line: usize, message: String },
    /// Referenced element UID not found.
    MissingElement { uid: i32 },
    /// Unexpected end of input.
    UnexpectedEndOfInput,
}

impl std::fmt::Display for ViewError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ViewError::InvalidVersion(v) => {
                write!(f, "Invalid view version: {}", v)
            }
            ViewError::ParseError { line, message } => {
                write!(f, "Parse error at line {}: {}", line, message)
            }
            ViewError::MissingElement { uid } => {
                write!(f, "Missing element with UID {}", uid)
            }
            ViewError::UnexpectedEndOfInput => {
                write!(f, "Unexpected end of input")
            }
        }
    }
}

impl std::error::Error for ViewError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vensim_view_insert_and_get() {
        let header = ViewHeader {
            version: ViewVersion::V300,
            title: "Test View".to_string(),
        };
        let mut view = VensimView::new(header);

        let var = VensimVariable {
            uid: 5,
            name: "test".to_string(),
            x: 100,
            y: 200,
            width: 40,
            height: 20,
            attached: false,
            is_ghost: false,
        };

        view.insert(5, VensimElement::Variable(var));

        assert!(view.get(5).is_some());
        assert!(view.get(0).is_none());
        assert!(view.get(10).is_none());

        if let Some(VensimElement::Variable(v)) = view.get(5) {
            assert_eq!(v.name, "test");
            assert_eq!(v.x, 100);
        } else {
            panic!("Expected Variable element");
        }
    }

    #[test]
    fn test_vensim_view_max_min() {
        let header = ViewHeader {
            version: ViewVersion::V300,
            title: "Test".to_string(),
        };
        let mut view = VensimView::new(header);

        view.insert(
            1,
            VensimElement::Variable(VensimVariable {
                uid: 1,
                name: "a".to_string(),
                x: 50,
                y: 100,
                width: 40,
                height: 20,
                attached: false,
                is_ghost: false,
            }),
        );
        view.insert(
            2,
            VensimElement::Variable(VensimVariable {
                uid: 2,
                name: "b".to_string(),
                x: 200,
                y: 50,
                width: 40,
                height: 20,
                attached: false,
                is_ghost: false,
            }),
        );

        assert_eq!(view.min_x(), Some(50));
        assert_eq!(view.max_x(0), 200);
        assert_eq!(view.min_y(), Some(50));
        assert_eq!(view.max_y(0), 100);
    }

    #[test]
    fn test_element_accessors() {
        let var = VensimElement::Variable(VensimVariable {
            uid: 1,
            name: "test".to_string(),
            x: 100,
            y: 200,
            width: 40,
            height: 20,
            attached: true,
            is_ghost: false,
        });

        assert_eq!(var.uid(), 1);
        assert_eq!(var.x(), 100);
        assert_eq!(var.y(), 200);
        assert_eq!(var.width(), 40);
        assert_eq!(var.height(), 20);
    }
}
