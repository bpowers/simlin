// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// ModuleDetails in read-only mode (issue #935): the panel opens for INSPECTION
// -- model reference, wiring, output ports, units/docs, and drill-in navigation
// all stay visible -- but every editing affordance (reference selector, wiring
// add/remove/edit, units/docs editing, module delete) is inert or absent.

import { describe, test, expect, beforeAll, rs } from '@rstest/core';

beforeAll(() => {
  // slate-react gates on element.isContentEditable, which jsdom lacks.
  Object.defineProperty(HTMLElement.prototype, 'isContentEditable', {
    configurable: true,
    get(this: HTMLElement): boolean {
      return this.getAttribute('contenteditable') === 'true';
    },
  });
});

import * as React from 'react';
import { render, screen, fireEvent } from '@testing-library/react';

import { ModuleDetails } from '../ModuleDetails';
import type { Aux, Model, Module, Project, ViewElement } from '@simlin/core/datamodel';

function makeAux(ident: string, overrides?: Partial<Aux>): Aux {
  return {
    type: 'aux',
    ident,
    equation: { type: 'scalar', equation: '0' },
    documentation: '',
    units: '',
    gf: undefined,
    canBeModuleInput: false,
    isPublic: false,
    data: undefined,
    errors: undefined,
    unitErrors: undefined,
    uid: undefined,
    ...overrides,
  };
}

function makeModule(ident: string, modelName: string, overrides?: Partial<Module>): Module {
  return {
    type: 'module',
    ident,
    modelName,
    documentation: '',
    units: '',
    references: [],
    canBeModuleInput: false,
    isPublic: false,
    dataSource: undefined,
    data: undefined,
    errors: undefined,
    unitErrors: undefined,
    uid: undefined,
    ...overrides,
  };
}

function makeModel(name: string, variables: ReadonlyArray<Aux | Module>): Model {
  const varMap = new Map<string, Aux | Module>();
  for (const v of variables) {
    varMap.set(v.ident, v);
  }
  return {
    name,
    variables: varMap,
    views: [],
    loopMetadata: [],
    groups: [],
  };
}

function makeProject(models: ReadonlyArray<Model>): Project {
  const modelMap = new Map<string, Model>();
  for (const m of models) {
    modelMap.set(m.name, m);
  }
  return {
    name: 'test',
    simSpecs: {
      start: 0,
      stop: 10,
      dt: { value: 1, isReciprocal: false },
      saveStep: undefined,
      simMethod: 'euler',
      timeUnits: undefined,
    },
    models: modelMap,
    dimensions: new Map(),
    hasNoEquations: false,
    source: undefined,
  };
}

function makeViewElement(ident: string): ViewElement {
  return {
    type: 'module',
    uid: 1,
    ident,
    x: 100,
    y: 100,
    isZeroRadius: false,
    labelSide: 'bottom',
  } as ViewElement;
}

function defaultCallbacks() {
  return {
    onDelete: rs.fn(),
    onModelReferenceChange: rs.fn(),
    onUnitsDocsChange: rs.fn(),
    onDrillIntoModule: rs.fn(),
    onCreateModel: rs.fn(),
    onDuplicateModel: rs.fn(),
    onReferencesChange: rs.fn(),
  };
}

// A module wired to a child model with one input reference, so every wiring
// affordance is exercisable.
function renderWired(readOnly: boolean) {
  const variable = makeModule('hares_mod', 'hares', {
    references: [{ src: 'birth_rate', dst: 'hares_mod·population' }],
  });
  const parentVars = [variable, makeAux('birth_rate', { canBeModuleInput: true })];
  const childVars = [makeAux('population', { canBeModuleInput: true, isPublic: true })];
  const project = makeProject([makeModel('main', parentVars), makeModel('hares', childVars)]);
  const callbacks = defaultCallbacks();
  const result = render(
    <ModuleDetails
      variable={variable}
      viewElement={makeViewElement('hares_mod')}
      project={project}
      currentModelName="main"
      readOnly={readOnly || undefined}
      {...callbacks}
    />,
  );
  return { container: result.container, callbacks };
}

// The custom Button component does not forward data-testid, so the Add Input
// affordance is found by its visible text.
function buttonTexts(container: HTMLElement): string[] {
  return Array.from(container.querySelectorAll('button')).map((b) => (b.textContent ?? '').trim());
}

describe('ModuleDetails read-only mode', () => {
  test('editable mode offers wiring + delete affordances (control)', () => {
    const { container } = renderWired(false);
    expect(buttonTexts(container)).toContain('Add Input');
    expect(screen.queryAllByLabelText('Remove reference').length).toBe(1);
    expect(buttonTexts(container)).toContain('Delete Module');
    const select = screen.getByTestId('model-ref-select') as HTMLSelectElement;
    expect(select.disabled).toBe(false);
  });

  test('disables the model-reference selector', () => {
    renderWired(true);
    const select = screen.getByTestId('model-ref-select') as HTMLSelectElement;
    expect(select.disabled).toBe(true);
  });

  test('hides Add Input, remove buttons, and Delete Module', () => {
    const { container } = renderWired(true);
    expect(buttonTexts(container)).not.toContain('Add Input');
    expect(screen.queryAllByLabelText('Remove reference').length).toBe(0);
    expect(buttonTexts(container)).not.toContain('Delete Module');
  });

  test('renders the wiring as static text instead of dropdowns', () => {
    const { container } = renderWired(true);
    // No combobox inputs inside the wiring table; the values render as text.
    const table = container.querySelector('.wiringTable');
    expect(table).not.toBeNull();
    expect(table!.querySelectorAll('input').length).toBe(0);
    expect(table!.textContent).toContain('birth_rate');
    expect(table!.textContent).toContain('population');
  });

  test('keeps Open Model (drill-in is inspection, not mutation)', () => {
    const { container, callbacks } = renderWired(true);
    const open = Array.from(container.querySelectorAll('button')).find((b) => b.textContent === 'Open Model');
    expect(open).toBeTruthy();
    fireEvent.click(open as HTMLButtonElement);
    expect(callbacks.onDrillIntoModule).toHaveBeenCalledWith('hares_mod', 'hares');
  });

  test('units and documentation editors are non-editable and never blur-save', () => {
    const { container, callbacks } = renderWired(true);
    const units = container.querySelector('.unitsEditor');
    const notes = container.querySelector('.notesEditor');
    expect(units).not.toBeNull();
    expect(notes).not.toBeNull();
    expect(units!.getAttribute('contenteditable')).toBe('false');
    expect(notes!.getAttribute('contenteditable')).toBe('false');
    fireEvent.blur(units as Element);
    fireEvent.blur(notes as Element);
    expect(callbacks.onUnitsDocsChange).not.toHaveBeenCalled();
  });
});
