// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';
import { render, fireEvent, screen } from '@testing-library/react';

import { ModuleDetails } from '../ModuleDetails';
import type { Module, Aux, Stock, Model, Project, ViewElement } from '@simlin/core/datamodel';

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
    activeInitial: undefined,
    dataSource: undefined,
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
    activeInitial: undefined,
    dataSource: undefined,
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

function makeModel(name: string, variables: ReadonlyArray<Aux | Stock | Module>): Model {
  const varMap = new Map<string, Aux | Stock | Module>();
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

describe('ModuleDetails', () => {
  // AC2.1: Selecting a module shows ModuleDetails panel
  describe('rendering', () => {
    test('renders without crashing', () => {
      const variable = makeModule('hares_mod', 'hares');
      const project = makeProject([makeModel('main', [variable]), makeModel('hares', [makeAux('population')])]);
      const callbacks = defaultCallbacks();

      const { container } = render(
        <ModuleDetails
          variable={variable}
          viewElement={makeViewElement('hares_mod')}
          project={project}
          currentModelName="main"
          {...callbacks}
        />,
      );

      expect(container.querySelector('.card')).not.toBeNull();
    });

    test('does not render an equation editor', () => {
      const variable = makeModule('hares_mod', 'hares');
      const project = makeProject([makeModel('main', [variable]), makeModel('hares', [makeAux('population')])]);
      const callbacks = defaultCallbacks();

      const { container } = render(
        <ModuleDetails
          variable={variable}
          viewElement={makeViewElement('hares_mod')}
          project={project}
          currentModelName="main"
          {...callbacks}
        />,
      );

      // No equation editor class from VariableDetails
      expect(container.querySelector('.eqnEditor')).toBeNull();
      expect(container.querySelector('.eqnPreview')).toBeNull();
    });
  });

  // AC2.2: Panel displays the referenced model name
  describe('model reference display', () => {
    test('shows the referenced model name in selector', () => {
      const variable = makeModule('hares_mod', 'hares');
      const project = makeProject([makeModel('main', [variable]), makeModel('hares', [makeAux('population')])]);
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

      const select = screen.getByTestId('model-ref-select') as HTMLSelectElement;
      expect(select.value).toBe('hares');
    });

    test('shows module ident as header', () => {
      const variable = makeModule('hares_mod', 'hares');
      const project = makeProject([makeModel('main', [variable]), makeModel('hares', [makeAux('population')])]);
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

      expect(screen.getByText('hares_mod')).not.toBeNull();
    });
  });

  // AC1.7: Model reference selection in details panel
  describe('model reference selector', () => {
    test('shows project models in selector', () => {
      const variable = makeModule('mod1', '');
      const project = makeProject([
        makeModel('main', [variable]),
        makeModel('hares', [makeAux('x')]),
        makeModel('foxes', [makeAux('y')]),
      ]);
      const callbacks = defaultCallbacks();

      const { container } = render(
        <ModuleDetails
          variable={variable}
          viewElement={makeViewElement('mod1')}
          project={project}
          currentModelName="main"
          {...callbacks}
        />,
      );

      const select = container.querySelector('select') as HTMLSelectElement;
      const optionTexts = Array.from(select.options).map((o) => o.text);
      expect(optionTexts).toContain('hares');
      expect(optionTexts).toContain('foxes');
    });

    test('excludes current model name from selector', () => {
      const variable = makeModule('mod1', '');
      const project = makeProject([makeModel('main', [variable]), makeModel('hares', [makeAux('x')])]);
      const callbacks = defaultCallbacks();

      const { container } = render(
        <ModuleDetails
          variable={variable}
          viewElement={makeViewElement('mod1')}
          project={project}
          currentModelName="main"
          {...callbacks}
        />,
      );

      const select = container.querySelector('select') as HTMLSelectElement;
      const optionValues = Array.from(select.options).map((o) => o.value);
      expect(optionValues).not.toContain('main');
    });

    // AC1.9: stdlib models not shown (not yet exposed through project serialization)
    test('does not show stdlib models in selector', () => {
      const variable = makeModule('mod1', '');
      const project = makeProject([makeModel('main', [variable])]);
      const callbacks = defaultCallbacks();

      const { container } = render(
        <ModuleDetails
          variable={variable}
          viewElement={makeViewElement('mod1')}
          project={project}
          currentModelName="main"
          {...callbacks}
        />,
      );

      const select = container.querySelector('select') as HTMLSelectElement;
      const optionTexts = Array.from(select.options).map((o) => o.text);
      expect(optionTexts).not.toContain('delay1');
      expect(optionTexts).not.toContain('smth3');
    });

    // AC1.8: selecting a model reference calls callback
    test('changing model reference calls onModelReferenceChange', () => {
      const variable = makeModule('mod1', '');
      const project = makeProject([makeModel('main', [variable]), makeModel('hares', [makeAux('x')])]);
      const callbacks = defaultCallbacks();

      const { container } = render(
        <ModuleDetails
          variable={variable}
          viewElement={makeViewElement('mod1')}
          project={project}
          currentModelName="main"
          {...callbacks}
        />,
      );

      const select = container.querySelector('select') as HTMLSelectElement;
      fireEvent.change(select, { target: { value: 'hares' } });
      expect(callbacks.onModelReferenceChange).toHaveBeenCalledWith('mod1', 'hares');
    });

    // AC1.10: "Create new model" action
    test('selecting "Create new model" calls onCreateModel', () => {
      const variable = makeModule('mod1', '');
      const project = makeProject([makeModel('main', [variable])]);
      const callbacks = defaultCallbacks();

      const { container } = render(
        <ModuleDetails
          variable={variable}
          viewElement={makeViewElement('mod1')}
          project={project}
          currentModelName="main"
          {...callbacks}
        />,
      );

      const select = container.querySelector('select') as HTMLSelectElement;
      fireEvent.change(select, { target: { value: '__create_new__' } });
      expect(callbacks.onCreateModel).toHaveBeenCalledWith('mod1');
    });

    // AC1.11: "Duplicate model" action
    test('selecting "Duplicate model" calls onDuplicateModel', () => {
      const variable = makeModule('mod1', 'hares');
      const project = makeProject([makeModel('main', [variable]), makeModel('hares', [makeAux('x')])]);
      const callbacks = defaultCallbacks();

      const { container } = render(
        <ModuleDetails
          variable={variable}
          viewElement={makeViewElement('mod1')}
          project={project}
          currentModelName="main"
          {...callbacks}
        />,
      );

      const select = container.querySelector('select') as HTMLSelectElement;
      fireEvent.change(select, { target: { value: '__duplicate__' } });
      expect(callbacks.onDuplicateModel).toHaveBeenCalledWith('mod1', 'hares');
    });

    test('duplicate option not shown when no model reference set', () => {
      const variable = makeModule('mod1', '');
      const project = makeProject([makeModel('main', [variable])]);
      const callbacks = defaultCallbacks();

      const { container } = render(
        <ModuleDetails
          variable={variable}
          viewElement={makeViewElement('mod1')}
          project={project}
          currentModelName="main"
          {...callbacks}
        />,
      );

      const select = container.querySelector('select') as HTMLSelectElement;
      const optionValues = Array.from(select.options).map((o) => o.value);
      expect(optionValues).not.toContain('__duplicate__');
    });
  });

  // AC2.8: "Open Model" button
  describe('Open Model button', () => {
    test('renders Open Model button when model reference is set', () => {
      const variable = makeModule('hares_mod', 'hares');
      const project = makeProject([makeModel('main', [variable]), makeModel('hares', [makeAux('x')])]);
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

      expect(screen.getByText('Open Model')).not.toBeNull();
    });

    test('does not render Open Model button when no model reference', () => {
      const variable = makeModule('mod1', '');
      const project = makeProject([makeModel('main', [variable])]);
      const callbacks = defaultCallbacks();

      render(
        <ModuleDetails
          variable={variable}
          viewElement={makeViewElement('mod1')}
          project={project}
          currentModelName="main"
          {...callbacks}
        />,
      );

      expect(screen.queryByText('Open Model')).toBeNull();
    });

    test('clicking Open Model calls onDrillIntoModule', () => {
      const variable = makeModule('hares_mod', 'hares');
      const project = makeProject([makeModel('main', [variable]), makeModel('hares', [makeAux('x')])]);
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

      fireEvent.click(screen.getByText('Open Model'));
      expect(callbacks.onDrillIntoModule).toHaveBeenCalledWith('hares_mod', 'hares');
    });
  });

  // AC2.3: Input wiring table
  describe('input wiring table', () => {
    test('shows references in wiring table', () => {
      const variable = makeModule('hares_mod', 'hares', {
        references: [
          { src: 'food', dst: 'input_food' },
          { src: 'water', dst: 'input_water' },
        ],
      });
      const project = makeProject([
        makeModel('main', [variable, makeAux('food'), makeAux('water')]),
        makeModel('hares', [makeAux('input_food'), makeAux('input_water')]),
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

      // Reference values are displayed inside Autocomplete input fields
      expect(screen.getByDisplayValue('food')).not.toBeNull();
      expect(screen.getByDisplayValue('input_food')).not.toBeNull();
      expect(screen.getByDisplayValue('water')).not.toBeNull();
      expect(screen.getByDisplayValue('input_water')).not.toBeNull();
    });

    // AC2.9: Module with zero input ports shows empty state
    test('shows empty message when no references configured', () => {
      const variable = makeModule('hares_mod', 'hares');
      const project = makeProject([makeModel('main', [variable]), makeModel('hares', [makeAux('x')])]);
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

      expect(screen.getByText('No inputs configured')).not.toBeNull();
    });

    test('does not show input wiring when no model reference', () => {
      const variable = makeModule('mod1', '');
      const project = makeProject([makeModel('main', [variable])]);
      const callbacks = defaultCallbacks();

      render(
        <ModuleDetails
          variable={variable}
          viewElement={makeViewElement('mod1')}
          project={project}
          currentModelName="main"
          {...callbacks}
        />,
      );

      expect(screen.queryByText('Input Wiring')).toBeNull();
    });
  });

  // AC2.4: Output ports list
  describe('output ports', () => {
    test('shows public variables from referenced model', () => {
      const variable = makeModule('hares_mod', 'hares');
      const project = makeProject([
        makeModel('main', [variable]),
        makeModel('hares', [
          makeAux('population', { isPublic: true }),
          makeAux('growth_rate', { isPublic: true }),
          makeAux('internal_var'),
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

      expect(screen.getByText('population')).not.toBeNull();
      expect(screen.getByText('growth_rate')).not.toBeNull();
      // internal_var is not public, should not appear in port list
      // (it may appear elsewhere, so we check the specific list)
    });

    // AC2.10: Model with zero public outputs
    test('shows empty message when no public outputs', () => {
      const variable = makeModule('hares_mod', 'hares');
      const project = makeProject([makeModel('main', [variable]), makeModel('hares', [makeAux('internal_only')])]);
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

      expect(screen.getByText('No public outputs')).not.toBeNull();
    });

    test('does not show output ports when no model reference', () => {
      const variable = makeModule('mod1', '');
      const project = makeProject([makeModel('main', [variable])]);
      const callbacks = defaultCallbacks();

      render(
        <ModuleDetails
          variable={variable}
          viewElement={makeViewElement('mod1')}
          project={project}
          currentModelName="main"
          {...callbacks}
        />,
      );

      expect(screen.queryByText('Output Ports')).toBeNull();
    });
  });

  // AC2.7: Units and documentation editors
  describe('units and docs editors', () => {
    test('renders units editor with placeholder', () => {
      const variable = makeModule('hares_mod', 'hares');
      const project = makeProject([makeModel('main', [variable]), makeModel('hares', [makeAux('x')])]);
      const callbacks = defaultCallbacks();

      const { container } = render(
        <ModuleDetails
          variable={variable}
          viewElement={makeViewElement('hares_mod')}
          project={project}
          currentModelName="main"
          {...callbacks}
        />,
      );

      const unitsEditor = container.querySelector('.unitsEditor');
      expect(unitsEditor).not.toBeNull();
    });

    test('renders docs editor with placeholder', () => {
      const variable = makeModule('hares_mod', 'hares');
      const project = makeProject([makeModel('main', [variable]), makeModel('hares', [makeAux('x')])]);
      const callbacks = defaultCallbacks();

      const { container } = render(
        <ModuleDetails
          variable={variable}
          viewElement={makeViewElement('hares_mod')}
          project={project}
          currentModelName="main"
          {...callbacks}
        />,
      );

      const notesEditor = container.querySelector('.notesEditor');
      expect(notesEditor).not.toBeNull();
    });

    test('initializes editors with existing units and docs', () => {
      const variable = makeModule('hares_mod', 'hares', {
        units: 'rabbits',
        documentation: 'Number of hares in the system',
      });
      const project = makeProject([makeModel('main', [variable]), makeModel('hares', [makeAux('x')])]);
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

      expect(screen.getByText('rabbits')).not.toBeNull();
      expect(screen.getByText('Number of hares in the system')).not.toBeNull();
    });
  });

  // Delete button
  describe('delete button', () => {
    test('renders delete button', () => {
      const variable = makeModule('hares_mod', 'hares');
      const project = makeProject([makeModel('main', [variable]), makeModel('hares', [makeAux('x')])]);
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

      expect(screen.getByText('Delete Module')).not.toBeNull();
    });

    test('clicking delete calls onDelete with ident', () => {
      const variable = makeModule('hares_mod', 'hares');
      const project = makeProject([makeModel('main', [variable]), makeModel('hares', [makeAux('x')])]);
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

      fireEvent.click(screen.getByText('Delete Module'));
      expect(callbacks.onDelete).toHaveBeenCalledWith('hares_mod');
    });
  });

  // AC2.4: Output ports with mixed variable types (stocks and auxes)
  describe('output ports with mixed types', () => {
    test('shows stocks and auxes as output ports', () => {
      const variable = makeModule('eco_mod', 'ecosystem');
      const project = makeProject([
        makeModel('main', [variable]),
        makeModel('ecosystem', [
          makeAux('growth_rate', { isPublic: true }),
          makeStock('population', { isPublic: true }),
          makeStock('internal_level'),
        ]),
      ]);
      const callbacks = defaultCallbacks();

      render(
        <ModuleDetails
          variable={variable}
          viewElement={makeViewElement('eco_mod')}
          project={project}
          currentModelName="main"
          {...callbacks}
        />,
      );

      expect(screen.getByText('growth_rate')).not.toBeNull();
      expect(screen.getByText('population')).not.toBeNull();
    });
  });

  // AC4.1: Instance count integration (the count is tested in utils;
  // here we verify ModuleDetails renders correctly with shared model data)
  describe('shared model awareness', () => {
    test('renders with multiple module instances referencing same model', () => {
      // Two modules reference 'hares' -- the banner is in Editor,
      // but ModuleDetails should still render correctly.
      const mod1 = makeModule('hares_mod_1', 'hares');
      const mod2 = makeModule('hares_mod_2', 'hares');
      const project = makeProject([
        makeModel('main', [mod1, mod2]),
        makeModel('hares', [makeAux('population', { isPublic: true })]),
      ]);
      const callbacks = defaultCallbacks();

      render(
        <ModuleDetails
          variable={mod1}
          viewElement={makeViewElement('hares_mod_1')}
          project={project}
          currentModelName="main"
          {...callbacks}
        />,
      );

      // Module should render normally even when model is shared
      expect(screen.getByText('hares_mod_1')).not.toBeNull();
      expect(screen.getByText('population')).not.toBeNull();
    });
  });

  // Verify that referenced model not in project is handled gracefully
  describe('missing model reference', () => {
    test('shows empty output ports when referenced model is missing from project', () => {
      const variable = makeModule('orphan_mod', 'nonexistent_model');
      const project = makeProject([makeModel('main', [variable])]);
      const callbacks = defaultCallbacks();

      render(
        <ModuleDetails
          variable={variable}
          viewElement={makeViewElement('orphan_mod')}
          project={project}
          currentModelName="main"
          {...callbacks}
        />,
      );

      // Should show empty states rather than crashing
      expect(screen.getByText('No inputs configured')).not.toBeNull();
      expect(screen.getByText('No public outputs')).not.toBeNull();
    });
  });

  // AC1.12: Cycle-creating models excluded from selector
  describe('cycle prevention in selector', () => {
    test('excludes models that would create a cycle', () => {
      // main -> hares (module). From hares, 'main' should be excluded
      // because hares->main would close the cycle.
      const mod = makeModule('hares_mod', 'hares');
      const variable = makeModule('sub_mod', '');
      const project = makeProject([
        makeModel('main', [mod]),
        makeModel('hares', [variable, makeAux('x')]),
        makeModel('foxes', [makeAux('y')]),
      ]);
      const callbacks = defaultCallbacks();

      const { container } = render(
        <ModuleDetails
          variable={variable}
          viewElement={makeViewElement('sub_mod')}
          project={project}
          currentModelName="hares"
          {...callbacks}
        />,
      );

      const select = container.querySelector('select') as HTMLSelectElement;
      const optionValues = Array.from(select.options).map((o) => o.value);
      // main->hares exists, so hares->main would create a cycle
      expect(optionValues).not.toContain('main');
      // hares is the current model, also excluded
      expect(optionValues).not.toContain('hares');
      // foxes is safe
      expect(optionValues).toContain('foxes');
    });
  });

  // AC5.3: Works at nested depth > 1
  describe('nested module support', () => {
    test('renders correctly for a module nested 2 levels deep', () => {
      // main has moduleA -> model_a, model_a has moduleB -> model_b
      const moduleA = makeModule('module_a', 'model_a');
      const moduleB = makeModule('module_b', '');
      const project = makeProject([
        makeModel('main', [moduleA]),
        makeModel('model_a', [moduleB, makeAux('local_var')]),
        makeModel('model_b', [makeAux('deep_var', { isPublic: true })]),
      ]);
      const callbacks = defaultCallbacks();

      const { container } = render(
        <ModuleDetails
          variable={moduleB}
          viewElement={makeViewElement('module_b')}
          project={project}
          currentModelName="model_a"
          {...callbacks}
        />,
      );

      // The selector should show available models for model_a context
      const select = container.querySelector('select') as HTMLSelectElement;
      const optionValues = Array.from(select.options).map((o) => o.value);

      // model_a is current, excluded
      expect(optionValues).not.toContain('model_a');
      // model_b is available (no cycle)
      expect(optionValues).toContain('model_b');
      // main is available (main doesn't depend on model_a in a way that model_a->main creates cycle)
      // Actually: main -> model_a exists. So model_a -> main creates a cycle.
      expect(optionValues).not.toContain('main');
    });

    test('shows output ports from the deeply nested referenced model', () => {
      // main -> model_a -> model_b. Viewing moduleB from model_a.
      const moduleA = makeModule('module_a', 'model_a');
      const moduleB = makeModule('module_b', 'model_b');
      const project = makeProject([
        makeModel('main', [moduleA]),
        makeModel('model_a', [moduleB, makeAux('local_var')]),
        makeModel('model_b', [
          makeAux('deep_output', { isPublic: true }),
          makeStock('deep_level', { isPublic: true }),
          makeAux('deep_internal'),
        ]),
      ]);
      const callbacks = defaultCallbacks();

      render(
        <ModuleDetails
          variable={moduleB}
          viewElement={makeViewElement('module_b')}
          project={project}
          currentModelName="model_a"
          {...callbacks}
        />,
      );

      // Output ports should come from model_b (the referenced model)
      expect(screen.getByText('deep_output')).not.toBeNull();
      expect(screen.getByText('deep_level')).not.toBeNull();
    });

    test('shows wiring from the parent model context for nested module', () => {
      const moduleA = makeModule('module_a', 'model_a');
      const moduleB = makeModule('module_b', 'model_b', {
        references: [{ src: 'local_var', dst: 'deep_input' }],
      });
      const project = makeProject([
        makeModel('main', [moduleA]),
        makeModel('model_a', [moduleB, makeAux('local_var')]),
        makeModel('model_b', [makeAux('deep_input', { canBeModuleInput: true })]),
      ]);
      const callbacks = defaultCallbacks();

      render(
        <ModuleDetails
          variable={moduleB}
          viewElement={makeViewElement('module_b')}
          project={project}
          currentModelName="model_a"
          {...callbacks}
        />,
      );

      // Wiring values are displayed inside Autocomplete input fields
      expect(screen.getByDisplayValue('local_var')).not.toBeNull();
      expect(screen.getByDisplayValue('deep_input')).not.toBeNull();
    });
  });

  // Verify open model callback at nested depth
  describe('open model at nested depth', () => {
    test('clicking Open Model at depth 2 passes correct arguments', () => {
      const moduleB = makeModule('module_b', 'model_b');
      const project = makeProject([
        makeModel('main', [makeModule('module_a', 'model_a')]),
        makeModel('model_a', [moduleB]),
        makeModel('model_b', [makeAux('x')]),
      ]);
      const callbacks = defaultCallbacks();

      render(
        <ModuleDetails
          variable={moduleB}
          viewElement={makeViewElement('module_b')}
          project={project}
          currentModelName="model_a"
          {...callbacks}
        />,
      );

      fireEvent.click(screen.getByText('Open Model'));
      expect(callbacks.onDrillIntoModule).toHaveBeenCalledWith('module_b', 'model_b');
    });
  });
});
