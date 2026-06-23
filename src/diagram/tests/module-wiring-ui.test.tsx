// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

/**
 * @jest-environment jsdom
 */

import * as React from 'react';
import { render, fireEvent, screen } from '@testing-library/react';

import { ModuleDetails } from '../ModuleDetails';
import type { Module, Aux, Stock, Flow, Model, Project, ViewElement } from '@simlin/core/datamodel';

// -- Test fixtures --

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

function makeStock(ident: string, overrides?: Partial<Stock>): Stock {
  return {
    type: 'stock',
    ident,
    equation: { type: 'scalar', equation: '0' },
    documentation: '',
    units: '',
    inflows: [],
    outflows: [],
    nonNegative: false,
    canBeModuleInput: false,
    isPublic: false,
    data: undefined,
    errors: undefined,
    unitErrors: undefined,
    uid: undefined,
    ...overrides,
  };
}

function makeFlow(ident: string, overrides?: Partial<Flow>): Flow {
  return {
    type: 'flow',
    ident,
    equation: { type: 'scalar', equation: '0' },
    documentation: '',
    units: '',
    gf: undefined,
    nonNegative: false,
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
    data: undefined,
    errors: undefined,
    unitErrors: undefined,
    uid: undefined,
    ...overrides,
  };
}

function makeModel(name: string, variables: ReadonlyArray<Aux | Stock | Flow | Module>): Model {
  const varMap = new Map<string, Aux | Stock | Flow | Module>();
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
    name: 'test_project',
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
    onDelete: jest.fn(),
    onModelReferenceChange: jest.fn(),
    onUnitsDocsChange: jest.fn(),
    onDrillIntoModule: jest.fn(),
    onCreateModel: jest.fn(),
    onDuplicateModel: jest.fn(),
    onReferencesChange: jest.fn(),
  };
}

// -- Tests --

describe('ModuleDetails wiring editor', () => {
  // AC2.5: User can add a new input reference
  describe('add reference', () => {
    test('clicking Add Input calls onReferencesChange with empty src/dst appended', () => {
      const variable = makeModule('hares_mod', 'hares');
      const project = makeProject([
        makeModel('main', [variable, makeAux('food')]),
        makeModel('hares', [makeAux('input_food', { canBeModuleInput: true })]),
      ]);
      const callbacks = defaultCallbacks();

      render(
        <ModuleDetails
          variable={variable}
          viewElement={makeViewElement('hares_mod')}
          project={project}
          currentModelName="main"
          {...callbacks}
        />,
      );

      fireEvent.click(screen.getByText('Add Input'));

      expect(callbacks.onReferencesChange).toHaveBeenCalledTimes(1);
      expect(callbacks.onReferencesChange).toHaveBeenCalledWith('hares_mod', [{ src: '', dst: '' }]);
    });

    test('adding to existing references appends without modifying existing entries', () => {
      const variable = makeModule('hares_mod', 'hares', {
        references: [{ src: 'food', dst: 'input_food' }],
      });
      const project = makeProject([
        makeModel('main', [variable, makeAux('food')]),
        makeModel('hares', [makeAux('input_food', { canBeModuleInput: true })]),
      ]);
      const callbacks = defaultCallbacks();

      render(
        <ModuleDetails
          variable={variable}
          viewElement={makeViewElement('hares_mod')}
          project={project}
          currentModelName="main"
          {...callbacks}
        />,
      );

      fireEvent.click(screen.getByText('Add Input'));

      expect(callbacks.onReferencesChange).toHaveBeenCalledWith('hares_mod', [
        { src: 'food', dst: 'input_food' },
        { src: '', dst: '' },
      ]);
    });
  });

  // AC2.5: Select src variable via Autocomplete dropdown
  describe('select src variable', () => {
    test('selecting a src option from the dropdown calls onReferencesChange with updated src', () => {
      const variable = makeModule('hares_mod', 'hares', {
        references: [{ src: '', dst: '' }],
      });
      const project = makeProject([
        makeModel('main', [variable, makeAux('food'), makeStock('water')]),
        makeModel('hares', [makeAux('input_food', { canBeModuleInput: true })]),
      ]);
      const callbacks = defaultCallbacks();

      render(
        <ModuleDetails
          variable={variable}
          viewElement={makeViewElement('hares_mod')}
          project={project}
          currentModelName="main"
          {...callbacks}
        />,
      );

      // The src input has placeholder "Select variable"
      const srcInput = screen.getByPlaceholderText('Select variable');

      // Type to filter and trigger downshift to open the dropdown
      fireEvent.change(srcInput, { target: { value: 'food' } });

      // The dropdown is rendered via portal to document.body.
      // Find the matching option in the listbox and click it.
      const foodOption = screen.getByText('food');
      fireEvent.click(foodOption);

      expect(callbacks.onReferencesChange).toHaveBeenCalledWith('hares_mod', [{ src: 'food', dst: '' }]);
    });
  });

  // AC2.5: Select dst variable via Autocomplete dropdown
  describe('select dst variable', () => {
    test('selecting a dst option from the dropdown calls onReferencesChange with updated dst', () => {
      const variable = makeModule('hares_mod', 'hares', {
        references: [{ src: 'food', dst: '' }],
      });
      const project = makeProject([
        makeModel('main', [variable, makeAux('food')]),
        makeModel('hares', [makeAux('input_food', { canBeModuleInput: true })]),
      ]);
      const callbacks = defaultCallbacks();

      render(
        <ModuleDetails
          variable={variable}
          viewElement={makeViewElement('hares_mod')}
          project={project}
          currentModelName="main"
          {...callbacks}
        />,
      );

      // The dst input has placeholder "Select input"
      const dstInput = screen.getByPlaceholderText('Select input');

      fireEvent.change(dstInput, { target: { value: 'input_food' } });

      const dstOption = screen.getByText('input_food');
      fireEvent.click(dstOption);

      // The bare dropdown port is persisted in the canonical module-qualified
      // form the engine wires against ({moduleIdent}·{port}).
      expect(callbacks.onReferencesChange).toHaveBeenCalledWith('hares_mod', [
        { src: 'food', dst: 'hares_mod·input_food' },
      ]);
    });

    test('a module-qualified dst is displayed as the bare port name', () => {
      const variable = makeModule('hares_mod', 'hares', {
        references: [{ src: 'food', dst: 'hares_mod·input_food' }],
      });
      const project = makeProject([
        makeModel('main', [variable, makeAux('food')]),
        makeModel('hares', [makeAux('input_food', { canBeModuleInput: true })]),
      ]);
      const callbacks = defaultCallbacks();

      render(
        <ModuleDetails
          variable={variable}
          viewElement={makeViewElement('hares_mod')}
          project={project}
          currentModelName="main"
          {...callbacks}
        />,
      );

      // The qualified dst renders as the bare port, not the raw `hares_mod·input_food`.
      const dstInput = screen.getByPlaceholderText('Select input') as HTMLInputElement;
      expect(dstInput.value).toBe('input_food');
    });
  });

  // AC2.6: User can remove an existing input reference
  describe('remove reference', () => {
    test('clicking remove on first row calls onReferencesChange with that row removed', () => {
      const variable = makeModule('hares_mod', 'hares', {
        references: [
          { src: 'food', dst: 'input_food' },
          { src: 'water', dst: 'input_water' },
        ],
      });
      const project = makeProject([
        makeModel('main', [variable, makeAux('food'), makeAux('water')]),
        makeModel('hares', [
          makeAux('input_food', { canBeModuleInput: true }),
          makeAux('input_water', { canBeModuleInput: true }),
        ]),
      ]);
      const callbacks = defaultCallbacks();

      render(
        <ModuleDetails
          variable={variable}
          viewElement={makeViewElement('hares_mod')}
          project={project}
          currentModelName="main"
          {...callbacks}
        />,
      );

      // Each row has a remove button with aria-label "Remove reference"
      const removeButtons = screen.getAllByLabelText('Remove reference');
      expect(removeButtons).toHaveLength(2);

      // Click the first remove button
      fireEvent.click(removeButtons[0]);

      expect(callbacks.onReferencesChange).toHaveBeenCalledTimes(1);
      expect(callbacks.onReferencesChange).toHaveBeenCalledWith('hares_mod', [{ src: 'water', dst: 'input_water' }]);
    });

    test('clicking remove on second row preserves first row', () => {
      const variable = makeModule('hares_mod', 'hares', {
        references: [
          { src: 'food', dst: 'input_food' },
          { src: 'water', dst: 'input_water' },
        ],
      });
      const project = makeProject([
        makeModel('main', [variable, makeAux('food'), makeAux('water')]),
        makeModel('hares', [
          makeAux('input_food', { canBeModuleInput: true }),
          makeAux('input_water', { canBeModuleInput: true }),
        ]),
      ]);
      const callbacks = defaultCallbacks();

      render(
        <ModuleDetails
          variable={variable}
          viewElement={makeViewElement('hares_mod')}
          project={project}
          currentModelName="main"
          {...callbacks}
        />,
      );

      const removeButtons = screen.getAllByLabelText('Remove reference');
      fireEvent.click(removeButtons[1]);

      expect(callbacks.onReferencesChange).toHaveBeenCalledWith('hares_mod', [{ src: 'food', dst: 'input_food' }]);
    });

    test('removing only reference results in empty array', () => {
      const variable = makeModule('hares_mod', 'hares', {
        references: [{ src: 'food', dst: 'input_food' }],
      });
      const project = makeProject([
        makeModel('main', [variable, makeAux('food')]),
        makeModel('hares', [makeAux('input_food', { canBeModuleInput: true })]),
      ]);
      const callbacks = defaultCallbacks();

      render(
        <ModuleDetails
          variable={variable}
          viewElement={makeViewElement('hares_mod')}
          project={project}
          currentModelName="main"
          {...callbacks}
        />,
      );

      const removeButton = screen.getByLabelText('Remove reference');
      fireEvent.click(removeButton);

      expect(callbacks.onReferencesChange).toHaveBeenCalledWith('hares_mod', []);
    });
  });

  // Dropdown options verification
  describe('dropdown options', () => {
    test('src dropdown contains parent model stocks, flows, and auxes', () => {
      const variable = makeModule('hares_mod', 'hares', {
        references: [{ src: '', dst: '' }],
      });
      const project = makeProject([
        makeModel('main', [variable, makeAux('aux_var'), makeStock('stock_var'), makeFlow('flow_var')]),
        makeModel('hares', [makeAux('input_a', { canBeModuleInput: true })]),
      ]);
      const callbacks = defaultCallbacks();

      render(
        <ModuleDetails
          variable={variable}
          viewElement={makeViewElement('hares_mod')}
          project={project}
          currentModelName="main"
          {...callbacks}
        />,
      );

      // Focus the src input to open the dropdown
      const srcInput = screen.getByPlaceholderText('Select variable');
      fireEvent.click(srcInput);

      // The portal-rendered dropdown uses <li> elements for options.
      // When the input is empty, all options show.
      const listItems = document.querySelectorAll('li');
      const listItemTexts = Array.from(listItems).map((li) => li.textContent);
      expect(listItemTexts).toContain('aux_var');
      expect(listItemTexts).toContain('flow_var');
      expect(listItemTexts).toContain('stock_var');
    });

    test('src dropdown excludes modules from parent model', () => {
      // The parent model has a module variable -- it must not appear in the
      // src dropdown because modules cannot be wired as inputs.
      const otherModule = makeModule('other_mod', 'other');
      const variable = makeModule('hares_mod', 'hares', {
        references: [{ src: '', dst: '' }],
      });
      const project = makeProject([
        makeModel('main', [variable, otherModule, makeAux('aux_var')]),
        makeModel('hares', [makeAux('input_a', { canBeModuleInput: true })]),
        makeModel('other', []),
      ]);
      const callbacks = defaultCallbacks();

      render(
        <ModuleDetails
          variable={variable}
          viewElement={makeViewElement('hares_mod')}
          project={project}
          currentModelName="main"
          {...callbacks}
        />,
      );

      const srcInput = screen.getByPlaceholderText('Select variable');
      fireEvent.click(srcInput);

      // The portal-rendered dropdown list items use <li> elements
      const listItems = document.querySelectorAll('li');
      const listItemTexts = Array.from(listItems).map((li) => li.textContent);

      // aux_var should be present
      expect(listItemTexts).toContain('aux_var');

      // Module variables must NOT appear in src options
      expect(listItemTexts).not.toContain('other_mod');
      expect(listItemTexts).not.toContain('hares_mod');
    });

    test('dst dropdown contains only variables with canBeModuleInput: true', () => {
      const variable = makeModule('hares_mod', 'hares', {
        references: [{ src: 'food', dst: '' }],
      });
      const project = makeProject([
        makeModel('main', [variable, makeAux('food')]),
        makeModel('hares', [
          makeAux('input_a', { canBeModuleInput: true }),
          makeAux('internal_only'),
          makeStock('input_stock', { canBeModuleInput: true }),
        ]),
      ]);
      const callbacks = defaultCallbacks();

      render(
        <ModuleDetails
          variable={variable}
          viewElement={makeViewElement('hares_mod')}
          project={project}
          currentModelName="main"
          {...callbacks}
        />,
      );

      const dstInput = screen.getByPlaceholderText('Select input');
      fireEvent.click(dstInput);

      // Only canBeModuleInput variables should appear in the dropdown
      const listItems = document.querySelectorAll('li');
      const listItemTexts = Array.from(listItems).map((li) => li.textContent);
      expect(listItemTexts).toContain('input_a');
      expect(listItemTexts).toContain('input_stock');
      expect(listItemTexts).not.toContain('internal_only');
    });
  });

  // Multiple rows interaction
  describe('multiple reference rows', () => {
    test('each row has independent src and dst values displayed', () => {
      const variable = makeModule('hares_mod', 'hares', {
        references: [
          { src: 'food', dst: 'input_food' },
          { src: 'water', dst: 'input_water' },
        ],
      });
      const project = makeProject([
        makeModel('main', [variable, makeAux('food'), makeAux('water')]),
        makeModel('hares', [
          makeAux('input_food', { canBeModuleInput: true }),
          makeAux('input_water', { canBeModuleInput: true }),
        ]),
      ]);
      const callbacks = defaultCallbacks();

      render(
        <ModuleDetails
          variable={variable}
          viewElement={makeViewElement('hares_mod')}
          project={project}
          currentModelName="main"
          {...callbacks}
        />,
      );

      // All four values should be visible as input values
      expect(screen.getByDisplayValue('food')).toBeTruthy();
      expect(screen.getByDisplayValue('input_food')).toBeTruthy();
      expect(screen.getByDisplayValue('water')).toBeTruthy();
      expect(screen.getByDisplayValue('input_water')).toBeTruthy();

      // Two remove buttons (one per row)
      expect(screen.getAllByLabelText('Remove reference')).toHaveLength(2);
    });
  });
});
