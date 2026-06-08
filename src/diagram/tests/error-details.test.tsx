// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// jsdom does not provide TextEncoder/TextDecoder, but the engine's memory
// module (pulled in transitively via errorCodeDescription) uses them at import
// time. Polyfill from Node's util before importing anything engine-backed.
import { TextEncoder, TextDecoder } from 'util';
Object.assign(globalThis, { TextEncoder, TextDecoder });

import * as React from 'react';
import { render, screen } from '@testing-library/react';

import { SimError, ModelError, EquationError, ErrorCode, UnitError } from '@simlin/core/datamodel';

import { ErrorDetails } from '../ErrorDetails';

const noErrors = {
  simError: undefined as SimError | undefined,
  modelErrors: [] as readonly ModelError[],
  varErrors: new Map<string, readonly EquationError[]>(),
  varUnitErrors: new Map<string, readonly UnitError[]>(),
  status: 'ok' as const,
};

describe('ErrorDetails', () => {
  test('shows the all-clear message when there are no errors', () => {
    render(<ErrorDetails {...noErrors} />);
    expect(screen.getByText(/your model is error free/i)).not.toBeNull();
  });

  test('renders a simulation error', () => {
    render(<ErrorDetails {...noErrors} simError={{ code: ErrorCode.Generic, details: undefined }} />);
    expect(screen.getByText(/simulation error:/i)).not.toBeNull();
    expect(screen.queryByText(/error free/i)).toBeNull();
  });

  test('suppresses a NotSimulatable sim error when model errors are present', () => {
    render(
      <ErrorDetails
        {...noErrors}
        simError={{ code: ErrorCode.NotSimulatable, details: undefined }}
        modelErrors={[{ code: ErrorCode.BadSimSpecs, details: undefined }]}
      />,
    );
    expect(screen.queryByText(/simulation error:/i)).toBeNull();
    expect(screen.getByText(/model error:/i)).not.toBeNull();
  });

  test('renders model error details when present', () => {
    render(<ErrorDetails {...noErrors} modelErrors={[{ code: ErrorCode.BadSimSpecs, details: 'dt is zero' }]} />);
    expect(screen.getByText(/dt is zero/)).not.toBeNull();
  });

  test('suppresses the VariablesHaveErrors umbrella when per-variable errors exist', () => {
    render(
      <ErrorDetails
        {...noErrors}
        modelErrors={[{ code: ErrorCode.VariablesHaveErrors, details: undefined }]}
        varErrors={new Map([['inflow', [{ code: ErrorCode.EmptyEquation, start: 0, end: 1 }]]])}
      />,
    );
    // The umbrella model error is dropped...
    expect(screen.queryByText(/model error:/i)).toBeNull();
    // ...in favor of the specific variable error.
    expect(screen.getByText(/variable "inflow" error:/i)).not.toBeNull();
  });

  test('renders per-variable unit errors with details', () => {
    render(
      <ErrorDetails
        {...noErrors}
        varUnitErrors={
          new Map([
            ['flow', [{ code: ErrorCode.UnitMismatch, start: 0, end: 1, isConsistencyError: true, details: 'm vs s' }]],
          ])
        }
      />,
    );
    expect(screen.getByText(/variable "flow" unit error:/i)).not.toBeNull();
    expect(screen.getByText(/m vs s/)).not.toBeNull();
  });
});
