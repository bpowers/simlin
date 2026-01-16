// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// Type definitions matching simlin.h

// Pointer types - all are numbers in JavaScript but aliases help documentation
export type Ptr = number;
export type SimlinProjectPtr = number;
export type SimlinModelPtr = number;
export type SimlinSimPtr = number;
export type SimlinErrorPtr = number;
export type SimlinLoopsPtr = number;
export type SimlinLinksPtr = number;
export type SimlinErrorDetailPtr = number;

// Error codes for the C API
export enum SimlinErrorCode {
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
}

// Error kind categorizing where in the project the error originates
export enum SimlinErrorKind {
  Project = 0,
  Model = 1,
  Variable = 2,
  Units = 3,
  Simulation = 4,
}

// Unit error kind for distinguishing types of unit-related errors
export enum SimlinUnitErrorKind {
  NotApplicable = 0,
  Definition = 1,
  Consistency = 2,
  Inference = 3,
}

// JSON format specifier
export enum SimlinJsonFormat {
  Native = 0,
  Sdai = 1,
}

// Link polarity
export enum SimlinLinkPolarity {
  Positive = 0,
  Negative = 1,
  Unknown = 2,
}

// Loop polarity
export enum SimlinLoopPolarity {
  Reinforcing = 0,
  Balancing = 1,
}

// Error detail structure - matches SimlinErrorDetail in simlin.h
export interface ErrorDetail {
  code: SimlinErrorCode;
  message: string | null;
  modelName: string | null;
  variableName: string | null;
  startOffset: number;
  endOffset: number;
  kind: SimlinErrorKind;
  unitErrorKind: SimlinUnitErrorKind;
}

// Link structure - matches SimlinLink in simlin.h
export interface Link {
  from: string;
  to: string;
  polarity: SimlinLinkPolarity;
  score: Float64Array | null;
}

// Loop structure - matches SimlinLoop in simlin.h
export interface Loop {
  id: string;
  variables: string[];
  polarity: SimlinLoopPolarity;
}
