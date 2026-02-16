// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Element parsing for Vensim view/sketch format.
//!
//! This module handles parsing individual element types from sketch lines,
//! using atoi-style integer parsing to match xmutil behavior exactly.

use super::types::{
    VensimComment, VensimConnector, VensimElement, VensimValve, VensimVariable, ViewError,
};

/// Parse an integer field using atoi-style semantics.
///
/// Finds the next comma, parses the substring as an integer.
/// Like C's atoi: stops at first non-numeric character after optional sign,
/// returns 0 for empty or non-numeric input.
///
/// Returns (parsed_value, remaining_string_after_comma).
pub fn parse_int_field(s: &str) -> (i32, &str) {
    // Find the comma
    let end = s.find(',').unwrap_or(s.len());
    let field = &s[..end];

    // atoi-style parsing: skip leading whitespace, parse sign and digits
    let trimmed = field.trim_start();
    let val = parse_atoi(trimmed);

    // Return remaining string after comma (if any)
    let rest = if end < s.len() { &s[end + 1..] } else { "" };
    (val, rest)
}

/// Parse an integer using atoi semantics.
///
/// Parses optional sign followed by digits, stops at first non-digit.
/// Returns 0 for empty or non-numeric input.
fn parse_atoi(s: &str) -> i32 {
    if s.is_empty() {
        return 0;
    }

    let bytes = s.as_bytes();
    let mut i = 0;
    let mut negative = false;

    // Handle sign
    if bytes[i] == b'-' {
        negative = true;
        i += 1;
    } else if bytes[i] == b'+' {
        i += 1;
    }

    // Parse digits
    let mut result: i32 = 0;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        result = result
            .saturating_mul(10)
            .saturating_add((bytes[i] - b'0') as i32);
        i += 1;
    }

    if negative { -result } else { result }
}

/// Parse a string field, handling both quoted and unquoted strings.
///
/// If the string starts with `"`, scans until unescaped closing `"`.
/// Escaped quotes (`\"`) inside quoted strings are handled.
/// If not quoted, scans until comma.
///
/// Returns (parsed_string, remaining_string_after_delimiter).
pub fn parse_string_field(s: &str) -> (String, &str) {
    if s.starts_with('"') {
        // Quoted string: find unescaped closing quote
        let mut result = String::new();
        let bytes = s.as_bytes();
        let mut i = 1; // Skip opening quote

        while i < bytes.len() {
            if bytes[i] == b'"' {
                // Found closing quote
                i += 1;
                // Skip the comma after the closing quote if present
                if i < bytes.len() && bytes[i] == b',' {
                    i += 1;
                }
                return (result, &s[i..]);
            } else if bytes[i] == b'\\' && i + 1 < bytes.len() && bytes[i + 1] == b'"' {
                // Escaped quote
                result.push('"');
                i += 2;
            } else {
                result.push(bytes[i] as char);
                i += 1;
            }
        }

        // Unterminated quoted string - return what we have
        (result, "")
    } else {
        // Unquoted string: find comma
        let end = s.find(',').unwrap_or(s.len());
        let field = &s[..end];
        let rest = if end < s.len() { &s[end + 1..] } else { "" };
        (field.to_string(), rest)
    }
}

/// Parse a variable element (type 10).
///
/// Format after `10,uid,`: `name,x,y,width,height,shape,bits,...`
///
/// - shape bit 5 (0x20): attached to valve (flow indicator)
/// - bits bit 0: if 0 = ghost, if 1 = primary definition
pub fn parse_variable(uid: i32, fields: &str) -> Result<VensimVariable, ViewError> {
    let (name, rest) = parse_string_field(fields);
    let (x, rest) = parse_int_field(rest);
    let (y, rest) = parse_int_field(rest);
    let (width, rest) = parse_int_field(rest);
    let (height, rest) = parse_int_field(rest);
    let (shape, rest) = parse_int_field(rest);
    let (bits, _) = parse_int_field(rest);

    let attached = (shape & (1 << 5)) != 0;
    let is_ghost = (bits & 1) == 0;

    Ok(VensimVariable {
        uid,
        name,
        x,
        y,
        width,
        height,
        attached,
        is_ghost,
    })
}

/// Parse a valve element (type 11).
///
/// Format after `11,uid,`: `name,x,y,width,height,shape,...`
///
/// - shape bit 5 (0x20): attached to flow
pub fn parse_valve(uid: i32, fields: &str) -> Result<VensimValve, ViewError> {
    let (name, rest) = parse_string_field(fields);
    let (x, rest) = parse_int_field(rest);
    let (y, rest) = parse_int_field(rest);
    let (width, rest) = parse_int_field(rest);
    let (height, rest) = parse_int_field(rest);
    let (shape, _) = parse_int_field(rest);

    let attached = (shape & (1 << 5)) != 0;

    Ok(VensimValve {
        uid,
        name,
        x,
        y,
        width,
        height,
        attached,
    })
}

/// Parse a comment element (type 12).
///
/// Format after `12,uid,`: `text,x,y,width,height,shape,bits,...`
///
/// - bits bit 2 (0x04): scratch_name - actual text is on next line
///
/// Returns (comment, scratch_name_flag). If scratch_name_flag is true,
/// the caller must read the next line and set it as the comment text.
pub fn parse_comment(uid: i32, fields: &str) -> Result<(VensimComment, bool), ViewError> {
    let (text, rest) = parse_string_field(fields);
    let (x, rest) = parse_int_field(rest);
    let (y, rest) = parse_int_field(rest);
    let (width, rest) = parse_int_field(rest);
    let (height, rest) = parse_int_field(rest);
    let (_shape, rest) = parse_int_field(rest);
    let (bits, _) = parse_int_field(rest);

    let scratch_name = (bits & (1 << 2)) != 0;

    Ok((
        VensimComment {
            uid,
            text,
            x,
            y,
            width,
            height,
            scratch_name,
        },
        scratch_name,
    ))
}

/// Parse a connector element (type 1).
///
/// Format after `1,uid,`: `from,to,ignore,ignore,polarity,ignore×6,npoints|(x,y)|...`
///
/// Polarity mapping:
/// - 'S'/'s' -> '+' (same direction, set letter polarity flag)
/// - 'O'/'0' -> '-' (opposite direction, set letter polarity flag)
/// - '+'/'-' -> as-is
/// - other -> None
pub fn parse_connector(uid: i32, fields: &str) -> Result<VensimConnector, ViewError> {
    let (from_uid, rest) = parse_int_field(fields);
    let (to_uid, rest) = parse_int_field(rest);
    let (_ignore1, rest) = parse_string_field(rest);
    let (_ignore2, rest) = parse_string_field(rest);
    let (polarity_ascii, rest) = parse_int_field(rest);

    // Skip 6 ignored fields
    let (_, rest) = parse_string_field(rest);
    let (_, rest) = parse_string_field(rest);
    let (_, rest) = parse_string_field(rest);
    let (_, rest) = parse_string_field(rest);
    let (_, rest) = parse_string_field(rest);
    let (_, rest) = parse_string_field(rest);

    // Parse control point from "npoints|(x,y)|" format
    let control_point = parse_points(rest);

    let (polarity, letter_polarity) = parse_polarity(polarity_ascii);

    Ok(VensimConnector {
        uid,
        from_uid,
        to_uid,
        polarity,
        letter_polarity,
        control_point,
    })
}

/// Parse polarity from ASCII value.
///
/// Returns (normalized_polarity, is_letter_polarity):
/// - 'S'/'s' (83/115) -> (Some('+'), true)
/// - 'O'/'0' (79/48) -> (Some('-'), true)
/// - '+' (43) -> (Some('+'), false)
/// - '-' (45) -> (Some('-'), false)
/// - other -> (None, false)
fn parse_polarity(ascii_val: i32) -> (Option<char>, bool) {
    match ascii_val {
        83 | 115 => (Some('+'), true), // 'S' or 's'
        79 | 48 => (Some('-'), true),  // 'O' or '0'
        43 => (Some('+'), false),      // '+'
        45 => (Some('-'), false),      // '-'
        _ => (None, false),
    }
}

/// Parse control points from "npoints|(x,y)|" format.
///
/// Returns (x, y) of the first point, or (0, 0) if not found.
fn parse_points(s: &str) -> (i32, i32) {
    // Format: "npoints|(x,y)|"
    // We use sscanf-style parsing: look for the pattern
    let mut x = 0;
    let mut y = 0;

    // Find "|(" pattern
    if let Some(start) = s.find("|(") {
        let after_paren = &s[start + 2..];
        // Find comma
        if let Some(comma) = after_paren.find(',') {
            let x_str = &after_paren[..comma];
            x = parse_atoi(x_str);

            let after_comma = &after_paren[comma + 1..];
            // Find closing paren
            if let Some(end) = after_comma.find(')') {
                let y_str = &after_comma[..end];
                y = parse_atoi(y_str);
            }
        }
    }

    (x, y)
}

/// Parse an element line and return the parsed element.
///
/// The line should start with `type,uid,...` where type is:
/// - 1: Connector
/// - 10: Variable
/// - 11: Valve
/// - 12: Comment
/// - 30: Ignored (returns None)
///
/// Returns (element, scratch_name_flag). If scratch_name_flag is true,
/// the caller must read the next line to get the comment text.
pub fn parse_element_line(line: &str) -> Result<Option<(VensimElement, bool)>, ViewError> {
    let (element_type, rest) = parse_int_field(line);
    let (uid, rest) = parse_int_field(rest);

    if element_type < 0 || uid < 0 {
        return Ok(None);
    }

    match element_type {
        1 => {
            let connector = parse_connector(uid, rest)?;
            Ok(Some((VensimElement::Connector(connector), false)))
        }
        10 => {
            let variable = parse_variable(uid, rest)?;
            Ok(Some((VensimElement::Variable(variable), false)))
        }
        11 => {
            let valve = parse_valve(uid, rest)?;
            Ok(Some((VensimElement::Valve(valve), false)))
        }
        12 => {
            let (comment, scratch_name) = parse_comment(uid, rest)?;
            Ok(Some((VensimElement::Comment(comment), scratch_name)))
        }
        30 => Ok(None), // Ignored element type
        _ => Ok(None),  // Unknown element type - ignore
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_int_field_simple() {
        let (val, rest) = parse_int_field("123,456");
        assert_eq!(val, 123);
        assert_eq!(rest, "456");
    }

    #[test]
    fn test_parse_int_field_negative() {
        let (val, rest) = parse_int_field("-42,next");
        assert_eq!(val, -42);
        assert_eq!(rest, "next");
    }

    #[test]
    fn test_parse_int_field_stops_at_non_digit() {
        // This is the critical xmutil behavior: "-1--1--1" parses as -1
        let (val, rest) = parse_int_field("-1--1--1,next");
        assert_eq!(val, -1);
        assert_eq!(rest, "next");
    }

    #[test]
    fn test_parse_int_field_empty() {
        let (val, rest) = parse_int_field(",next");
        assert_eq!(val, 0);
        assert_eq!(rest, "next");
    }

    #[test]
    fn test_parse_int_field_no_comma() {
        let (val, rest) = parse_int_field("789");
        assert_eq!(val, 789);
        assert_eq!(rest, "");
    }

    #[test]
    fn test_parse_string_field_unquoted() {
        let (val, rest) = parse_string_field("hello,world");
        assert_eq!(val, "hello");
        assert_eq!(rest, "world");
    }

    #[test]
    fn test_parse_string_field_quoted() {
        let (val, rest) = parse_string_field("\"hello, world\",next");
        assert_eq!(val, "hello, world");
        assert_eq!(rest, "next");
    }

    #[test]
    fn test_parse_string_field_escaped_quote() {
        let (val, rest) = parse_string_field("\"say \\\"hello\\\"\",next");
        assert_eq!(val, "say \"hello\"");
        assert_eq!(rest, "next");
    }

    #[test]
    fn test_parse_variable() {
        // From SIR.mdl: 10,1,Susceptible Population S,162,192,40,20,3,3,0,0,0,0,0,0
        let line = "Susceptible Population S,162,192,40,20,3,3,0,0,0,0,0,0";
        let var = parse_variable(1, line).unwrap();

        assert_eq!(var.uid, 1);
        assert_eq!(var.name, "Susceptible Population S");
        assert_eq!(var.x, 162);
        assert_eq!(var.y, 192);
        assert_eq!(var.width, 40);
        assert_eq!(var.height, 20);
        assert!(!var.attached); // shape=3, bit 5 not set
        assert!(!var.is_ghost); // bits=3, bit 0 is set
    }

    #[test]
    fn test_parse_variable_attached() {
        // Flow variable attached to valve: shape has bit 5 set (32)
        let line = "Infection Rate,295,228,40,29,40,3,0,0,-1,0,0,0";
        let var = parse_variable(6, line).unwrap();

        assert_eq!(var.name, "Infection Rate");
        assert!(var.attached); // shape=40 has bit 5 set
        assert!(!var.is_ghost);
    }

    #[test]
    fn test_parse_variable_ghost() {
        // Ghost variable: bits & 1 == 0
        let line =
            "Contact Rate c,249,523,40,20,8,2,0,3,-1,0,0,0,128-128-128,0-0-0,|12|B|128-128-128";
        let var = parse_variable(27, line).unwrap();

        assert_eq!(var.name, "Contact Rate c");
        assert!(var.is_ghost); // bits=2, bit 0 not set
    }

    #[test]
    fn test_parse_valve() {
        // From SIR.mdl: 11,5,444,295,191,6,8,34,3,0,0,1,0,0,0
        let line = "444,295,191,6,8,34,3,0,0,1,0,0,0";
        let valve = parse_valve(5, line).unwrap();

        assert_eq!(valve.uid, 5);
        assert_eq!(valve.name, "444");
        assert_eq!(valve.x, 295);
        assert_eq!(valve.y, 191);
        assert!(valve.attached); // shape=34 has bit 5 set
    }

    #[test]
    fn test_parse_comment_scratch_name() {
        // From SIR.mdl: 12,13,0,232,218,15,15,5,4,0,0,-1,0,0,0
        // bits=4 means scratch_name is set (bit 2)
        let line = "0,232,218,15,15,5,4,0,0,-1,0,0,0";
        let (comment, scratch_name) = parse_comment(13, line).unwrap();

        assert_eq!(comment.uid, 13);
        assert_eq!(comment.text, "0");
        assert_eq!(comment.x, 232);
        assert_eq!(comment.y, 218);
        assert!(scratch_name);
        assert!(comment.scratch_name);
    }

    #[test]
    fn test_parse_comment_no_scratch_name() {
        // bits=0 means scratch_name not set
        let line = "Some text,100,100,50,50,0,0,0,0,-1,0,0,0";
        let (comment, scratch_name) = parse_comment(1, line).unwrap();

        assert_eq!(comment.text, "Some text");
        assert!(!scratch_name);
        assert!(!comment.scratch_name);
    }

    #[test]
    fn test_parse_connector() {
        // From SIR.mdl: 1,3,5,2,4,0,0,22,0,0,0,-1--1--1,,1|(344,191)|
        let line = "5,2,4,0,0,22,0,0,0,-1--1--1,,1|(344,191)|";
        let conn = parse_connector(3, line).unwrap();

        assert_eq!(conn.uid, 3);
        assert_eq!(conn.from_uid, 5);
        assert_eq!(conn.to_uid, 2);
        assert_eq!(conn.control_point, (344, 191));
    }

    #[test]
    fn test_parse_connector_with_polarity_s() {
        // Polarity 'S' (83) -> '+', letter polarity
        let line = "1,2,x,y,83,a,b,c,d,e,f,1|(100,200)|";
        let conn = parse_connector(1, line).unwrap();

        assert_eq!(conn.polarity, Some('+'));
        assert!(conn.letter_polarity);
    }

    #[test]
    fn test_parse_connector_with_polarity_o() {
        // Polarity 'O' (79) -> '-', letter polarity
        let line = "1,2,x,y,79,a,b,c,d,e,f,1|(100,200)|";
        let conn = parse_connector(1, line).unwrap();

        assert_eq!(conn.polarity, Some('-'));
        assert!(conn.letter_polarity);
    }

    #[test]
    fn test_parse_connector_with_polarity_zero() {
        // Polarity '0' (48) -> '-', letter polarity
        let line = "1,2,x,y,48,a,b,c,d,e,f,1|(100,200)|";
        let conn = parse_connector(1, line).unwrap();

        assert_eq!(conn.polarity, Some('-'));
        assert!(conn.letter_polarity);
    }

    #[test]
    fn test_parse_connector_with_symbol_polarity() {
        // Polarity '+' (43) -> '+', NOT letter polarity
        let line = "1,2,x,y,43,a,b,c,d,e,f,1|(100,200)|";
        let conn = parse_connector(1, line).unwrap();

        assert_eq!(conn.polarity, Some('+'));
        assert!(!conn.letter_polarity);

        // Polarity '-' (45) -> '-', NOT letter polarity
        let line = "1,2,x,y,45,a,b,c,d,e,f,1|(100,200)|";
        let conn = parse_connector(1, line).unwrap();

        assert_eq!(conn.polarity, Some('-'));
        assert!(!conn.letter_polarity);
    }

    #[test]
    fn test_parse_points() {
        assert_eq!(parse_points("1|(100,200)|"), (100, 200));
        assert_eq!(parse_points("2|(50,75)|(100,200)|"), (50, 75));
        assert_eq!(parse_points("no points here"), (0, 0));
        assert_eq!(parse_points(""), (0, 0));
    }

    #[test]
    fn test_parse_element_line_variable() {
        let line = "10,1,Test Var,100,200,40,20,3,3,0,0,0,0,0,0";
        let result = parse_element_line(line).unwrap();

        assert!(result.is_some());
        let (elem, scratch_name) = result.unwrap();
        assert!(!scratch_name);

        if let VensimElement::Variable(v) = elem {
            assert_eq!(v.name, "Test Var");
        } else {
            panic!("Expected Variable");
        }
    }

    #[test]
    fn test_parse_element_line_ignored() {
        // Type 30 should be ignored
        let line = "30,1,some,data,here";
        let result = parse_element_line(line).unwrap();
        assert!(result.is_none());
    }
}
