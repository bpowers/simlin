/**
 * @jest-environment jsdom
 *
 * Copyright 2026 The Simlin Authors. All rights reserved.
 * Use of this source code is governed by the Apache License,
 * Version 2.0, that can be found in the LICENSE file.
 */

// Regression tests for the equation preview in VariableDetails:
//
//  - Raw equation text must never be fed to katex.renderToString: while the
//    engine LaTeX is loading (or when the engine can't produce LaTeX), the
//    raw text is NOT LaTeX -- identifiers like revenue_per_unit would render
//    with `_` as subscripts and `\`/`{}`/`^` as control sequences.
//  - Non-fatal unit warnings must not force the panel out of the rendered
//    preview into the raw editor (mirrors variableDetailsView, where only
//    equation errors hide the chart).
//  - Equation errors DO force the editor (so the highlight is visible).

import { TextEncoder, TextDecoder } from 'util';
Object.assign(globalThis, { TextEncoder, TextDecoder });

import * as React from 'react';
import { render, act } from '@testing-library/react';
import { VariableDetails } from '../VariableDetails';
import { Aux, AuxViewElement, EquationError, ErrorCode, UnitError } from '@simlin/core/datamodel';

function makeAux(ident: string, equation: string, overrides: Partial<Aux> = {}): Aux {
  return {
    type: 'aux',
    ident,
    equation: { type: 'scalar', equation },
    documentation: '',
    units: '',
    gf: undefined,
    data: undefined,
    errors: undefined,
    unitErrors: undefined,
    uid: undefined,
    ...overrides,
  };
}

function makeViewElement(ident: string): AuxViewElement {
  return {
    type: 'aux',
    uid: 1,
    name: ident,
    ident,
    var: undefined,
    x: 0,
    y: 0,
    labelSide: 'right',
    isZeroRadius: false,
  };
}

const noop = () => {};

function renderDetails(variable: Aux, getLatexEquation?: (ident: string) => Promise<string | undefined>) {
  return render(
    <VariableDetails
      variable={variable}
      viewElement={makeViewElement(variable.ident)}
      getLatexEquation={getLatexEquation}
      onDelete={noop}
      onEquationChange={noop}
      onTableChange={noop}
      activeTab={0}
      onActiveTabChange={noop}
    />,
  );
}

describe('VariableDetails equation preview', () => {
  it('renders raw text as plain text (not KaTeX) while the engine LaTeX loads', async () => {
    // A pending promise keeps latexEquation undefined.
    const never = new Promise<string | undefined>(() => {});
    const { container } = renderDetails(makeAux('rev', 'revenue_per_unit * sales'), () => never);

    const preview = container.querySelector('.eqnPreview');
    expect(preview).not.toBeNull();
    // The raw text appears verbatim, and no KaTeX markup was generated
    // (KaTeX would split revenue_per_unit into subscript spans).
    expect(preview!.textContent).toContain('revenue_per_unit * sales');
    expect(container.querySelector('.katex')).toBeNull();
  });

  it('renders engine-provided LaTeX through KaTeX once it arrives', async () => {
    let resolve!: (v: string | undefined) => void;
    const pending = new Promise<string | undefined>((res) => {
      resolve = res;
    });
    const { container } = renderDetails(makeAux('rev', 'a + b'), () => pending);

    await act(async () => {
      resolve('a + b');
    });

    expect(container.querySelector('.katex')).not.toBeNull();
  });

  it('renders raw text as plain text when the engine cannot produce LaTeX', async () => {
    const { container } = renderDetails(makeAux('rev', 'revenue_per_unit * 2'), async () => undefined);

    await act(async () => {
      // let the resolved-undefined promise settle
    });

    const preview = container.querySelector('.eqnPreview');
    expect(preview).not.toBeNull();
    expect(preview!.textContent).toContain('revenue_per_unit * 2');
    expect(container.querySelector('.katex')).toBeNull();
  });

  it('keeps the preview for a variable with only non-fatal unit warnings', () => {
    const unitErrors: UnitError[] = [
      { start: 0, end: 0, code: 0 as unknown as ErrorCode, isConsistencyError: false, details: undefined },
    ];
    const { container } = renderDetails(makeAux('rev', 'a + b', { unitErrors }));

    expect(container.querySelector('.eqnPreview')).not.toBeNull();
  });

  it('shows the editor (not the preview) for a variable with equation errors', () => {
    const errors: EquationError[] = [{ start: 0, end: 1, code: 0 as unknown as ErrorCode }];
    const { container } = renderDetails(makeAux('rev', 'a + b', { errors }));

    expect(container.querySelector('.eqnPreview')).toBeNull();
    expect(container.querySelector('.eqnEditor')).not.toBeNull();
  });

  it('highlights the errored range accounting for non-ASCII byte offsets', () => {
    // 'é' is 2 UTF-8 bytes; engine byte range [8, 9) of 'café + bad' is 'b'…
    // here we use [7, 10) which covers 'bad' (bytes: c1 a2 f3 é5 ␠6 +7 ␠8 b9 a10 d11)
    // → byte range for 'bad' is [8, 11).
    const errors: EquationError[] = [{ start: 8, end: 11, code: 0 as unknown as ErrorCode }];
    const { container } = renderDetails(makeAux('x', 'café + bad', { errors }));

    const marked = container.querySelector('.eqnError');
    expect(marked).not.toBeNull();
    expect(marked!.textContent).toBe('bad');
  });

  it('highlights an error on the second line of a multi-line equation', () => {
    const equation = 'a +\nbad_ref';
    // byte offsets into the raw equation: 'bad_ref' starts at 4 (after 'a +\n')
    const errors: EquationError[] = [{ start: 4, end: 11, code: 0 as unknown as ErrorCode }];
    const { container } = renderDetails(makeAux('x', equation, { errors }));

    const marked = container.querySelector('.eqnError');
    expect(marked).not.toBeNull();
    expect(marked!.textContent).toBe('bad_ref');
  });
});
