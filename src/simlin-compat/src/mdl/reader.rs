// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! EquationReader: Driver for the LALRPOP parser that handles comment capture
//! and macro mode assembly.
//!
//! The LALRPOP parser returns when it sees the end of the equation+units section.
//! This reader then captures the comment text (raw text between second `~` and `|`)
//! and assembles the final `FullEquation`.

use std::borrow::Cow;

use crate::mdl::ast::{
    Equation, Expr, FullEquation, Group, Loc, MacroDef, MdlItem, SectionEnd, Units,
};
use crate::mdl::normalizer::{NormalizerError, Token, TokenNormalizer};
use crate::mdl::parser;

/// Error from the equation reader.
#[derive(Clone, Debug)]
pub enum ReaderError {
    /// Parser error
    Parse(String),
    /// Normalizer error
    Normalizer(NormalizerError),
    /// Macro end without matching start
    UnmatchedMacroEnd,
    /// EOF inside macro
    EofInsideMacro,
}

impl From<NormalizerError> for ReaderError {
    fn from(e: NormalizerError) -> Self {
        ReaderError::Normalizer(e)
    }
}

/// Tracks macro definition state.
enum MacroState<'input> {
    /// Not currently inside a macro.
    Normal,
    /// Inside a macro definition.
    InMacro {
        name: Cow<'input, str>,
        args: Vec<Expr<'input>>,
        equations: Vec<FullEquation<'input>>,
        loc: Loc,
    },
}

/// Result of scanning for a comment terminator.
enum CommentTerminator {
    /// Found `|` at given byte offset from scan start
    Pipe(usize),
    /// Found EqEnd marker (`\\\---///` or `///---\\\`) at given byte offset
    EqEnd(usize),
    /// Reached end of input
    Eof,
}

/// EquationReader wraps the LALRPOP parser to handle comment capture and macro assembly.
///
/// Comments in Vensim are raw text between the second `~` and `|`. The LALRPOP parser
/// can't easily capture this (it's not tokenized), so this reader scans raw source text
/// for comment content rather than tokenizing it.
pub struct EquationReader<'input> {
    source: &'input str,
    /// Current byte position in source (for creating normalizers)
    position: usize,
    /// Macro state
    macro_state: MacroState<'input>,
    /// Whether we've reached the end of the equations section
    finished: bool,
}

impl<'input> EquationReader<'input> {
    /// Create a new equation reader for the given MDL source.
    pub fn new(source: &'input str) -> Self {
        EquationReader {
            source,
            position: 0,
            macro_state: MacroState::Normal,
            finished: false,
        }
    }

    /// Returns remaining unparsed source after the equations section.
    ///
    /// This is useful for parsing post-equation content like views and settings.
    /// The returned slice starts immediately after the last parsed position
    /// (typically after an EqEnd marker).
    pub fn remaining(&self) -> &'input str {
        &self.source[self.position..]
    }

    /// Scan for comment terminator: `|` or EqEnd marker.
    ///
    /// xmutil treats comments as raw text and stops on either `|` or the EqEnd marker.
    /// This ensures that special characters like `$`, `@`, `#` in comments don't cause
    /// lexer/normalizer errors.
    fn find_comment_terminator(text: &str) -> CommentTerminator {
        let bytes = text.as_bytes();
        let len = bytes.len();
        let mut i = 0;

        while i < len {
            let c = bytes[i];

            // Check for pipe
            if c == b'|' {
                return CommentTerminator::Pipe(i);
            }

            // Check for EqEnd markers: \\\---/// or ///---\\\
            if c == b'\\' && i + 9 <= len && &bytes[i..i + 9] == b"\\\\\\---///" {
                return CommentTerminator::EqEnd(i);
            }
            if c == b'/' && i + 9 <= len && &bytes[i..i + 9] == b"///---\\\\\\" {
                return CommentTerminator::EqEnd(i);
            }

            i += 1;
        }

        CommentTerminator::Eof
    }

    /// Read the next top-level item from the MDL file.
    ///
    /// Returns `None` when the end of the equations section is reached.
    pub fn next_item(&mut self) -> Option<Result<MdlItem<'input>, ReaderError>> {
        if self.finished {
            return None;
        }

        // Skip past already-processed source
        if self.position >= self.source.len() {
            self.finished = true;
            if let MacroState::InMacro { .. } = &self.macro_state {
                return Some(Err(ReaderError::EofInsideMacro));
            }
            return None;
        }

        // Create a fresh normalizer from the current position
        let remaining = &self.source[self.position..];
        let mut normalizer = TokenNormalizer::with_offset(remaining, self.position);

        // Collect tokens into a buffer to check for EOF before parsing
        let mut tokens: Vec<Result<(usize, Token<'input>, usize), NormalizerError>> = Vec::new();
        let mut tilde_count = 0;
        let mut last_end = self.position;
        // Track if we're collecting a :MACRO: header (need to read through closing paren)
        let mut in_macro_header = false;
        let mut macro_paren_depth = 0;

        loop {
            match normalizer.next() {
                Some(Ok((start, tok, end))) => {
                    last_end = end;

                    // Check for special markers that terminate token collection
                    // These markers should be parsed in isolation (or with their arguments)
                    if tokens.is_empty() {
                        match &tok {
                            Token::EqEnd | Token::EndOfMacro | Token::GroupStar(_) => {
                                // These markers are complete on their own
                                tokens.push(Ok((start, tok, end)));
                                break;
                            }
                            Token::Macro => {
                                // :MACRO: needs to collect through closing )
                                in_macro_header = true;
                                tokens.push(Ok((start, tok, end)));
                                continue;
                            }
                            _ => {}
                        }
                    }

                    // Track tilde count
                    if matches!(tok, Token::Tilde) {
                        tilde_count += 1;
                    }

                    // Track parentheses in macro header
                    if in_macro_header {
                        match &tok {
                            Token::LParen => macro_paren_depth += 1,
                            Token::RParen => {
                                macro_paren_depth -= 1;
                                if macro_paren_depth == 0 {
                                    // Finished macro header
                                    tokens.push(Ok((start, tok, end)));
                                    break;
                                }
                            }
                            _ => {}
                        }
                        tokens.push(Ok((start, tok, end)));
                        continue;
                    }

                    let is_second_tilde = matches!(tok, Token::Tilde) && tilde_count >= 2;
                    let is_pipe = matches!(tok, Token::Pipe);

                    tokens.push(Ok((start, tok, end)));

                    // Stop after second tilde or pipe
                    if is_second_tilde || is_pipe {
                        break;
                    }
                }
                Some(Err(e)) => {
                    return Some(Err(ReaderError::Normalizer(e)));
                }
                None => {
                    // EOF reached
                    if tokens.is_empty() {
                        // No tokens at all - true EOF
                        self.finished = true;
                        if let MacroState::InMacro { .. } = &self.macro_state {
                            return Some(Err(ReaderError::EofInsideMacro));
                        }
                        return None;
                    }
                    // Some tokens but then EOF - let parser handle it
                    break;
                }
            }
        }

        // Update position to after the collected tokens
        self.position = last_end;

        // Parse the collected tokens
        let result = parser::FullEqWithUnitsParser::new().parse(tokens);

        match result {
            Ok((eq, units, section_end)) => {
                self.handle_parse_result(eq, units, section_end, last_end)
            }
            Err(e) => Some(Err(ReaderError::Parse(format!("{:?}", e)))),
        }
    }

    /// Handle the result from the parser.
    fn handle_parse_result(
        &mut self,
        eq: Equation<'input>,
        units: Option<Units<'input>>,
        section_end: SectionEnd<'input>,
        last_pos: usize,
    ) -> Option<Result<MdlItem<'input>, ReaderError>> {
        match section_end {
            SectionEnd::Tilde => {
                // Comment follows - capture it from raw source
                let (comment, supplementary) = self.capture_comment(last_pos);
                let full_eq = self.make_full_equation(eq, units, comment, supplementary);
                self.handle_equation(full_eq)
            }
            SectionEnd::Pipe => {
                // No comment
                let full_eq = self.make_full_equation(eq, units, None, false);
                self.handle_equation(full_eq)
            }
            SectionEnd::EqEnd(loc) => {
                self.finished = true;
                if let MacroState::InMacro { .. } = &self.macro_state {
                    return Some(Err(ReaderError::EofInsideMacro));
                }
                Some(Ok(MdlItem::EqEnd(loc)))
            }
            SectionEnd::GroupStar(name, loc) => Some(Ok(MdlItem::Group(Group { name, loc }))),
            SectionEnd::MacroStart(name, args, loc) => {
                // Start macro mode
                self.macro_state = MacroState::InMacro {
                    name,
                    args,
                    equations: Vec::new(),
                    loc,
                };
                // Continue reading to get the next item
                self.next_item()
            }
            SectionEnd::MacroEnd(end_loc) => {
                // End macro mode and emit the macro definition
                match std::mem::replace(&mut self.macro_state, MacroState::Normal) {
                    MacroState::InMacro {
                        name,
                        args,
                        equations,
                        loc,
                    } => {
                        let macro_def = MacroDef {
                            name,
                            args,
                            equations,
                            loc: Loc::merge(loc, end_loc),
                        };
                        Some(Ok(MdlItem::Macro(Box::new(macro_def))))
                    }
                    MacroState::Normal => Some(Err(ReaderError::UnmatchedMacroEnd)),
                }
            }
        }
    }

    /// Handle an equation (either add to macro or emit as top-level item).
    fn handle_equation(
        &mut self,
        full_eq: FullEquation<'input>,
    ) -> Option<Result<MdlItem<'input>, ReaderError>> {
        match &mut self.macro_state {
            MacroState::InMacro { equations, .. } => {
                // Add to current macro definition
                equations.push(full_eq);
                // Continue reading to get the next item
                self.next_item()
            }
            MacroState::Normal => {
                // Emit as top-level equation
                Some(Ok(MdlItem::Equation(Box::new(full_eq))))
            }
        }
    }

    /// Create a FullEquation from the parsed components.
    fn make_full_equation(
        &self,
        eq: Equation<'input>,
        units: Option<Units<'input>>,
        comment: Option<Cow<'input, str>>,
        supplementary: bool,
    ) -> FullEquation<'input> {
        let loc = self.equation_loc(&eq, &units);
        FullEquation {
            equation: eq,
            units,
            comment,
            supplementary,
            loc,
        }
    }

    /// Compute the location span for a full equation.
    fn equation_loc(&self, eq: &Equation<'_>, units: &Option<Units<'_>>) -> Loc {
        let eq_loc = match eq {
            Equation::Regular(lhs, _) => lhs.loc,
            Equation::EmptyRhs(lhs, _) => lhs.loc,
            Equation::Implicit(lhs) => lhs.loc,
            Equation::Lookup(lhs, _) => lhs.loc,
            Equation::WithLookup(lhs, _, _) => lhs.loc,
            Equation::Data(lhs, _) => lhs.loc,
            Equation::TabbedArray(lhs, _) => lhs.loc,
            Equation::NumberList(lhs, _) => lhs.loc,
            Equation::SubscriptDef(_, def) => def.loc,
            Equation::Equivalence(_, _, loc) => *loc,
        };

        if let Some(u) = units {
            Loc::merge(eq_loc, u.loc)
        } else {
            eq_loc
        }
    }

    /// Capture the comment text from the given position until `|` or EqEnd.
    ///
    /// This scans raw source text without tokenizing, so special characters like
    /// `$`, `@`, `#` in comments don't cause errors. Updates self.position to
    /// the appropriate location for the next parse.
    ///
    /// Returns (comment, supplementary) where supplementary is true if the
    /// `:SUP` or `:SUPPLEMENTARY` flag was found.
    fn capture_comment(&mut self, start_pos: usize) -> (Option<Cow<'input, str>>, bool) {
        let remaining = &self.source[start_pos..];

        let end_offset = match Self::find_comment_terminator(remaining) {
            CommentTerminator::Pipe(offset) => {
                // Update position to after the `|`
                self.position = start_pos + offset + 1;
                offset
            }
            CommentTerminator::EqEnd(offset) => {
                // EqEnd found - position at the marker so next_item will see it
                // Don't set finished here - let next_item handle the EqEnd normally
                self.position = start_pos + offset;
                offset
            }
            CommentTerminator::Eof => {
                // No terminator - use rest of input as comment
                self.position = self.source.len();
                remaining.len()
            }
        };

        // Extract the raw comment text
        let raw_comment = remaining[..end_offset].trim();

        // Check for supplementary flag (third ~ followed by :SUP or :SUPPLEMENTARY)
        let (comment, supplementary) = Self::extract_supplementary_flag(raw_comment);

        let comment = if comment.is_empty() {
            None
        } else {
            Some(Cow::Borrowed(comment))
        };

        (comment, supplementary)
    }

    /// Extract the `:SUP` or `:SUPPLEMENTARY` flag from comment text.
    ///
    /// MDL format: `doc text ~ :SUP` or `doc text ~ :SUPPLEMENTARY`
    /// The third tilde separates the documentation from the supplementary flag.
    ///
    /// Returns (remaining_comment, is_supplementary).
    fn extract_supplementary_flag(text: &str) -> (&str, bool) {
        // Look for a tilde that separates the comment from the supplementary flag
        if let Some(tilde_pos) = text.rfind('~') {
            let after_tilde = text[tilde_pos + 1..].trim();
            // Check for :SUP or :SUPPLEMENTARY (case-insensitive)
            let upper = after_tilde.to_uppercase();
            if upper == ":SUP" || upper == ":SUPPLEMENTARY" {
                let comment = text[..tilde_pos].trim();
                return (comment, true);
            }
        }
        (text, false)
    }
}

/// Iterator adapter for EquationReader.
impl<'input> Iterator for EquationReader<'input> {
    type Item = Result<MdlItem<'input>, ReaderError>;

    fn next(&mut self) -> Option<Self::Item> {
        self.next_item()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mdl::ast::{CallKind, InterpMode, UnitExpr};

    #[test]
    fn test_reader_simple_equation() {
        let input = "x = 5 ~ Units ~ A comment |";
        let mut reader = EquationReader::new(input);

        let item = reader.next_item();
        assert!(item.is_some(), "Expected Some, got None");
        match item {
            Some(Ok(MdlItem::Equation(eq))) => {
                assert!(matches!(eq.equation, Equation::Regular(_, _)));
                assert!(eq.comment.is_some(), "Expected comment");
                assert_eq!(eq.comment.as_ref().unwrap().as_ref(), "A comment");
            }
            Some(Ok(other)) => panic!("Expected Equation, got {:?}", other),
            Some(Err(e)) => panic!("Expected Ok, got Err({:?})", e),
            None => panic!("Expected Some, got None"),
        }
    }

    #[test]
    fn test_reader_no_comment() {
        let input = "x = 5 ~ Units |";
        let mut reader = EquationReader::new(input);

        let item = reader.next_item();
        assert!(item.is_some());
        match item {
            Some(Ok(MdlItem::Equation(eq))) => {
                assert!(eq.comment.is_none());
            }
            other => panic!("Expected equation, got {:?}", other),
        }
    }

    #[test]
    fn test_reader_no_units() {
        let input = "x = 5 ~ ~ A comment |";
        let mut reader = EquationReader::new(input);

        let item = reader.next_item();
        assert!(item.is_some());
        match item {
            Some(Ok(MdlItem::Equation(eq))) => {
                assert!(eq.units.is_none());
            }
            other => panic!("Expected equation, got {:?}", other),
        }
    }

    #[test]
    fn test_reader_multiple_equations() {
        let input = "x = 5 ~ ~ comment1 |\ny = 10 ~ Year ~ comment2 |";
        let mut reader = EquationReader::new(input);

        // First equation
        let item1 = reader.next_item();
        assert!(
            matches!(item1, Some(Ok(MdlItem::Equation(_)))),
            "item1: {:?}",
            item1
        );

        // Second equation
        let item2 = reader.next_item();
        assert!(
            matches!(item2, Some(Ok(MdlItem::Equation(_)))),
            "item2: {:?}",
            item2
        );

        // EOF
        let item3 = reader.next_item();
        assert!(item3.is_none(), "item3 should be None, got: {:?}", item3);
    }

    // ========================================================================
    // Expression parsing tests
    // ========================================================================

    #[test]
    fn test_expression_addition() {
        let input = "x = a + b ~ ~ |";
        let mut reader = EquationReader::new(input);
        let item = reader.next_item();
        assert!(
            matches!(item, Some(Ok(MdlItem::Equation(_)))),
            "Expected equation, got {:?}",
            item
        );
    }

    #[test]
    fn test_expression_multiplication() {
        let input = "x = a * b + c ~ ~ |";
        let mut reader = EquationReader::new(input);
        let item = reader.next_item();
        assert!(
            matches!(item, Some(Ok(MdlItem::Equation(_)))),
            "Expected equation, got {:?}",
            item
        );
    }

    #[test]
    fn test_expression_exponentiation() {
        let input = "x = a ^ 2 ~ ~ |";
        let mut reader = EquationReader::new(input);
        let item = reader.next_item();
        assert!(
            matches!(item, Some(Ok(MdlItem::Equation(_)))),
            "Expected equation, got {:?}",
            item
        );
    }

    #[test]
    fn test_expression_parentheses() {
        let input = "x = (a + b) * c ~ ~ |";
        let mut reader = EquationReader::new(input);
        let item = reader.next_item();
        assert!(
            matches!(item, Some(Ok(MdlItem::Equation(_)))),
            "Expected equation, got {:?}",
            item
        );
    }

    #[test]
    fn test_expression_comparison() {
        let input = "x = IF THEN ELSE(a > b, 1, 0) ~ ~ |";
        let mut reader = EquationReader::new(input);
        let item = reader.next_item();
        assert!(
            matches!(item, Some(Ok(MdlItem::Equation(_)))),
            "Expected equation, got {:?}",
            item
        );
    }

    #[test]
    fn test_expression_logical_operators() {
        let input = "x = a :AND: b :OR: c ~ ~ |";
        let mut reader = EquationReader::new(input);
        let item = reader.next_item();
        assert!(
            matches!(item, Some(Ok(MdlItem::Equation(_)))),
            "Expected equation, got {:?}",
            item
        );
    }

    #[test]
    fn test_expression_unary_negative() {
        let input = "x = -5 ~ ~ |";
        let mut reader = EquationReader::new(input);
        let item = reader.next_item();
        assert!(
            matches!(item, Some(Ok(MdlItem::Equation(_)))),
            "Expected equation, got {:?}",
            item
        );
    }

    #[test]
    fn test_expression_unary_not() {
        let input = "x = :NOT: flag ~ ~ |";
        let mut reader = EquationReader::new(input);
        let item = reader.next_item();
        assert!(
            matches!(item, Some(Ok(MdlItem::Equation(_)))),
            "Expected equation, got {:?}",
            item
        );
    }

    #[test]
    fn test_expression_na_constant() {
        let input = "x = :NA: ~ ~ |";
        let mut reader = EquationReader::new(input);
        let item = reader.next_item();
        match item {
            Some(Ok(MdlItem::Equation(eq))) => {
                assert!(matches!(eq.equation, Equation::Regular(_, Expr::Na(_))));
            }
            other => panic!("Expected equation with Na, got {:?}", other),
        }
    }

    // ========================================================================
    // Number list tests
    // ========================================================================

    #[test]
    fn test_number_list_comma_separated() {
        let input = "x = 1, 2, 3, 4 ~ ~ |";
        let mut reader = EquationReader::new(input);
        let item = reader.next_item();
        match item {
            Some(Ok(MdlItem::Equation(eq))) => {
                assert!(
                    matches!(eq.equation, Equation::NumberList(_, ref nums) if nums == &vec![1.0, 2.0, 3.0, 4.0]),
                    "Expected NumberList with [1,2,3,4], got {:?}",
                    eq.equation
                );
            }
            other => panic!("Expected equation, got {:?}", other),
        }
    }

    #[test]
    fn test_number_list_semicolon_separated() {
        let input = "x = 1; 2; 3 ~ ~ |";
        let mut reader = EquationReader::new(input);
        let item = reader.next_item();
        match item {
            Some(Ok(MdlItem::Equation(eq))) => {
                assert!(
                    matches!(eq.equation, Equation::NumberList(_, ref nums) if nums == &vec![1.0, 2.0, 3.0]),
                    "Expected NumberList with [1,2,3], got {:?}",
                    eq.equation
                );
            }
            other => panic!("Expected equation, got {:?}", other),
        }
    }

    #[test]
    fn test_number_list_trailing_semicolon() {
        let input = "x = 1, 2, 3; ~ ~ |";
        let mut reader = EquationReader::new(input);
        let item = reader.next_item();
        match item {
            Some(Ok(MdlItem::Equation(eq))) => {
                assert!(
                    matches!(eq.equation, Equation::NumberList(_, ref nums) if nums == &vec![1.0, 2.0, 3.0]),
                    "Expected NumberList with [1,2,3], got {:?}",
                    eq.equation
                );
            }
            other => panic!("Expected equation, got {:?}", other),
        }
    }

    #[test]
    fn test_number_list_with_negative() {
        let input = "x = 1, -2, 3 ~ ~ |";
        let mut reader = EquationReader::new(input);
        let item = reader.next_item();
        match item {
            Some(Ok(MdlItem::Equation(eq))) => {
                assert!(
                    matches!(eq.equation, Equation::NumberList(_, ref nums) if nums == &vec![1.0, -2.0, 3.0]),
                    "Expected NumberList with [1,-2,3], got {:?}",
                    eq.equation
                );
            }
            other => panic!("Expected equation, got {:?}", other),
        }
    }

    // ========================================================================
    // Function call tests
    // ========================================================================

    #[test]
    fn test_function_call_simple() {
        let input = "x = MAX(a, b) ~ ~ |";
        let mut reader = EquationReader::new(input);
        let item = reader.next_item();
        match item {
            Some(Ok(MdlItem::Equation(eq))) => {
                if let Equation::Regular(_, Expr::App(name, _, args, kind, _)) = &eq.equation {
                    // Function name is stored as-is from source (uppercase)
                    assert_eq!(name.as_ref(), "MAX");
                    assert_eq!(args.len(), 2);
                    assert!(matches!(kind, CallKind::Builtin));
                } else {
                    panic!("Expected function call, got {:?}", eq.equation);
                }
            }
            other => panic!("Expected equation, got {:?}", other),
        }
    }

    #[test]
    fn test_function_call_empty_args() {
        // Use RANDOM 0 1 which is a builtin that can have zero args
        let input = "x = RANDOM 0 1() ~ ~ |";
        let mut reader = EquationReader::new(input);
        let item = reader.next_item();
        match item {
            Some(Ok(MdlItem::Equation(eq))) => {
                if let Equation::Regular(_, Expr::App(name, _, args, kind, _)) = &eq.equation {
                    assert_eq!(name.as_ref(), "RANDOM 0 1");
                    assert_eq!(args.len(), 0);
                    assert!(matches!(kind, CallKind::Builtin));
                } else {
                    panic!("Expected function call, got {:?}", eq.equation);
                }
            }
            other => panic!("Expected equation, got {:?}", other),
        }
    }

    #[test]
    fn test_function_call_trailing_comma() {
        let input = "x = SMOOTH(input, delay,) ~ ~ |";
        let mut reader = EquationReader::new(input);
        let item = reader.next_item();
        match item {
            Some(Ok(MdlItem::Equation(eq))) => {
                if let Equation::Regular(_, Expr::App(_, _, args, _, _)) = &eq.equation {
                    assert_eq!(args.len(), 3);
                    assert!(
                        matches!(&args[2], Expr::Literal(lit, _) if lit.as_ref() == "?"),
                        "Expected trailing ? literal, got {:?}",
                        args[2]
                    );
                } else {
                    panic!("Expected function call, got {:?}", eq.equation);
                }
            }
            other => panic!("Expected equation, got {:?}", other),
        }
    }

    #[test]
    fn test_integ_function() {
        // Use underscores to avoid parsing issues with spaces in variable names
        let input = "Stock = INTEG(inflow_rate, initial_val) ~ Units ~ |";
        let mut reader = EquationReader::new(input);
        let item = reader.next_item();
        match item {
            Some(Ok(MdlItem::Equation(eq))) => {
                if let Equation::Regular(lhs, Expr::App(name, _, args, _, _)) = &eq.equation {
                    assert_eq!(lhs.name.as_ref(), "Stock");
                    // Function name stored as-is from source (uppercase)
                    assert_eq!(name.as_ref(), "INTEG");
                    assert_eq!(args.len(), 2);
                } else {
                    panic!("Expected INTEG call, got {:?}", eq.equation);
                }
            }
            other => panic!("Expected equation, got {:?}", other),
        }
    }

    // ========================================================================
    // Lookup table tests
    // ========================================================================

    #[test]
    fn test_lookup_pairs_format() {
        let input = "table((0, 0), (1, 1), (2, 4)) ~ ~ |";
        let mut reader = EquationReader::new(input);
        let item = reader.next_item();
        match item {
            Some(Ok(MdlItem::Equation(eq))) => {
                if let Equation::Lookup(_, table) = &eq.equation {
                    assert_eq!(table.x_vals, vec![0.0, 1.0, 2.0]);
                    assert_eq!(table.y_vals, vec![0.0, 1.0, 4.0]);
                } else {
                    panic!("Expected Lookup equation, got {:?}", eq.equation);
                }
            }
            other => panic!("Expected equation, got {:?}", other),
        }
    }

    #[test]
    fn test_lookup_pairs_with_range() {
        let input = "table([(0, 0) - (10, 10)], (0, 0), (5, 5), (10, 10)) ~ ~ |";
        let mut reader = EquationReader::new(input);
        let item = reader.next_item();
        match item {
            Some(Ok(MdlItem::Equation(eq))) => {
                if let Equation::Lookup(_, table) = &eq.equation {
                    assert!(table.x_range.is_some());
                    assert_eq!(table.x_vals, vec![0.0, 5.0, 10.0]);
                } else {
                    panic!("Expected Lookup equation, got {:?}", eq.equation);
                }
            }
            other => panic!("Expected equation, got {:?}", other),
        }
    }

    #[test]
    fn test_lookup_legacy_xy_format() {
        // Legacy format: x1, x2, x3, y1, y2, y3
        let input = "table(0, 1, 2, 10, 20, 30) ~ ~ |";
        let mut reader = EquationReader::new(input);
        let item = reader.next_item();
        match item {
            Some(Ok(MdlItem::Equation(eq))) => {
                if let Equation::Lookup(_, table) = &eq.equation {
                    assert_eq!(table.x_vals, vec![0.0, 1.0, 2.0]);
                    assert_eq!(table.y_vals, vec![10.0, 20.0, 30.0]);
                } else {
                    panic!("Expected Lookup equation, got {:?}", eq.equation);
                }
            }
            other => panic!("Expected equation, got {:?}", other),
        }
    }

    #[test]
    fn test_with_lookup() {
        let input = "y = WITH LOOKUP(Time, ((0, 0), (1, 1), (2, 4))) ~ ~ |";
        let mut reader = EquationReader::new(input);
        let item = reader.next_item();
        match item {
            Some(Ok(MdlItem::Equation(eq))) => {
                if let Equation::WithLookup(lhs, _, table) = &eq.equation {
                    assert_eq!(lhs.name.as_ref(), "y");
                    assert_eq!(table.x_vals, vec![0.0, 1.0, 2.0]);
                } else {
                    panic!("Expected WithLookup equation, got {:?}", eq.equation);
                }
            }
            other => panic!("Expected equation, got {:?}", other),
        }
    }

    // ========================================================================
    // Equation type tests
    // ========================================================================

    #[test]
    fn test_data_equation() {
        let input = "data var := GET XLS DATA('file.xlsx', 'Sheet1', 'A', 'B2') ~ ~ |";
        let mut reader = EquationReader::new(input);
        let item = reader.next_item();
        match item {
            Some(Ok(MdlItem::Equation(eq))) => {
                assert!(
                    matches!(eq.equation, Equation::Data(_, _)),
                    "Expected Data equation, got {:?}",
                    eq.equation
                );
            }
            other => panic!("Expected equation, got {:?}", other),
        }
    }

    #[test]
    fn test_implicit_equation() {
        let input = "exogenous data ~ ~ |";
        let mut reader = EquationReader::new(input);
        let item = reader.next_item();
        match item {
            Some(Ok(MdlItem::Equation(eq))) => {
                assert!(
                    matches!(eq.equation, Equation::Implicit(_)),
                    "Expected Implicit equation, got {:?}",
                    eq.equation
                );
            }
            other => panic!("Expected equation, got {:?}", other),
        }
    }

    #[test]
    fn test_empty_rhs_equation() {
        let input = "placeholder = ~ ~ This is A FUNCTION OF |";
        let mut reader = EquationReader::new(input);
        let item = reader.next_item();
        match item {
            Some(Ok(MdlItem::Equation(eq))) => {
                assert!(
                    matches!(eq.equation, Equation::EmptyRhs(_, _)),
                    "Expected EmptyRhs equation, got {:?}",
                    eq.equation
                );
            }
            other => panic!("Expected equation, got {:?}", other),
        }
    }

    // ========================================================================
    // Subscript tests
    // ========================================================================

    #[test]
    fn test_subscripted_variable() {
        let input = "var[DimA, DimB] = 5 ~ ~ |";
        let mut reader = EquationReader::new(input);
        let item = reader.next_item();
        match item {
            Some(Ok(MdlItem::Equation(eq))) => {
                if let Equation::Regular(lhs, _) = &eq.equation {
                    assert_eq!(lhs.name.as_ref(), "var");
                    assert_eq!(lhs.subscripts.len(), 2);
                } else {
                    panic!("Expected Regular equation, got {:?}", eq.equation);
                }
            }
            other => panic!("Expected equation, got {:?}", other),
        }
    }

    #[test]
    fn test_subscript_definition() {
        let input = "DimA: A1, A2, A3 ~ ~ |";
        let mut reader = EquationReader::new(input);
        let item = reader.next_item();
        match item {
            Some(Ok(MdlItem::Equation(eq))) => {
                if let Equation::SubscriptDef(name, def) = &eq.equation {
                    assert_eq!(name.as_ref(), "DimA");
                    assert_eq!(def.elements.len(), 3);
                } else {
                    panic!("Expected SubscriptDef equation, got {:?}", eq.equation);
                }
            }
            other => panic!("Expected equation, got {:?}", other),
        }
    }

    #[test]
    fn test_subscript_definition_with_range() {
        let input = "DimA: (A1 - A10) ~ ~ |";
        let mut reader = EquationReader::new(input);
        let item = reader.next_item();
        match item {
            Some(Ok(MdlItem::Equation(eq))) => {
                if let Equation::SubscriptDef(name, def) = &eq.equation {
                    assert_eq!(name.as_ref(), "DimA");
                    assert_eq!(def.elements.len(), 1);
                    assert!(matches!(
                        &def.elements[0],
                        crate::mdl::ast::SubscriptElement::Range(_, _, _)
                    ));
                } else {
                    panic!("Expected SubscriptDef equation, got {:?}", eq.equation);
                }
            }
            other => panic!("Expected equation, got {:?}", other),
        }
    }

    #[test]
    fn test_subscript_equivalence() {
        let input = "DimA <-> DimB ~ ~ |";
        let mut reader = EquationReader::new(input);
        let item = reader.next_item();
        match item {
            Some(Ok(MdlItem::Equation(eq))) => {
                assert!(
                    matches!(eq.equation, Equation::Equivalence(_, _, _)),
                    "Expected Equivalence equation, got {:?}",
                    eq.equation
                );
            }
            other => panic!("Expected equation, got {:?}", other),
        }
    }

    #[test]
    fn test_except_clause() {
        let input = "var[DimA] :EXCEPT: [A1, A2] = 5 ~ ~ |";
        let mut reader = EquationReader::new(input);
        let item = reader.next_item();
        match item {
            Some(Ok(MdlItem::Equation(eq))) => {
                if let Equation::Regular(lhs, _) = &eq.equation {
                    assert!(lhs.except.is_some());
                } else {
                    panic!(
                        "Expected Regular equation with except, got {:?}",
                        eq.equation
                    );
                }
            }
            other => panic!("Expected equation, got {:?}", other),
        }
    }

    // ========================================================================
    // Units tests
    // ========================================================================

    #[test]
    fn test_units_simple() {
        let input = "x = 5 ~ widgets ~ |";
        let mut reader = EquationReader::new(input);
        let item = reader.next_item();
        match item {
            Some(Ok(MdlItem::Equation(eq))) => {
                assert!(eq.units.is_some());
                let units = eq.units.unwrap();
                assert!(units.expr.is_some());
            }
            other => panic!("Expected equation, got {:?}", other),
        }
    }

    #[test]
    fn test_units_compound() {
        let input = "x = 5 ~ widgets/Year ~ |";
        let mut reader = EquationReader::new(input);
        let item = reader.next_item();
        match item {
            Some(Ok(MdlItem::Equation(eq))) => {
                assert!(eq.units.is_some());
                let units = eq.units.unwrap();
                if let Some(UnitExpr::Div(_, _, _)) = &units.expr {
                    // Expected
                } else {
                    panic!("Expected division unit, got {:?}", units.expr);
                }
            }
            other => panic!("Expected equation, got {:?}", other),
        }
    }

    #[test]
    fn test_units_with_range() {
        let input = "x = 5 ~ widgets [0, 100] ~ |";
        let mut reader = EquationReader::new(input);
        let item = reader.next_item();
        match item {
            Some(Ok(MdlItem::Equation(eq))) => {
                assert!(eq.units.is_some());
                let units = eq.units.unwrap();
                assert!(units.range.is_some());
                let range = units.range.unwrap();
                assert_eq!(range.min, Some(0.0));
                assert_eq!(range.max, Some(100.0));
            }
            other => panic!("Expected equation, got {:?}", other),
        }
    }

    // ========================================================================
    // Interp mode tests
    // ========================================================================

    #[test]
    fn test_interp_mode_interpolate() {
        let input = "data var :INTERPOLATE: ~ ~ |";
        let mut reader = EquationReader::new(input);
        let item = reader.next_item();
        match item {
            Some(Ok(MdlItem::Equation(eq))) => {
                if let Equation::Implicit(lhs) = &eq.equation {
                    assert_eq!(lhs.interp_mode, Some(InterpMode::Interpolate));
                } else {
                    panic!("Expected Implicit equation, got {:?}", eq.equation);
                }
            }
            other => panic!("Expected equation, got {:?}", other),
        }
    }

    #[test]
    fn test_interp_mode_raw() {
        let input = "data var :RAW: ~ ~ |";
        let mut reader = EquationReader::new(input);
        let item = reader.next_item();
        match item {
            Some(Ok(MdlItem::Equation(eq))) => {
                if let Equation::Implicit(lhs) = &eq.equation {
                    assert_eq!(lhs.interp_mode, Some(InterpMode::Raw));
                } else {
                    panic!("Expected Implicit equation, got {:?}", eq.equation);
                }
            }
            other => panic!("Expected equation, got {:?}", other),
        }
    }

    // ========================================================================
    // Special markers tests
    // ========================================================================

    #[test]
    fn test_eq_end_marker() {
        let input = "x = 5 ~ ~ |\n\\\\\\---///";
        let mut reader = EquationReader::new(input);

        // First should be the equation
        let item1 = reader.next_item();
        assert!(
            matches!(item1, Some(Ok(MdlItem::Equation(_)))),
            "item1: {:?}",
            item1
        );

        // Second should be EqEnd
        let item2 = reader.next_item();
        assert!(
            matches!(item2, Some(Ok(MdlItem::EqEnd(_)))),
            "item2 should be EqEnd, got: {:?}",
            item2
        );
    }

    #[test]
    fn test_group_marker() {
        // Group markers use {**name**} or ***\nname\n***| format
        let input = "{**Control Panel**}";
        let mut reader = EquationReader::new(input);
        let item = reader.next_item();
        match item {
            Some(Ok(MdlItem::Group(group))) => {
                assert!(
                    group.name.contains("Control Panel"),
                    "Expected 'Control Panel' in group name, got: {}",
                    group.name
                );
            }
            other => panic!("Expected Group, got {:?}", other),
        }
    }

    #[test]
    fn test_eq_end_followed_by_content() {
        // EqEnd marker followed by sketch content - should stop at EqEnd
        let input = "x = 5 ~ ~ |\n\\\\\\---///\nV300 sketch data here";
        let mut reader = EquationReader::new(input);

        // First: equation
        let item1 = reader.next_item();
        assert!(
            matches!(item1, Some(Ok(MdlItem::Equation(_)))),
            "item1: {:?}",
            item1
        );

        // Second: EqEnd marker (not combined with sketch data)
        let item2 = reader.next_item();
        assert!(
            matches!(item2, Some(Ok(MdlItem::EqEnd(_)))),
            "item2 should be EqEnd, got: {:?}",
            item2
        );

        // After EqEnd, reader should be finished
        let item3 = reader.next_item();
        assert!(item3.is_none(), "item3 should be None, got: {:?}", item3);
    }

    #[test]
    fn test_group_marker_followed_by_equation() {
        // Group marker followed by an equation - both should parse separately
        let input = "{**My Group**}\nx = 5 ~ ~ |";
        let mut reader = EquationReader::new(input);

        // First: group marker
        let item1 = reader.next_item();
        assert!(
            matches!(item1, Some(Ok(MdlItem::Group(_)))),
            "item1 should be Group, got: {:?}",
            item1
        );

        // Second: equation
        let item2 = reader.next_item();
        assert!(
            matches!(item2, Some(Ok(MdlItem::Equation(_)))),
            "item2 should be Equation, got: {:?}",
            item2
        );
    }

    // ========================================================================
    // Units with ? range tests
    // ========================================================================

    #[test]
    fn test_units_with_question_mark_max() {
        let input = "x = 5 ~ Month [0, ?] ~ |";
        let mut reader = EquationReader::new(input);
        let item = reader.next_item();
        match item {
            Some(Ok(MdlItem::Equation(eq))) => {
                assert!(eq.units.is_some());
                let units = eq.units.unwrap();
                assert!(units.range.is_some());
                let range = units.range.unwrap();
                assert_eq!(range.min, Some(0.0));
                assert_eq!(range.max, None); // ? maps to None
            }
            other => panic!("Expected equation, got {:?}", other),
        }
    }

    #[test]
    fn test_units_with_question_mark_min() {
        let input = "x = 5 ~ widgets [?, 100] ~ |";
        let mut reader = EquationReader::new(input);
        let item = reader.next_item();
        match item {
            Some(Ok(MdlItem::Equation(eq))) => {
                assert!(eq.units.is_some());
                let units = eq.units.unwrap();
                assert!(units.range.is_some());
                let range = units.range.unwrap();
                assert_eq!(range.min, None); // ? maps to None
                assert_eq!(range.max, Some(100.0));
            }
            other => panic!("Expected equation, got {:?}", other),
        }
    }

    #[test]
    fn test_units_with_question_mark_both() {
        let input = "x = 5 ~ Dmnl [?, ?] ~ |";
        let mut reader = EquationReader::new(input);
        let item = reader.next_item();
        match item {
            Some(Ok(MdlItem::Equation(eq))) => {
                assert!(eq.units.is_some());
                let units = eq.units.unwrap();
                assert!(units.range.is_some());
                let range = units.range.unwrap();
                assert_eq!(range.min, None);
                assert_eq!(range.max, None);
            }
            other => panic!("Expected equation, got {:?}", other),
        }
    }

    // ========================================================================
    // Macro tests
    // ========================================================================

    #[test]
    fn test_macro_followed_by_equation() {
        // :MACRO: definition followed by an equation inside it
        let input = ":MACRO: MYFUNC(arg1, arg2)\nx = arg1 + arg2 ~ ~ |\n:END OF MACRO:";
        let mut reader = EquationReader::new(input);

        // The reader should return a complete MacroDef after seeing :END OF MACRO:
        let item = reader.next_item();
        match item {
            Some(Ok(MdlItem::Macro(macro_def))) => {
                assert_eq!(macro_def.name.as_ref(), "MYFUNC");
                assert_eq!(macro_def.args.len(), 2);
                assert_eq!(macro_def.equations.len(), 1);
            }
            other => panic!("Expected Macro, got {:?}", other),
        }
    }

    // ========================================================================
    // Comment handling tests (Issue: special characters in comments)
    // ========================================================================

    #[test]
    fn test_comment_with_dollar_sign() {
        // Dollar signs in comments should not cause DollarSymbolOutsideUnits errors
        let input = "x = 5 ~ Units ~ Cost is $100 per unit |";
        let mut reader = EquationReader::new(input);
        let item = reader.next_item();
        match item {
            Some(Ok(MdlItem::Equation(eq))) => {
                assert_eq!(
                    eq.comment.as_ref().unwrap().as_ref(),
                    "Cost is $100 per unit"
                );
            }
            other => panic!("Expected equation with comment, got {:?}", other),
        }
    }

    #[test]
    fn test_comment_with_at_symbol() {
        // @ symbols in comments should not cause errors
        let input = "email = 0 ~ ~ Contact: user@example.com |";
        let mut reader = EquationReader::new(input);
        let item = reader.next_item();
        match item {
            Some(Ok(MdlItem::Equation(eq))) => {
                assert_eq!(
                    eq.comment.as_ref().unwrap().as_ref(),
                    "Contact: user@example.com"
                );
            }
            other => panic!("Expected equation with comment, got {:?}", other),
        }
    }

    #[test]
    fn test_comment_with_hash_symbol() {
        // # symbols in comments should not cause errors
        let input = "count = 10 ~ ~ See issue #123 for details |";
        let mut reader = EquationReader::new(input);
        let item = reader.next_item();
        match item {
            Some(Ok(MdlItem::Equation(eq))) => {
                assert_eq!(
                    eq.comment.as_ref().unwrap().as_ref(),
                    "See issue #123 for details"
                );
            }
            other => panic!("Expected equation with comment, got {:?}", other),
        }
    }

    #[test]
    fn test_comment_with_multiple_special_chars() {
        // Multiple special characters in comments
        let input = "var = 1 ~ ~ Price: $50 @ 10% discount = #savings! |";
        let mut reader = EquationReader::new(input);
        let item = reader.next_item();
        match item {
            Some(Ok(MdlItem::Equation(eq))) => {
                assert_eq!(
                    eq.comment.as_ref().unwrap().as_ref(),
                    "Price: $50 @ 10% discount = #savings!"
                );
            }
            other => panic!("Expected equation with comment, got {:?}", other),
        }
    }

    // ========================================================================
    // Comment termination tests (Issue: EqEnd in comments)
    // ========================================================================

    #[test]
    fn test_comment_terminated_by_eq_end() {
        // If a comment lacks the | terminator but EqEnd marker appears,
        // the comment should be captured and EqEnd should still be emitted
        let input = "x = 5 ~ Units ~ This is a comment\n\\\\\\---///";
        let mut reader = EquationReader::new(input);

        // First should be the equation with comment
        let item1 = reader.next_item();
        match item1 {
            Some(Ok(MdlItem::Equation(eq))) => {
                assert!(matches!(eq.equation, Equation::Regular(_, _)));
                assert_eq!(eq.comment.as_ref().unwrap().as_ref(), "This is a comment");
            }
            other => panic!("Expected equation with comment, got {:?}", other),
        }

        // Second should be EqEnd
        let item2 = reader.next_item();
        assert!(
            matches!(item2, Some(Ok(MdlItem::EqEnd(_)))),
            "Expected EqEnd, got {:?}",
            item2
        );

        // Third should be None
        let item3 = reader.next_item();
        assert!(item3.is_none(), "Expected None, got {:?}", item3);
    }

    #[test]
    fn test_comment_terminated_by_forward_eq_end() {
        // Test with the forward EqEnd marker (///---\\\)
        let input = "x = 5 ~ Units ~ Comment here\n///---\\\\\\";
        let mut reader = EquationReader::new(input);

        let item1 = reader.next_item();
        match item1 {
            Some(Ok(MdlItem::Equation(eq))) => {
                assert_eq!(eq.comment.as_ref().unwrap().as_ref(), "Comment here");
            }
            other => panic!("Expected equation, got {:?}", other),
        }

        let item2 = reader.next_item();
        assert!(
            matches!(item2, Some(Ok(MdlItem::EqEnd(_)))),
            "Expected EqEnd, got {:?}",
            item2
        );
    }

    // ========================================================================
    // :NA: in number lists tests
    // ========================================================================

    #[test]
    fn test_number_list_with_na() {
        // :NA: should be allowed in number lists as a sentinel value
        let input = "x = 1, :NA:, 3 ~ ~ |";
        let mut reader = EquationReader::new(input);
        let item = reader.next_item();
        match item {
            Some(Ok(MdlItem::Equation(eq))) => {
                if let Equation::NumberList(_, nums) = &eq.equation {
                    assert_eq!(nums.len(), 3);
                    assert_eq!(nums[0], 1.0);
                    assert_eq!(nums[1], -1e38); // NA sentinel value
                    assert_eq!(nums[2], 3.0);
                } else {
                    panic!("Expected NumberList, got {:?}", eq.equation);
                }
            }
            other => panic!("Expected equation, got {:?}", other),
        }
    }

    #[test]
    fn test_number_list_starting_with_na() {
        let input = "x = :NA:, 2, 3 ~ ~ |";
        let mut reader = EquationReader::new(input);
        let item = reader.next_item();
        match item {
            Some(Ok(MdlItem::Equation(eq))) => {
                if let Equation::NumberList(_, nums) = &eq.equation {
                    assert_eq!(nums.len(), 3);
                    assert_eq!(nums[0], -1e38);
                    assert_eq!(nums[1], 2.0);
                    assert_eq!(nums[2], 3.0);
                } else {
                    panic!("Expected NumberList, got {:?}", eq.equation);
                }
            }
            other => panic!("Expected equation, got {:?}", other),
        }
    }

    #[test]
    fn test_number_list_all_na() {
        let input = "x = :NA:, :NA:, :NA: ~ ~ |";
        let mut reader = EquationReader::new(input);
        let item = reader.next_item();
        match item {
            Some(Ok(MdlItem::Equation(eq))) => {
                if let Equation::NumberList(_, nums) = &eq.equation {
                    assert_eq!(nums.len(), 3);
                    assert!(nums.iter().all(|&n| n == -1e38));
                } else {
                    panic!("Expected NumberList, got {:?}", eq.equation);
                }
            }
            other => panic!("Expected equation, got {:?}", other),
        }
    }

    // ========================================================================
    // Legacy lookup error propagation tests
    // ========================================================================

    #[test]
    fn test_legacy_lookup_odd_count_error() {
        // Legacy lookup with odd number of values should produce an error
        let input = "table(0, 1, 2) ~ ~ |";
        let mut reader = EquationReader::new(input);
        let item = reader.next_item();
        assert!(
            matches!(item, Some(Err(ReaderError::Parse(_)))),
            "Expected parse error for odd-count legacy lookup, got {:?}",
            item
        );
    }

    // ========================================================================
    // Supplementary flag tests
    // ========================================================================

    #[test]
    fn test_supplementary_flag_full() {
        // :SUPPLEMENTARY flag should be parsed and removed from comment
        let input = "x = 1 ~ units ~ docs ~ :SUPPLEMENTARY |";
        let mut reader = EquationReader::new(input);
        let item = reader.next_item();
        match item {
            Some(Ok(MdlItem::Equation(eq))) => {
                assert!(eq.supplementary, "Expected supplementary=true");
                assert_eq!(eq.comment.as_deref(), Some("docs"));
            }
            other => panic!("Expected equation, got {:?}", other),
        }
    }

    #[test]
    fn test_supplementary_flag_short() {
        // :SUP short form should also work
        let input = "x = 1 ~ ~ ~ :SUP |";
        let mut reader = EquationReader::new(input);
        let item = reader.next_item();
        match item {
            Some(Ok(MdlItem::Equation(eq))) => {
                assert!(eq.supplementary, "Expected supplementary=true");
                assert!(
                    eq.comment.is_none(),
                    "Expected no comment, got {:?}",
                    eq.comment
                );
            }
            other => panic!("Expected equation, got {:?}", other),
        }
    }

    #[test]
    fn test_no_supplementary_flag() {
        // Normal equations should have supplementary=false
        let input = "x = 1 ~ units ~ docs |";
        let mut reader = EquationReader::new(input);
        let item = reader.next_item();
        match item {
            Some(Ok(MdlItem::Equation(eq))) => {
                assert!(!eq.supplementary, "Expected supplementary=false");
                assert_eq!(eq.comment.as_deref(), Some("docs"));
            }
            other => panic!("Expected equation, got {:?}", other),
        }
    }

    #[test]
    fn test_supplementary_inline_compact() {
        // Compact inline format: ~~~:SUP|
        let input = "x = 1~~~:SUP|";
        let mut reader = EquationReader::new(input);
        let item = reader.next_item();
        match item {
            Some(Ok(MdlItem::Equation(eq))) => {
                assert!(eq.supplementary, "Expected supplementary=true");
            }
            other => panic!("Expected equation, got {:?}", other),
        }
    }

    #[test]
    fn test_supplementary_with_comment_text() {
        // Real-world example: comment text followed by supplementary flag
        let input = "profit = revenue - cost ~ $ ~ Companies Profits - not used ~ :SUP |";
        let mut reader = EquationReader::new(input);
        let item = reader.next_item();
        match item {
            Some(Ok(MdlItem::Equation(eq))) => {
                assert!(eq.supplementary, "Expected supplementary=true");
                assert_eq!(eq.comment.as_deref(), Some("Companies Profits - not used"));
            }
            other => panic!("Expected equation, got {:?}", other),
        }
    }

    #[test]
    fn test_supplementary_case_insensitive() {
        // :supplementary should work too (case insensitive)
        let input = "x = 1 ~ ~ ~ :supplementary |";
        let mut reader = EquationReader::new(input);
        let item = reader.next_item();
        match item {
            Some(Ok(MdlItem::Equation(eq))) => {
                assert!(eq.supplementary, "Expected supplementary=true");
            }
            other => panic!("Expected equation, got {:?}", other),
        }
    }

    // ========================================================================
    // Remaining source tests
    // ========================================================================

    #[test]
    fn test_remaining_after_eq_end() {
        // After parsing, remaining() should return content after EqEnd marker
        let input = "x = 5 ~ ~ |\n\\\\\\---/// Sketch info\nV300\n*View 1\n///---\\\\\\\n:L<%^E!@\n15:0,0,0,1,0,0";
        let mut reader = EquationReader::new(input);

        // Parse the equation
        let item = reader.next_item();
        assert!(matches!(item, Some(Ok(MdlItem::Equation(_)))));

        // Parse EqEnd
        let item = reader.next_item();
        assert!(matches!(item, Some(Ok(MdlItem::EqEnd(_)))));

        // Remaining should contain everything after the EqEnd marker
        let remaining = reader.remaining();
        assert!(
            remaining.contains("V300"),
            "remaining should contain V300, got: {}",
            remaining
        );
        assert!(
            remaining.contains(":L<%^E!@"),
            "remaining should contain settings marker, got: {}",
            remaining
        );
        assert!(
            remaining.contains("15:0,0,0,1,0,0"),
            "remaining should contain type 15 line, got: {}",
            remaining
        );
    }

    #[test]
    fn test_remaining_exhausted_reader() {
        let input = "x = 5 ~ ~ |";
        let mut reader = EquationReader::new(input);

        // Parse all items
        let item = reader.next_item();
        assert!(matches!(item, Some(Ok(MdlItem::Equation(_)))));

        let item = reader.next_item();
        assert!(item.is_none());

        // Remaining should be empty after exhaustion
        let remaining = reader.remaining();
        assert!(
            remaining.is_empty(),
            "remaining should be empty, got: '{}'",
            remaining
        );
    }
}
