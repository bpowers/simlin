/**
 * @jest-environment jsdom
 *
 * Copyright 2026 The Simlin Authors. All rights reserved.
 * Use of this source code is governed by the Apache License,
 * Version 2.0, that can be found in the LICENSE file.
 */

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

describe('VariableDetails latex loading', () => {
  test('loadLatex race condition: only latest request updates state', async () => {
    const calls: Array<{ ident: string; deferred: Deferred<string | undefined> }> = [];

    const getLatexEquation = jest.fn((ident: string) => {
      const deferred = createDeferred<string | undefined>();
      calls.push({ ident, deferred });
      return deferred.promise;
    });

    const ref = React.createRef<VariableDetails>();
    const noop = () => {};

    const { rerender } = render(
      <VariableDetails
        ref={ref}
        variable={makeAux('var_a')}
        viewElement={makeViewElement('var_a')}
        getLatexEquation={getLatexEquation}
        onDelete={noop}
        onEquationChange={noop}
        onTableChange={noop}
        activeTab={0}
        onActiveTabChange={noop}
      />,
    );

    // componentDidMount triggers loadLatex for var_a
    expect(calls.length).toBe(1);
    expect(calls[0].ident).toBe('var_a');

    // Simulate selecting a new variable before var_a's response arrives
    rerender(
      <VariableDetails
        ref={ref}
        variable={makeAux('var_b')}
        viewElement={makeViewElement('var_b')}
        getLatexEquation={getLatexEquation}
        onDelete={noop}
        onEquationChange={noop}
        onTableChange={noop}
        activeTab={0}
        onActiveTabChange={noop}
      />,
    );

    // componentDidUpdate triggers loadLatex for var_b
    expect(calls.length).toBe(2);
    expect(calls[1].ident).toBe('var_b');

    // Now resolve var_b first (the current request)
    await act(async () => {
      calls[1].deferred.resolve('\\text{var\\_b equation}');
    });

    expect(ref.current!.state.latexEquation).toBe('\\text{var\\_b equation}');
    expect(ref.current!.state.latexLoading).toBe(false);

    // Now resolve var_a (the stale request) -- should be ignored
    await act(async () => {
      calls[0].deferred.resolve('\\text{var\\_a equation}');
    });

    // State should NOT have been overwritten by the stale response
    expect(ref.current!.state.latexEquation).toBe('\\text{var\\_b equation}');
  });

  test('loadLatex race condition: stale error does not overwrite success', async () => {
    const calls: Array<{ ident: string; deferred: Deferred<string | undefined> }> = [];

    const getLatexEquation = jest.fn((ident: string) => {
      const deferred = createDeferred<string | undefined>();
      calls.push({ ident, deferred });
      return deferred.promise;
    });

    const ref = React.createRef<VariableDetails>();
    const noop = () => {};

    const { rerender } = render(
      <VariableDetails
        ref={ref}
        variable={makeAux('var_a')}
        viewElement={makeViewElement('var_a')}
        getLatexEquation={getLatexEquation}
        onDelete={noop}
        onEquationChange={noop}
        onTableChange={noop}
        activeTab={0}
        onActiveTabChange={noop}
      />,
    );

    rerender(
      <VariableDetails
        ref={ref}
        variable={makeAux('var_b')}
        viewElement={makeViewElement('var_b')}
        getLatexEquation={getLatexEquation}
        onDelete={noop}
        onEquationChange={noop}
        onTableChange={noop}
        activeTab={0}
        onActiveTabChange={noop}
      />,
    );

    expect(calls.length).toBe(2);

    // Resolve var_b successfully
    await act(async () => {
      calls[1].deferred.resolve('\\text{var\\_b}');
    });

    expect(ref.current!.state.latexEquation).toBe('\\text{var\\_b}');

    // Reject var_a with an error -- should be ignored
    await act(async () => {
      calls[0].deferred.reject(new Error('network error'));
    });

    // State should still show var_b's equation
    expect(ref.current!.state.latexEquation).toBe('\\text{var\\_b}');
    expect(ref.current!.state.latexLoading).toBe(false);
  });

  test('rapid variable changes: only final request matters', async () => {
    const calls: Array<{ ident: string; deferred: Deferred<string | undefined> }> = [];

    const getLatexEquation = jest.fn((ident: string) => {
      const deferred = createDeferred<string | undefined>();
      calls.push({ ident, deferred });
      return deferred.promise;
    });

    const ref = React.createRef<VariableDetails>();
    const noop = () => {};

    const { rerender } = render(
      <VariableDetails
        ref={ref}
        variable={makeAux('v1')}
        viewElement={makeViewElement('v1')}
        getLatexEquation={getLatexEquation}
        onDelete={noop}
        onEquationChange={noop}
        onTableChange={noop}
        activeTab={0}
        onActiveTabChange={noop}
      />,
    );

    // Rapidly switch through v2 and v3
    rerender(
      <VariableDetails
        ref={ref}
        variable={makeAux('v2')}
        viewElement={makeViewElement('v2')}
        getLatexEquation={getLatexEquation}
        onDelete={noop}
        onEquationChange={noop}
        onTableChange={noop}
        activeTab={0}
        onActiveTabChange={noop}
      />,
    );

    rerender(
      <VariableDetails
        ref={ref}
        variable={makeAux('v3')}
        viewElement={makeViewElement('v3')}
        getLatexEquation={getLatexEquation}
        onDelete={noop}
        onEquationChange={noop}
        onTableChange={noop}
        activeTab={0}
        onActiveTabChange={noop}
      />,
    );

    expect(calls.length).toBe(3);

    // Resolve in reverse order: v1 first, then v2, then v3
    await act(async () => {
      calls[0].deferred.resolve('latex_v1');
    });
    expect(ref.current!.state.latexEquation).toBeUndefined(); // stale, ignored

    await act(async () => {
      calls[1].deferred.resolve('latex_v2');
    });
    expect(ref.current!.state.latexEquation).toBeUndefined(); // stale, ignored

    await act(async () => {
      calls[2].deferred.resolve('latex_v3');
    });
    expect(ref.current!.state.latexEquation).toBe('latex_v3'); // current, accepted
  });

  test('switching variables clears stale latexEquation while new request is in flight', async () => {
    const calls: Array<{ ident: string; deferred: Deferred<string | undefined> }> = [];

    const getLatexEquation = jest.fn((ident: string) => {
      const deferred = createDeferred<string | undefined>();
      calls.push({ ident, deferred });
      return deferred.promise;
    });

    const ref = React.createRef<VariableDetails>();
    const noop = () => {};

    const { rerender } = render(
      <VariableDetails
        ref={ref}
        variable={makeAux('var_a')}
        viewElement={makeViewElement('var_a')}
        getLatexEquation={getLatexEquation}
        onDelete={noop}
        onEquationChange={noop}
        onTableChange={noop}
        activeTab={0}
        onActiveTabChange={noop}
      />,
    );

    // Resolve var_a's LaTeX
    await act(async () => {
      calls[0].deferred.resolve('\\text{var\\_a}');
    });
    expect(ref.current!.state.latexEquation).toBe('\\text{var\\_a}');
    expect(ref.current!.state.latexLoading).toBe(false);

    // Switch to var_b
    rerender(
      <VariableDetails
        ref={ref}
        variable={makeAux('var_b')}
        viewElement={makeViewElement('var_b')}
        getLatexEquation={getLatexEquation}
        onDelete={noop}
        onEquationChange={noop}
        onTableChange={noop}
        activeTab={0}
        onActiveTabChange={noop}
      />,
    );

    // While var_b's request is in flight, the old LaTeX should be cleared
    expect(ref.current!.state.latexEquation).toBeUndefined();
    expect(ref.current!.state.latexLoading).toBe(true);

    // Resolve var_b
    await act(async () => {
      calls[1].deferred.resolve('\\text{var\\_b}');
    });
    expect(ref.current!.state.latexEquation).toBe('\\text{var\\_b}');
    expect(ref.current!.state.latexLoading).toBe(false);
  });
});
