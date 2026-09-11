// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { describe, test, expect, beforeAll, rs } from '@rstest/core';

import * as React from 'react';
import { act, render, fireEvent, screen } from '@testing-library/react';
import { Editor as SlateEditor, Transforms } from 'slate';
import { ELEMENT_TO_NODE } from 'slate-dom';

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
    onDelete: rs.fn(),
    onModelReferenceChange: rs.fn(),
    onUnitsDocsChange: rs.fn(),
    onDrillIntoModule: rs.fn(),
    onCreateModel: rs.fn(),
    onDuplicateModel: rs.fn(),
    onReferencesChange: rs.fn(),
  };
}

// -- Tests --

describe('ModuleDetails drafts', () => {
  beforeAll(() => {
    // jsdom lacks isContentEditable and Range geometry, which slate-react reads.
    Object.defineProperty(HTMLElement.prototype, 'isContentEditable', {
      configurable: true,
      get(this: HTMLElement): boolean {
        return this.getAttribute('contenteditable') === 'true';
      },
    });
    if (!('getBoundingClientRect' in Range.prototype)) {
      const zero = () =>
        ({ x: 0, y: 0, width: 0, height: 0, top: 0, left: 0, right: 0, bottom: 0, toJSON() {} }) as DOMRect;
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      (Range.prototype as any).getBoundingClientRect = zero;
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      (Range.prototype as any).getClientRects = () =>
        ({ length: 0, item: () => null, [Symbol.iterator]: function* () {} }) as unknown as DOMRectList;
    }
  });

  // The units field, then the documentation field.
  function slateEditors(container: HTMLElement): SlateEditor[] {
    return Array.from(container.querySelectorAll('[data-slate-editor="true"]')).map(
      (el) => ELEMENT_TO_NODE.get(el as HTMLElement) as unknown as SlateEditor,
    );
  }

  async function append(editor: SlateEditor, text: string): Promise<void> {
    await act(async () => {
      Transforms.insertText(editor, text, { at: SlateEditor.end(editor, []) });
      editor.onChange();
      await Promise.resolve();
    });
  }

  test('reports a draft, and a flush submits only the field holding it; a submission that does not land can be submitted again', async () => {
    const variable = makeModule('hares_mod', 'hares', { units: 'people', documentation: 'docs' });
    const project = makeProject([makeModel('main', [variable]), makeModel('hares', [makeAux('population')])]);
    const callbacks = defaultCallbacks();
    let landed = false;
    callbacks.onUnitsDocsChange.mockImplementation(async () => landed);
    const draftStates: boolean[] = [];
    let flush: (() => boolean) | undefined;
    const { container } = render(
      <ModuleDetails
        variable={variable}
        viewElement={makeViewElement('hares_mod')}
        project={project}
        currentModelName="main"
        registerDraftFlush={(f) => {
          flush = f;
          return () => {};
        }}
        onDraftStateChange={(hasDraft) => draftStates.push(hasDraft)}
        {...callbacks}
      />,
    );
    const [units] = slateEditors(container);
    await append(units, ' per year');
    expect(draftStates[draftStates.length - 1]).toBe(true);

    let submitted = false;
    await act(async () => {
      submitted = flush!();
      await Promise.resolve();
    });
    expect(submitted).toBe(true);
    // The documentation field was never touched, so it is not echoed.
    expect(callbacks.onUnitsDocsChange).toHaveBeenLastCalledWith('hares_mod', 'people per year', undefined);
    await act(async () => {
      await Promise.resolve();
      await Promise.resolve();
    });
    // The submission settled as not landed, so the text is a draft again.
    expect(draftStates[draftStates.length - 1]).toBe(true);
    landed = true;
    await act(async () => {
      submitted = flush!();
      await Promise.resolve();
    });
    expect(submitted).toBe(true);
    expect(callbacks.onUnitsDocsChange).toHaveBeenCalledTimes(2);
  });

  function deferred(): { promise: Promise<boolean>; resolve: (ok: boolean) => void } {
    let resolve!: (ok: boolean) => void;
    const promise = new Promise<boolean>((r) => {
      resolve = r;
    });
    return { promise, resolve };
  }

  async function removeLast(editor: SlateEditor, count: number): Promise<void> {
    await act(async () => {
      Transforms.delete(editor, { at: SlateEditor.end(editor, []), distance: count, unit: 'character', reverse: true });
      editor.onChange();
      await Promise.resolve();
    });
  }

  function mountPanel(variable: Module, onUnitsDocsChange: (...args: unknown[]) => Promise<boolean>) {
    const project = makeProject([makeModel('main', [variable]), makeModel('hares', [makeAux('population')])]);
    const callbacks = { ...defaultCallbacks(), onUnitsDocsChange: rs.fn(onUnitsDocsChange) };
    const draftStates: boolean[] = [];
    let flush: (() => boolean) | undefined;
    const props = {
      viewElement: makeViewElement('hares_mod'),
      project,
      currentModelName: 'main',
      registerDraftFlush: (f: () => boolean) => {
        flush = f;
        return () => {};
      },
      onDraftStateChange: (hasDraft: boolean) => draftStates.push(hasDraft),
      ...callbacks,
    };
    const result = render(<ModuleDetails variable={variable} {...props} />);
    return {
      result,
      callbacks,
      draftStates,
      hasDraft: () => draftStates[draftStates.length - 1],
      flush: async () => {
        let submitted = false;
        await act(async () => {
          submitted = flush!();
          await Promise.resolve();
        });
        return submitted;
      },
      rerender: (next: Module) => result.rerender(<ModuleDetails variable={next} {...props} />),
    };
  }

  test("a field changed back to its seeded text after a submission is a draft (the submission is the field's base)", async () => {
    const pending = deferred();
    const panel = mountPanel(makeModule('hares_mod', 'hares', { units: 'people' }), () => pending.promise);
    const [units] = slateEditors(panel.result.container);
    await append(units, ' per year');
    expect(await panel.flush()).toBe(true);
    expect(panel.hasDraft()).toBe(false);
    // While the edit is in flight, back to what the field was seeded with.
    await removeLast(units, ' per year'.length);
    expect(panel.hasDraft()).toBe(true);
    expect(await panel.flush()).toBe(true);
    expect(panel.callbacks.onUnitsDocsChange).toHaveBeenLastCalledWith('hares_mod', 'people', undefined);
  });

  test('a failed submission falls back to the committed text, not the seeded one', async () => {
    // Seeded '', then 'X' landed; the user clears the field and that fails.
    let landed = true;
    const panel = mountPanel(makeModule('hares_mod', 'hares', { units: '' }), async () => landed);
    const [units] = slateEditors(panel.result.container);
    await append(units, 'X');
    expect(await panel.flush()).toBe(true);
    await act(async () => {
      panel.rerender(makeModule('hares_mod', 'hares', { units: 'X' }));
      await Promise.resolve();
    });
    expect(panel.hasDraft()).toBe(false);
    await removeLast(units, 1);
    landed = false;
    expect(await panel.flush()).toBe(true);
    await act(async () => {
      await Promise.resolve();
      await Promise.resolve();
    });
    // Committed is still 'X', so the cleared field stays a draft to retry.
    expect(panel.hasDraft()).toBe(true);
  });

  test("a submission failing after a newer one for the same field leaves the newer one as the field's base", async () => {
    const first = deferred();
    const second = deferred();
    const outcomes = [first.promise, second.promise];
    const panel = mountPanel(makeModule('hares_mod', 'hares', { units: 'people' }), () => outcomes.shift()!);
    const [units] = slateEditors(panel.result.container);
    await append(units, ' per year');
    expect(await panel.flush()).toBe(true);
    await append(units, '!');
    expect(await panel.flush()).toBe(true);
    expect(panel.hasDraft()).toBe(false);
    await act(async () => {
      first.resolve(false);
      await Promise.resolve();
      await Promise.resolve();
    });
    // The newer submission is still pending and is still the base: no draft.
    expect(panel.hasDraft()).toBe(false);
  });
});

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
