/**
 * @jest-environment jsdom
 *
 * Copyright 2026 The Simlin Authors. All rights reserved.
 * Use of this source code is governed by the Apache License,
 * Version 2.0, that can be found in the LICENSE file.
 */

// Regression tests for the loadLatex race guard in VariableDetails. Selecting a
// new variable while the previous variable's getLatexEquation() promise is still
// in flight must not let the stale response (success or error) overwrite the
// current variable's rendered LaTeX. The component is keyed on viewElement.ident
// inside its load effect, so a rerender with a new ident re-fires the request and
// bumps the request id; only the latest request is allowed to commit.
//
// These assertions are made against observable render output -- whether KaTeX
// markup (.katex) is present and what raw text the plain-text fallback shows --
// rather than internal state. The plain-text fallback (.eqnPlain) renders exactly
// when latexEquation is undefined (loading, or no engine LaTeX); KaTeX renders
// once a non-undefined LaTeX string has been committed.

// jsdom does not provide TextEncoder/TextDecoder, but the engine's
// memory module uses them at import time.  Polyfill from Node's util.
import { TextEncoder, TextDecoder } from 'util';
Object.assign(globalThis, { TextEncoder, TextDecoder });

import * as React from 'react';
import { render, act } from '@testing-library/react';
import { VariableDetails } from '../VariableDetails';
import { Aux, AuxViewElement } from '@simlin/core/datamodel';

function makeAux(ident: string): Aux {
  return {
    type: 'aux',
    ident,
    equation: { type: 'scalar', equation: '1' },
    documentation: '',
    units: '',
    gf: undefined,
    data: undefined,
    errors: undefined,
    unitErrors: undefined,
    uid: undefined,
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

interface Deferred<T> {
  promise: Promise<T>;
  resolve: (value: T) => void;
  reject: (err: Error) => void;
}

function createDeferred<T>(): Deferred<T> {
  let resolve!: (value: T) => void;
  let reject!: (err: Error) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

const noop = () => {};

function renderDetails(
  ident: string,
  getLatexEquation: (ident: string) => Promise<string | undefined>,
): ReturnType<typeof render> {
  return render(
    <VariableDetails
      variable={makeAux(ident)}
      viewElement={makeViewElement(ident)}
      getLatexEquation={getLatexEquation}
      onDelete={noop}
      onEquationChange={noop}
      onTableChange={noop}
      activeTab={0}
      onActiveTabChange={noop}
    />,
  );
}

function rerenderDetails(
  rerender: ReturnType<typeof render>['rerender'],
  ident: string,
  getLatexEquation: (ident: string) => Promise<string | undefined>,
): void {
  rerender(
    <VariableDetails
      variable={makeAux(ident)}
      viewElement={makeViewElement(ident)}
      getLatexEquation={getLatexEquation}
      onDelete={noop}
      onEquationChange={noop}
      onTableChange={noop}
      activeTab={0}
      onActiveTabChange={noop}
    />,
  );
}

// The KaTeX preview is committed once a non-undefined LaTeX string arrives; the
// raw equation ('1') renders in .eqnPlain while LaTeX is undefined.
function hasKatex(container: HTMLElement): boolean {
  return container.querySelector('.katex') !== null;
}

// The visible text KaTeX rendered, read from the HTML (non-MathML) layer. KaTeX
// duplicates content into a clipped MathML accessibility mirror (.katex-mathml);
// reading .katex-html avoids counting that mirror twice. Whitespace is collapsed
// because KaTeX splits glyphs across many spans.
function katexText(container: HTMLElement): string {
  const htmlLayer = container.querySelector('.katex-html') ?? container.querySelector('.katex');
  return (htmlLayer?.textContent ?? '').replace(/\s+/g, ' ').trim();
}

function plainText(container: HTMLElement): string | undefined {
  return container.querySelector('.eqnPlain')?.textContent ?? undefined;
}

describe('VariableDetails latex loading', () => {
  test('loadLatex race condition: only the latest request commits its LaTeX', async () => {
    const calls: Array<{ ident: string; deferred: Deferred<string | undefined> }> = [];
    const getLatexEquation = jest.fn((ident: string) => {
      const deferred = createDeferred<string | undefined>();
      calls.push({ ident, deferred });
      return deferred.promise;
    });

    const { container, rerender } = renderDetails('var_a', getLatexEquation);

    // Mount fires loadLatex for var_a.
    expect(calls.length).toBe(1);
    expect(calls[0].ident).toBe('var_a');

    // Select a new variable before var_a's response arrives; the ident-keyed
    // effect re-fires for var_b.
    rerenderDetails(rerender, 'var_b', getLatexEquation);
    expect(calls.length).toBe(2);
    expect(calls[1].ident).toBe('var_b');

    // Resolve var_b first (the current request): KaTeX renders var_b's LaTeX.
    await act(async () => {
      calls[1].deferred.resolve('\\text{betaEqn}');
    });
    expect(katexText(container)).toContain('betaEqn');

    // Resolve var_a (the stale request) with a DIFFERENT, recognizable LaTeX
    // string. The request-id guard must drop it, so the rendered KaTeX still
    // shows var_b's equation and never var_a's. (If the guard is removed, the
    // stale success overwrites the preview and this assertion fails.)
    await act(async () => {
      calls[0].deferred.resolve('\\text{alphaEqn}');
    });
    expect(katexText(container)).toContain('betaEqn');
    expect(katexText(container)).not.toContain('alphaEqn');
  });

  test('loadLatex race condition: a stale error does not clear the current LaTeX', async () => {
    const calls: Array<{ ident: string; deferred: Deferred<string | undefined> }> = [];
    const getLatexEquation = jest.fn((ident: string) => {
      const deferred = createDeferred<string | undefined>();
      calls.push({ ident, deferred });
      return deferred.promise;
    });

    const { container, rerender } = renderDetails('var_a', getLatexEquation);
    rerenderDetails(rerender, 'var_b', getLatexEquation);
    expect(calls.length).toBe(2);

    await act(async () => {
      calls[1].deferred.resolve('\\text{var b}');
    });
    expect(hasKatex(container)).toBe(true);

    // Reject var_a with an error -- the stale rejection must be ignored.
    await act(async () => {
      calls[0].deferred.reject(new Error('network error'));
    });
    expect(hasKatex(container)).toBe(true);
  });

  test('rapid variable changes: only the final request commits', async () => {
    const calls: Array<{ ident: string; deferred: Deferred<string | undefined> }> = [];
    const getLatexEquation = jest.fn((ident: string) => {
      const deferred = createDeferred<string | undefined>();
      calls.push({ ident, deferred });
      return deferred.promise;
    });

    const { container, rerender } = renderDetails('v1', getLatexEquation);
    rerenderDetails(rerender, 'v2', getLatexEquation);
    rerenderDetails(rerender, 'v3', getLatexEquation);
    expect(calls.length).toBe(3);

    // Resolve in the order v1, v2, v3. Only v3 (the current request) commits.
    await act(async () => {
      calls[0].deferred.resolve('latex_v1');
    });
    expect(hasKatex(container)).toBe(false);

    await act(async () => {
      calls[1].deferred.resolve('latex_v2');
    });
    expect(hasKatex(container)).toBe(false);

    await act(async () => {
      calls[2].deferred.resolve('latex_v3');
    });
    expect(hasKatex(container)).toBe(true);
  });

  test('switching variables clears stale LaTeX while the new request is in flight', async () => {
    const calls: Array<{ ident: string; deferred: Deferred<string | undefined> }> = [];
    const getLatexEquation = jest.fn((ident: string) => {
      const deferred = createDeferred<string | undefined>();
      calls.push({ ident, deferred });
      return deferred.promise;
    });

    const { container, rerender } = renderDetails('var_a', getLatexEquation);

    await act(async () => {
      calls[0].deferred.resolve('\\text{var a}');
    });
    expect(hasKatex(container)).toBe(true);

    // Switch to var_b: the stale KaTeX is cleared immediately and the raw
    // equation ('1') shows as plain text while var_b's request is in flight.
    rerenderDetails(rerender, 'var_b', getLatexEquation);
    expect(hasKatex(container)).toBe(false);
    expect(plainText(container)).toBe('1');

    await act(async () => {
      calls[1].deferred.resolve('\\text{var b}');
    });
    expect(hasKatex(container)).toBe(true);
  });
});
