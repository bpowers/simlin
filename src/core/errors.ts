// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// This enum MUST mirror libsimlin's `SimlinErrorCode` (the C ABI wire
// format in src/libsimlin/src/lib.rs) value-for-value: error codes reach
// TypeScript as raw integers from that enum, NOT from the engine's internal
// `ErrorCode` (which numbers some variants differently). Engine codes that
// libsimlin collapses (e.g. the `Todo*` family) arrive as `Generic`.
// An identical copy lives in src/engine/src/errors.ts (the packages don't
// depend on each other); keep the two in sync.
export enum ErrorCode {
  NoError = 0,
  DoesNotExist = 1,
  XmlDeserialization = 2,
  VensimConversion = 3,
  ProtobufDecode = 4,
  InvalidToken = 5,
  UnrecognizedEof = 6,
  UnrecognizedToken = 7,
  ExtraToken = 8,
  UnclosedComment = 9,
  UnclosedQuotedIdent = 10,
  ExpectedNumber = 11,
  UnknownBuiltin = 12,
  BadBuiltinArgs = 13,
  EmptyEquation = 14,
  BadModuleInputDst = 15,
  BadModuleInputSrc = 16,
  NotSimulatable = 17,
  BadTable = 18,
  BadSimSpecs = 19,
  NoAbsoluteReferences = 20,
  CircularDependency = 21,
  ArraysNotImplemented = 22,
  MultiDimensionalArraysNotImplemented = 23,
  BadDimensionName = 24,
  BadModelName = 25,
  MismatchedDimensions = 26,
  ArrayReferenceNeedsExplicitSubscripts = 27,
  DuplicateVariable = 28,
  UnknownDependency = 29,
  VariablesHaveErrors = 30,
  UnitDefinitionErrors = 31,
  Generic = 32,
  UnitMismatch = 33,
  BadOverride = 34,
  NoAppInUnits = 35,
  NoSubscriptInUnits = 36,
  NoIfInUnits = 37,
  NoUnaryOpInUnits = 38,
  BadBinaryOpInUnits = 39,
  NoConstInUnits = 40,
  ExpectedInteger = 41,
  ExpectedIntegerOne = 42,
  DuplicateUnit = 43,
  ExpectedModule = 44,
  ExpectedIdent = 45,
}

export function errorCodeDescription(code: ErrorCode): string {
  switch (code) {
    case ErrorCode.NoError:
      return 'Internal error';
    case ErrorCode.DoesNotExist:
      return 'Does not exist';
    case ErrorCode.XmlDeserialization:
      return 'XML deserialization error';
    case ErrorCode.VensimConversion:
      return 'Vensim conversion error';
    case ErrorCode.ProtobufDecode:
      return 'Internal error (protocol buffer decoding)';
    case ErrorCode.InvalidToken:
      return 'Invalid input in equation';
    case ErrorCode.UnrecognizedEof:
      return 'Unexpectedly reached the end of the equation';
    case ErrorCode.UnrecognizedToken:
      return 'Unrecognized input in equation';
    case ErrorCode.ExtraToken:
      return 'Extra input after equation fully parsed';
    case ErrorCode.UnclosedComment:
      return 'Unclosed comment';
    case ErrorCode.UnclosedQuotedIdent:
      return 'Unclosed quoted identifier';
    case ErrorCode.ExpectedNumber:
      return 'Expected a literal number';
    case ErrorCode.UnknownBuiltin:
      return 'Reference to unknown or unimplemented builtin';
    case ErrorCode.BadBuiltinArgs:
      return 'Incorrect arguments to a builtin function (e.g. too many, too few)';
    case ErrorCode.EmptyEquation:
      return 'Variable has empty equation';
    case ErrorCode.BadModuleInputDst:
      return 'Module input destination is unknown';
    case ErrorCode.BadModuleInputSrc:
      return 'Module input source is unknown';
    case ErrorCode.NotSimulatable:
      return 'Model has errors and is not simulatable';
    case ErrorCode.BadTable:
      return 'No graphical function for specified variable';
    case ErrorCode.BadSimSpecs:
      return 'Simulation Specs are not valid';
    case ErrorCode.NoAbsoluteReferences:
      return 'Absolute references are not supported';
    case ErrorCode.CircularDependency:
      return 'Circular dependency';
    case ErrorCode.ArraysNotImplemented:
      return 'Arrays not implemented';
    case ErrorCode.MultiDimensionalArraysNotImplemented:
      return 'Multi-dimensional arrays not implemented';
    case ErrorCode.BadDimensionName:
      return 'Unknown dimension name';
    case ErrorCode.BadModelName:
      return 'Unknown model name';
    case ErrorCode.MismatchedDimensions:
      return 'Mismatched dimensions';
    case ErrorCode.ArrayReferenceNeedsExplicitSubscripts:
      return 'Array reference needs explicit subscripts';
    case ErrorCode.DuplicateVariable:
      return 'Duplicate variable';
    case ErrorCode.UnknownDependency:
      return 'Equation refers to unknown variable';
    case ErrorCode.VariablesHaveErrors:
      return 'Variables have equation errors';
    case ErrorCode.UnitDefinitionErrors:
      return "The project's unit definitions have errors";
    case ErrorCode.Generic:
      return 'Generic error from core engine';
    case ErrorCode.UnitMismatch:
      return 'Unit mismatch';
    case ErrorCode.BadOverride:
      return 'Invalid value override';
    case ErrorCode.NoAppInUnits:
      return 'Function calls are not allowed in unit definition';
    case ErrorCode.NoSubscriptInUnits:
      return 'Subscripts are not allowed in unit definition';
    case ErrorCode.NoIfInUnits:
      return 'If statements are not allowed in unit definition';
    case ErrorCode.NoUnaryOpInUnits:
      return "Negative units like `-people` don't make sense. Try e.g. `1/people`";
    case ErrorCode.BadBinaryOpInUnits:
      return 'Only * and / operations are supported in unit definitions';
    case ErrorCode.NoConstInUnits:
      return 'Constants are not supported in unit definitions';
    case ErrorCode.ExpectedInteger:
      return 'Expected an integer';
    case ErrorCode.ExpectedIntegerOne:
      return 'Expected the integer `1`';
    case ErrorCode.DuplicateUnit:
      return 'Duplicate unit definition';
    case ErrorCode.ExpectedModule:
      return 'Expected a module, found a non-module';
    case ErrorCode.ExpectedIdent:
      return 'Expected an identifier';
  }
  return 'Unknown error from core engine';
}
