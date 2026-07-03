// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import {
  graphicalFunctionScaleFromJson,
  graphicalFunctionScaleToJson,
  graphicalFunctionFromJson,
  graphicalFunctionToJson,
  stockFromJson,
  stockToJson,
  flowFromJson,
  flowToJson,
  auxFromJson,
  auxToJson,
  moduleFromJson,
  moduleToJson,
  stockViewElementFromJson,
  flowViewElementFromJson,
  linkViewElementFromJson,
  cloudViewElementFromJson,
  stockFlowViewFromJson,
  stockFlowViewToJson,
  findNonFiniteViewCoord,
  simSpecsFromJson,
  simSpecsToJson,
  dimensionFromJson,
  dimensionToJson,
  sourceFromJson,
  sourceToJson,
  loopMetadataFromJson,
  loopMetadataToJson,
  macroSpecFromJson,
  macroSpecToJson,
  modelFromJson,
  modelToJson,
  projectFromJson,
  projectToJson,
  projectAttachData,
  variableHasError,
  ErrorCode,
} from '../datamodel';

import type {
  GraphicalFunctionScale,
  GraphicalFunction,
  Stock,
  Flow,
  Aux,
  Module,
  ScalarEquation,
  ApplyToAllEquation,
  ArrayedEquation,
  ArrayedElement,
  DataSource,
  SimSpecs,
  Dimension,
  Source,
  StockFlowView,
  StockViewElement,
  FlowViewElement,
  LinkViewElement,
  CloudViewElement,
  AuxViewElement,
  ViewElement,
  Point,
  LoopMetadata,
  MacroSpec,
  Model,
  Project,
  Variable,
} from '../datamodel';
import type {
  JsonStockViewElement,
  JsonFlowViewElement,
  JsonLinkViewElement,
  JsonCloudViewElement,
  JsonStock,
  JsonAuxiliary,
  JsonModule,
} from '@simlin/engine';
import { defined, type Series } from '../common';

describe('GraphicalFunctionScale', () => {
  it('should roundtrip correctly', () => {
    const scale: GraphicalFunctionScale = { min: -10, max: 100 };
    const json = graphicalFunctionScaleToJson(scale);
    const restored = graphicalFunctionScaleFromJson(json);
    expect(restored.min).toBe(scale.min);
    expect(restored.max).toBe(scale.max);
  });
});

describe('GraphicalFunction', () => {
  it('should roundtrip with points', () => {
    const gf: GraphicalFunction = {
      kind: 'continuous',
      xPoints: [0, 1, 2],
      yPoints: [10, 20, 30],
      xScale: { min: 0, max: 2 },
      yScale: { min: 0, max: 50 },
    };
    const json = graphicalFunctionToJson(gf);
    expect(json.points).toHaveLength(3);
    expect(json.points![0]).toEqual([0, 10]);
    expect(json.points![1]).toEqual([1, 20]);
    expect(json.points![2]).toEqual([2, 30]);

    const restored = graphicalFunctionFromJson(json);
    expect(restored.kind).toBe('continuous');
    expect(restored.xPoints).toEqual([0, 1, 2]);
    expect(restored.yPoints).toEqual([10, 20, 30]);
  });

  it('should roundtrip with yPoints only', () => {
    const gf: GraphicalFunction = {
      kind: 'extrapolate',
      xPoints: undefined,
      yPoints: [5, 10, 15, 20],
      xScale: { min: 0, max: 3 },
      yScale: { min: 0, max: 25 },
    };
    const json = graphicalFunctionToJson(gf);
    expect(json.yPoints).toEqual([5, 10, 15, 20]);
    expect(json.points).toBeUndefined();

    const restored = graphicalFunctionFromJson(json);
    expect(restored.yPoints).toEqual([5, 10, 15, 20]);
    expect(restored.xPoints).toBeUndefined();
  });

  it('should default empty yPoints to a zero-width x scale', () => {
    const restored = graphicalFunctionFromJson({});

    expect(restored.xScale).toEqual({ min: 0, max: 0 });
    expect(restored.yPoints).toEqual([]);
    expect(restored.xPoints).toBeUndefined();
  });
});

describe('Stock', () => {
  it('should roundtrip correctly', () => {
    const stock: Stock = {
      type: 'stock',
      ident: 'population',
      equation: { type: 'scalar', equation: '100' },
      documentation: 'Population of the system',
      units: 'people',
      inflows: ['births'],
      outflows: ['deaths'],
      nonNegative: true,
      canBeModuleInput: false,
      isPublic: false,
      activeInitial: undefined,
      dataSource: undefined,
      data: undefined,
      errors: undefined,
      unitErrors: undefined,
      uid: 1,
    };

    const json = stockToJson(stock);
    expect(json.name).toBe('population');
    expect(json.initialEquation).toBe('100');
    expect(json.inflows).toEqual(['births']);
    expect(json.outflows).toEqual(['deaths']);
    expect(json.compat?.nonNegative).toBe(true);
    expect(json.uid).toBe(1);

    const restored = stockFromJson(json);
    expect(restored.ident).toBe('population');
    expect(restored.equation.type).toBe('scalar');
    expect((restored.equation as ScalarEquation).equation).toBe('100');
    expect(restored.inflows).toEqual(['births']);
    expect(restored.outflows).toEqual(['deaths']);
    expect(restored.nonNegative).toBe(true);
    expect(restored.uid).toBe(1);
  });

  it('should preserve legacy top-level nonNegative', () => {
    // Simulates old JSON with nonNegative at top level and compat only for activeInitial
    const legacyJson = {
      name: 'pop',
      inflows: [],
      outflows: [],
      nonNegative: true,
      compat: { activeInitial: '50' },
    };
    const stock = stockFromJson(legacyJson);
    expect(stock.nonNegative).toBe(true);
  });

  it('should read nonNegative from compat in new format', () => {
    const newJson = {
      name: 'pop',
      inflows: [],
      outflows: [],
      compat: { nonNegative: true },
    };
    const stock = stockFromJson(newJson);
    expect(stock.nonNegative).toBe(true);
  });
});

describe('legacy top-level canBeModuleInput / isPublic', () => {
  // The engine's JSON reader (json.rs) OR-merges legacy top-level
  // canBeModuleInput/isPublic with compat for stock/flow/aux/module (read from
  // old JSON, never written). datamodel.ts must mirror it so a project saved in
  // the old Go `sd` JSON schema -- flags at the top level rather than under
  // compat -- does not silently lose module-input / public visibility the next
  // time the variable is edited and re-upserted.
  it('stockFromJson reads legacy top-level flags', () => {
    const stock = stockFromJson({ name: 'level', inflows: [], outflows: [], canBeModuleInput: true, isPublic: true });
    expect(stock.canBeModuleInput).toBe(true);
    expect(stock.isPublic).toBe(true);
  });

  it('flowFromJson reads legacy top-level flags', () => {
    const flow = flowFromJson({ name: 'rate', canBeModuleInput: true, isPublic: true });
    expect(flow.canBeModuleInput).toBe(true);
    expect(flow.isPublic).toBe(true);
  });

  it('auxFromJson reads legacy top-level flags', () => {
    const aux = auxFromJson({ name: 'x', canBeModuleInput: true, isPublic: true });
    expect(aux.canBeModuleInput).toBe(true);
    expect(aux.isPublic).toBe(true);
  });

  it('moduleFromJson reads legacy top-level flags', () => {
    const mod = moduleFromJson({ name: 'sub_inst', modelName: 'sub', canBeModuleInput: true, isPublic: true });
    expect(mod.canBeModuleInput).toBe(true);
    expect(mod.isPublic).toBe(true);
  });

  it('moduleFromJson still reads the new compat format', () => {
    const mod = moduleFromJson({
      name: 'sub_inst',
      modelName: 'sub',
      compat: { canBeModuleInput: true, isPublic: true },
    });
    expect(mod.canBeModuleInput).toBe(true);
    expect(mod.isPublic).toBe(true);
  });
});

describe('ACTIVE INITIAL under arrayedEquation.compat', () => {
  // The engine's JSON reader (json.rs) reads a flow/aux ACTIVE INITIAL from the
  // top-level compat first, then falls back to arrayedEquation.compat (a legacy/
  // native JSON shape). datamodel.ts must mirror it so an arrayed flow/aux that
  // stored ACTIVE INITIAL on the arrayed equation does not lose it on the next
  // edit+upsert. (Stocks have no such fallback in the engine reader, so we do
  // not add one here.)
  it('flowFromJson falls back to arrayedEquation.compat.activeInitial', () => {
    const flow = flowFromJson({
      name: 'rate',
      arrayedEquation: { dimensions: ['region'], equation: '1', compat: { activeInitial: '7' } },
    });
    expect(flow.activeInitial).toBe('7');
  });

  it('auxFromJson falls back to arrayedEquation.compat.activeInitial', () => {
    const aux = auxFromJson({
      name: 'x',
      arrayedEquation: { dimensions: ['region'], equation: '1', compat: { activeInitial: '9' } },
    });
    expect(aux.activeInitial).toBe('9');
  });

  it('top-level compat.activeInitial wins over the arrayed fallback', () => {
    const aux = auxFromJson({
      name: 'x',
      compat: { activeInitial: 'top' },
      arrayedEquation: { dimensions: ['region'], equation: '1', compat: { activeInitial: 'arrayed' } },
    });
    expect(aux.activeInitial).toBe('top');
  });
});

describe('Flow', () => {
  it('should roundtrip correctly', () => {
    const flow: Flow = {
      type: 'flow',
      ident: 'births',
      equation: { type: 'scalar', equation: 'population * birth_rate' },
      documentation: 'Birth rate',
      units: 'people/year',
      gf: undefined,
      nonNegative: true,
      canBeModuleInput: false,
      isPublic: false,
      activeInitial: undefined,
      dataSource: undefined,
      data: undefined,
      errors: undefined,
      unitErrors: undefined,
      uid: 2,
    };

    const json = flowToJson(flow);
    expect(json.name).toBe('births');
    expect(json.equation).toBe('population * birth_rate');

    const restored = flowFromJson(json);
    expect(restored.ident).toBe('births');
    expect((restored.equation as ScalarEquation).equation).toBe('population * birth_rate');
  });

  it('should preserve legacy top-level nonNegative', () => {
    const legacyJson = {
      name: 'rate',
      nonNegative: true,
      compat: { activeInitial: '5' },
    };
    const flow = flowFromJson(legacyJson);
    expect(flow.nonNegative).toBe(true);
  });
});

describe('Aux', () => {
  it('should roundtrip correctly', () => {
    const aux: Aux = {
      type: 'aux',
      ident: 'birth_rate',
      equation: { type: 'scalar', equation: '0.03' },
      documentation: 'Annual birth rate',
      units: '1/year',
      gf: undefined,
      canBeModuleInput: false,
      isPublic: false,
      activeInitial: undefined,
      dataSource: undefined,
      data: undefined,
      errors: undefined,
      unitErrors: undefined,
      uid: 3,
    };

    const json = auxToJson(aux);
    expect(json.name).toBe('birth_rate');
    expect(json.equation).toBe('0.03');

    const restored = auxFromJson(json);
    expect(restored.ident).toBe('birth_rate');
  });

  it('should roundtrip with graphical function', () => {
    const gf: GraphicalFunction = {
      kind: 'continuous',
      xPoints: [0, 50, 100],
      yPoints: [0, 0.5, 1],
      xScale: { min: 0, max: 100 },
      yScale: { min: 0, max: 1 },
    };

    const aux: Aux = {
      type: 'aux',
      ident: 'effect',
      equation: { type: 'scalar', equation: 'input' },
      documentation: '',
      units: '',
      gf,
      canBeModuleInput: false,
      isPublic: false,
      activeInitial: undefined,
      dataSource: undefined,
      data: undefined,
      errors: undefined,
      unitErrors: undefined,
      uid: 4,
    };

    const json = auxToJson(aux);
    expect(json.graphicalFunction).toBeDefined();
    expect(json.graphicalFunction!.points).toHaveLength(3);

    const restored = auxFromJson(json);
    expect(restored.gf).toBeDefined();
    expect(restored.gf!.yPoints).toEqual([0, 0.5, 1]);
  });
});

describe('Module', () => {
  it('should roundtrip correctly', () => {
    const mod: Module = {
      type: 'module',
      ident: 'sector',
      modelName: 'SectorModel',
      documentation: 'A sector submodel',
      units: '',
      references: [{ src: 'input_var', dst: 'sector_input' }],
      canBeModuleInput: false,
      isPublic: false,
      dataSource: undefined,
      data: undefined,
      errors: undefined,
      unitErrors: undefined,
      uid: 5,
    };

    const json = moduleToJson(mod);
    expect(json.name).toBe('sector');
    expect(json.modelName).toBe('SectorModel');
    expect(json.references).toHaveLength(1);
    expect(json.references![0].src).toBe('input_var');

    const restored = moduleFromJson(json);
    expect(restored.ident).toBe('sector');
    expect(restored.modelName).toBe('SectorModel');
    expect(restored.references.length).toBe(1);
  });

  it('roundtrips a freshly-drawn module (empty modelName, no references)', () => {
    // The editor creates a module with an EMPTY modelName until the user
    // assigns a target model -- the exact state that panicked the engine in
    // c1c4c954. The empty modelName must survive serialization (the Rust
    // `model_name` is a required String, so a dropped field is an FFI hard
    // error) and deserialize back to empty.
    const mod: Module = {
      type: 'module',
      ident: 'new_module',
      modelName: '',
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
    };

    const json = moduleToJson(mod);
    expect(json.modelName).toBe('');
    expect(json.references ?? []).toEqual([]);

    const restored = moduleFromJson(json);
    expect(restored.modelName).toBe('');
    expect(restored.references).toEqual([]);
  });

  it('preserves placeholder references with empty src/dst', () => {
    // The wiring UI persists partially-filled rows (empty src or dst) as the
    // user builds up inputs; these must round-trip verbatim, not be coalesced
    // or dropped.
    const mod: Module = {
      type: 'module',
      ident: 'sector',
      modelName: 'SectorModel',
      documentation: '',
      units: '',
      references: [
        { src: '', dst: '' },
        { src: 'food', dst: '' },
      ],
      canBeModuleInput: false,
      isPublic: false,
      dataSource: undefined,
      data: undefined,
      errors: undefined,
      unitErrors: undefined,
      uid: 5,
    };

    const restored = moduleFromJson(moduleToJson(mod));
    expect(restored.references).toEqual([
      { src: '', dst: '' },
      { src: 'food', dst: '' },
    ]);
  });

  it('roundtrips a module-qualified dst reference', () => {
    // The canonical reference dst is the module-qualified `{moduleIdent}·{port}`
    // form (the engine strips the prefix to wire the input); the middot must
    // survive the JSON round trip intact.
    const mod: Module = {
      type: 'module',
      ident: 'sector',
      modelName: 'SectorModel',
      documentation: '',
      units: '',
      references: [{ src: 'food', dst: 'sector·sector_input' }],
      canBeModuleInput: false,
      isPublic: false,
      dataSource: undefined,
      data: undefined,
      errors: undefined,
      unitErrors: undefined,
      uid: 5,
    };

    const restored = moduleFromJson(moduleToJson(mod));
    expect(restored.references[0].dst).toBe('sector·sector_input');
  });

  it('round-trips module compat (canBeModuleInput, isPublic, dataSource)', () => {
    // The engine's From<Module> (json.rs) reads these three compat fields and
    // defaults the rest, so a module edit must preserve exactly these.
    const json: JsonModule = {
      name: 'sector',
      modelName: 'SectorModel',
      compat: {
        canBeModuleInput: true,
        isPublic: true,
        dataSource: { kind: 'constants', file: 'c.csv', tabOrDelimiter: ',', rowOrCol: '1', cell: 'A1' },
      },
    };
    const restored = moduleFromJson(json);
    expect(restored.canBeModuleInput).toBe(true);
    expect(restored.isPublic).toBe(true);
    expect(restored.dataSource?.kind).toBe('constants');
    expect(restored.dataSource?.file).toBe('c.csv');

    const out = moduleToJson(restored);
    expect(out.compat?.canBeModuleInput).toBe(true);
    expect(out.compat?.isPublic).toBe(true);
    expect(out.compat?.dataSource).toEqual(json.compat!.dataSource);
  });
});

describe('View Elements', () => {
  it('should roundtrip StockViewElement', () => {
    const elem: StockViewElement = {
      type: 'stock',
      uid: 1,
      name: 'Population',
      ident: 'population',
      var: undefined,
      x: 100,
      y: 200,
      labelSide: 'top',
      isZeroRadius: false,
      inflows: [],
      outflows: [],
    };

    const json = stockFlowViewToJson({
      nextUid: 2,
      elements: [elem],
      viewBox: { x: 0, y: 0, width: 0, height: 0 },
      zoom: -1,
      useLetteredPolarity: false,
    });
    const stockJson = json.elements[0] as JsonStockViewElement;
    expect(stockJson.type).toBe('stock');
    expect(stockJson.name).toBe('Population');
    expect(stockJson.x).toBe(100);
    expect(stockJson.y).toBe(200);
    expect(stockJson.labelSide).toBe('top');

    const restored = stockViewElementFromJson(stockJson);
    expect(restored.uid).toBe(1);
    expect(restored.name).toBe('Population');
    expect(restored.x).toBe(100);
    expect(restored.y).toBe(200);
  });

  it('should roundtrip FlowViewElement', () => {
    const elem: FlowViewElement = {
      type: 'flow',
      uid: 2,
      name: 'Births',
      ident: 'births',
      var: undefined,
      x: 50,
      y: 200,
      labelSide: 'bottom',
      points: [
        { x: 0, y: 200, attachedToUid: undefined },
        { x: 100, y: 200, attachedToUid: 1 },
      ],
      isZeroRadius: false,
    };

    const json = stockFlowViewToJson({
      nextUid: 3,
      elements: [elem],
      viewBox: { x: 0, y: 0, width: 0, height: 0 },
      zoom: -1,
      useLetteredPolarity: false,
    });
    const flowJson = json.elements[0] as JsonFlowViewElement;
    expect(flowJson.type).toBe('flow');
    expect(flowJson.points).toHaveLength(2);
    expect(flowJson.points[1].attachedToUid).toBe(1);

    const restored = flowViewElementFromJson(flowJson);
    expect(restored.points.length).toBe(2);
    expect(restored.points[1]?.attachedToUid).toBe(1);
  });

  it('should roundtrip LinkViewElement with arc', () => {
    const elem: LinkViewElement = {
      type: 'link',
      uid: 3,
      fromUid: 1,
      toUid: 2,
      arc: 30,
      isStraight: false,
      multiPoint: undefined,
      polarity: undefined,
      x: NaN,
      y: NaN,
      isZeroRadius: false,
      ident: undefined,
    };

    const json = stockFlowViewToJson({
      nextUid: 4,
      elements: [elem],
      viewBox: { x: 0, y: 0, width: 0, height: 0 },
      zoom: -1,
      useLetteredPolarity: false,
    });
    const linkJson = json.elements[0] as JsonLinkViewElement;
    expect(linkJson.type).toBe('link');
    expect(linkJson.arc).toBe(30);
    expect(linkJson.multiPoints).toBeUndefined();

    const restored = linkViewElementFromJson(linkJson);
    expect(restored.arc).toBe(30);
    expect(restored.isStraight).toBe(false);
  });

  it('should roundtrip LinkViewElement straight line', () => {
    const elem: LinkViewElement = {
      type: 'link',
      uid: 4,
      fromUid: 1,
      toUid: 3,
      arc: undefined,
      isStraight: true,
      multiPoint: undefined,
      polarity: undefined,
      x: NaN,
      y: NaN,
      isZeroRadius: false,
      ident: undefined,
    };

    const json = stockFlowViewToJson({
      nextUid: 5,
      elements: [elem],
      viewBox: { x: 0, y: 0, width: 0, height: 0 },
      zoom: -1,
      useLetteredPolarity: false,
    });
    const linkJson = json.elements[0] as JsonLinkViewElement;
    expect(linkJson.type).toBe('link');
    expect(linkJson.arc).toBeUndefined();

    const restored = linkViewElementFromJson(linkJson);
    expect(restored.isStraight).toBe(true);
    expect(restored.arc).toBeUndefined();
  });

  it('should roundtrip CloudViewElement', () => {
    const elem: CloudViewElement = {
      type: 'cloud',
      uid: 5,
      flowUid: 2,
      x: 0,
      y: 200,
      isZeroRadius: false,
      ident: undefined,
    };

    const json = stockFlowViewToJson({
      nextUid: 6,
      elements: [elem],
      viewBox: { x: 0, y: 0, width: 0, height: 0 },
      zoom: -1,
      useLetteredPolarity: false,
    });
    const cloudJson = json.elements[0] as JsonCloudViewElement;
    expect(cloudJson.type).toBe('cloud');
    expect(cloudJson.flowUid).toBe(2);

    const restored = cloudViewElementFromJson(cloudJson);
    expect(restored.flowUid).toBe(2);
  });
});

describe('SimSpecs', () => {
  it('should roundtrip correctly', () => {
    const specs: SimSpecs = {
      start: 0,
      stop: 100,
      dt: { value: 0.25, isReciprocal: false },
      saveStep: { value: 1, isReciprocal: false },
      simMethod: 'rk4',
      timeUnits: 'years',
    };

    const json = simSpecsToJson(specs);
    expect(json.startTime).toBe(0);
    expect(json.endTime).toBe(100);
    expect(json.dt).toBe('0.25');
    expect(json.saveStep).toBe(1);
    expect(json.method).toBe('rk4');

    const restored = simSpecsFromJson(json);
    expect(restored.start).toBe(0);
    expect(restored.stop).toBe(100);
    expect(restored.dt.value).toBe(0.25);
  });

  it('should handle reciprocal dt', () => {
    const specs: SimSpecs = {
      start: 0,
      stop: 10,
      dt: { value: 4, isReciprocal: true },
      saveStep: undefined,
      simMethod: 'euler',
      timeUnits: 'days',
    };

    const json = simSpecsToJson(specs);
    expect(json.dt).toBe('1/4');

    const restored = simSpecsFromJson(json);
    expect(restored.dt.isReciprocal).toBe(true);
    expect(restored.dt.value).toBe(4);
  });
});

describe('Dimension', () => {
  it('should roundtrip with elements', () => {
    const dim: Dimension = {
      name: 'regions',
      subscripts: ['north', 'south', 'east', 'west'],
    };

    const json = dimensionToJson(dim);
    expect(json.name).toBe('regions');
    expect(json.elements).toEqual(['north', 'south', 'east', 'west']);

    const restored = dimensionFromJson(json);
    expect(restored.name).toBe('regions');
    expect(restored.subscripts).toEqual(['north', 'south', 'east', 'west']);
  });
});

describe('Source', () => {
  it('should roundtrip correctly', () => {
    const source: Source = {
      extension: 'xmile',
      content: '<xmile>...</xmile>',
    };

    const json = sourceToJson(source);
    expect(json.extension).toBe('xmile');
    expect(json.content).toBe('<xmile>...</xmile>');

    const restored = sourceFromJson(json);
    expect(restored.extension).toBe('xmile');
    expect(restored.content).toBe('<xmile>...</xmile>');
  });

  it('should handle empty source', () => {
    const source: Source = {
      extension: undefined,
      content: '',
    };

    const json = sourceToJson(source);
    const restored = sourceFromJson(json);
    expect(restored.extension).toBeUndefined();
    expect(restored.content).toBe('');
  });
});

describe('LoopMetadata', () => {
  it('should roundtrip correctly', () => {
    const loop: LoopMetadata = {
      uids: [1, 2, 3, 4, 1],
      deleted: false,
      name: 'Growth Loop',
      description: 'Main reinforcing loop',
    };

    const json = loopMetadataToJson(loop);
    expect(json.uids).toEqual([1, 2, 3, 4, 1]);
    expect(json.name).toBe('Growth Loop');
    expect(json.description).toBe('Main reinforcing loop');
    expect(json.deleted).toBeUndefined();

    const restored = loopMetadataFromJson(json);
    expect(restored.uids).toEqual([1, 2, 3, 4, 1]);
    expect(restored.name).toBe('Growth Loop');
    expect(restored.deleted).toBe(false);
  });

  it('should handle deleted loop', () => {
    const loop: LoopMetadata = {
      uids: [1, 2],
      deleted: true,
      name: 'Deleted Loop',
      description: '',
    };

    const json = loopMetadataToJson(loop);
    expect(json.deleted).toBe(true);

    const restored = loopMetadataFromJson(json);
    expect(restored.deleted).toBe(true);
  });
});

describe('MacroSpec', () => {
  it('should roundtrip correctly', () => {
    const spec: MacroSpec = {
      parameters: ['input', 'gain'],
      primaryOutput: 'output',
      additionalOutputs: ['debug_trace'],
    };

    const json = macroSpecToJson(spec);
    expect(json.parameters).toEqual(['input', 'gain']);
    expect(json.primaryOutput).toBe('output');
    expect(json.additionalOutputs).toEqual(['debug_trace']);

    const restored = macroSpecFromJson(json);
    expect(restored.parameters).toEqual(['input', 'gain']);
    expect(restored.primaryOutput).toBe('output');
    expect(restored.additionalOutputs).toEqual(['debug_trace']);
  });

  it('should omit empty additionalOutputs', () => {
    const spec: MacroSpec = {
      parameters: ['input'],
      primaryOutput: 'output',
      additionalOutputs: [],
    };

    const json = macroSpecToJson(spec);
    expect(json.parameters).toEqual(['input']);
    expect(json.primaryOutput).toBe('output');
    expect(json.additionalOutputs).toBeUndefined();

    const restored = macroSpecFromJson(json);
    expect(restored.additionalOutputs).toEqual([]);
  });
});

describe('StockFlowView', () => {
  it('should roundtrip with various element types', () => {
    const stockElem: StockViewElement = {
      type: 'stock',
      uid: 1,
      name: 'Population',
      ident: 'population',
      var: undefined,
      x: 200,
      y: 200,
      labelSide: 'top',
      isZeroRadius: false,
      inflows: [2],
      outflows: [],
    };

    const flowElem: FlowViewElement = {
      type: 'flow',
      uid: 2,
      name: 'Births',
      ident: 'births',
      var: undefined,
      x: 100,
      y: 200,
      labelSide: 'bottom',
      points: [
        { x: 50, y: 200, attachedToUid: undefined },
        { x: 150, y: 200, attachedToUid: 1 },
      ],
      isZeroRadius: false,
    };

    const auxElem: AuxViewElement = {
      type: 'aux',
      uid: 3,
      name: 'BirthRate',
      ident: 'birth_rate',
      var: undefined,
      x: 100,
      y: 100,
      labelSide: 'right',
      isZeroRadius: false,
    };

    const linkElem: LinkViewElement = {
      type: 'link',
      uid: 4,
      fromUid: 3,
      toUid: 2,
      arc: 45,
      isStraight: false,
      multiPoint: undefined,
      polarity: undefined,
      x: NaN,
      y: NaN,
      isZeroRadius: false,
      ident: undefined,
    };

    const cloudElem: CloudViewElement = {
      type: 'cloud',
      uid: 5,
      flowUid: 2,
      x: 50,
      y: 200,
      isZeroRadius: false,
      ident: undefined,
    };

    const view: StockFlowView = {
      elements: [stockElem, flowElem, auxElem, linkElem, cloudElem],
      nextUid: 6,
      viewBox: { x: 0, y: 0, width: 800, height: 600 },
      zoom: 1.5,
      useLetteredPolarity: false,
    };

    const json = stockFlowViewToJson(view);
    expect(json.elements).toHaveLength(5);
    expect(json.viewBox).toEqual({ x: 0, y: 0, width: 800, height: 600 });
    expect(json.zoom).toBe(1.5);

    const restored = stockFlowViewFromJson(json, new Map());
    expect(restored.elements.length).toBe(5);
    expect(restored.viewBox.width).toBe(800);
    expect(restored.zoom).toBe(1.5);
  });
});

// These pin the fields the (now-deleted) diagram-local stockFlowViewToJson copy
// silently dropped, so unifying every diagram caller onto this implementation
// cannot regress them (issue #821, an #811-class field-drop cluster).
describe('StockFlowView serialization completeness (#821)', () => {
  it('preserves link polarity through a round-trip', () => {
    for (const polarity of ['+', '-'] as const) {
      const link: LinkViewElement = {
        type: 'link',
        uid: 4,
        fromUid: 1,
        toUid: 2,
        arc: 30,
        isStraight: false,
        multiPoint: undefined,
        polarity,
        x: NaN,
        y: NaN,
        isZeroRadius: false,
        ident: undefined,
      };
      const json = stockFlowViewToJson({
        nextUid: 5,
        elements: [link],
        viewBox: { x: 0, y: 0, width: 0, height: 0 },
        zoom: -1,
        useLetteredPolarity: false,
      });
      const linkJson = json.elements[0] as JsonLinkViewElement;
      expect(linkJson.polarity).toBe(polarity);
      expect(linkViewElementFromJson(linkJson).polarity).toBe(polarity);
    }
  });

  it('treats arc and multiPoint as mutually exclusive (arc wins, matching fromJson)', () => {
    const link: LinkViewElement = {
      type: 'link',
      uid: 4,
      fromUid: 1,
      toUid: 2,
      arc: 45,
      isStraight: false,
      multiPoint: [{ x: 1, y: 2, attachedToUid: undefined }],
      polarity: undefined,
      x: NaN,
      y: NaN,
      isZeroRadius: false,
      ident: undefined,
    };
    const json = stockFlowViewToJson({
      nextUid: 5,
      elements: [link],
      viewBox: { x: 0, y: 0, width: 0, height: 0 },
      zoom: -1,
      useLetteredPolarity: false,
    });
    const linkJson = json.elements[0] as JsonLinkViewElement;
    expect(linkJson.arc).toBe(45);
    expect(linkJson.multiPoints).toBeUndefined();
  });

  it('preserves useLetteredPolarity when set', () => {
    const json = stockFlowViewToJson({
      nextUid: 1,
      elements: [],
      viewBox: { x: 0, y: 0, width: 0, height: 0 },
      zoom: -1,
      useLetteredPolarity: true,
    });
    expect(json.useLetteredPolarity).toBe(true);
    const restored = stockFlowViewFromJson(json, new Map());
    expect(restored.useLetteredPolarity).toBe(true);
  });

  it('keeps a viewBox with a single non-zero dimension (OR, not AND)', () => {
    const json = stockFlowViewToJson({
      nextUid: 1,
      elements: [],
      viewBox: { x: 0, y: 0, width: 800, height: 0 },
      zoom: -1,
      useLetteredPolarity: false,
    });
    expect(json.viewBox).toEqual({ x: 0, y: 0, width: 800, height: 0 });
  });

  it('round-trips a Z-shape flow whose corner points are unattached (#819)', () => {
    const flow: FlowViewElement = {
      type: 'flow',
      uid: 10,
      name: 'Births',
      ident: 'births',
      var: undefined,
      x: 50,
      y: 200,
      labelSide: 'center',
      points: [
        { x: 0, y: 200, attachedToUid: undefined },
        { x: 50, y: 200, attachedToUid: undefined },
        { x: 50, y: 100, attachedToUid: undefined },
        { x: 100, y: 100, attachedToUid: 1 },
      ],
      isZeroRadius: false,
    };
    const json = stockFlowViewToJson({
      nextUid: 11,
      elements: [flow],
      viewBox: { x: 0, y: 0, width: 0, height: 0 },
      zoom: -1,
      useLetteredPolarity: false,
    });
    const flowJson = json.elements[0] as JsonFlowViewElement;
    // Unattached corners omit attachedToUid entirely; the attached corner keeps it.
    expect(flowJson.points.map((p) => p.attachedToUid)).toEqual([undefined, undefined, undefined, 1]);
    for (const p of flowJson.points.slice(0, 3)) {
      expect('attachedToUid' in p).toBe(false);
    }
    const restored = flowViewElementFromJson(flowJson);
    expect(restored.points.map((p) => p.attachedToUid)).toEqual([undefined, undefined, undefined, 1]);
  });
});

describe('canBeModuleInput and isPublic', () => {
  it('should default canBeModuleInput and isPublic to false for Stock', () => {
    const json = { name: 'pop', inflows: [], outflows: [] };
    const stock = stockFromJson(json);
    expect(stock.canBeModuleInput).toBe(false);
    expect(stock.isPublic).toBe(false);
  });

  it('should read canBeModuleInput and isPublic from compat for Stock', () => {
    const json = {
      name: 'pop',
      inflows: [],
      outflows: [],
      compat: { canBeModuleInput: true, isPublic: true },
    };
    const stock = stockFromJson(json);
    expect(stock.canBeModuleInput).toBe(true);
    expect(stock.isPublic).toBe(true);
  });

  it('should write canBeModuleInput and isPublic to compat for Stock', () => {
    const stock: Stock = {
      type: 'stock',
      ident: 'pop',
      equation: { type: 'scalar', equation: '100' },
      documentation: '',
      units: '',
      inflows: [],
      outflows: [],
      nonNegative: false,
      canBeModuleInput: true,
      isPublic: true,
      activeInitial: undefined,
      dataSource: undefined,
      data: undefined,
      errors: undefined,
      unitErrors: undefined,
      uid: 1,
    };
    const json = stockToJson(stock);
    expect(json.compat?.canBeModuleInput).toBe(true);
    expect(json.compat?.isPublic).toBe(true);
  });

  it('should not write false canBeModuleInput/isPublic to compat for Stock', () => {
    const stock: Stock = {
      type: 'stock',
      ident: 'pop',
      equation: { type: 'scalar', equation: '100' },
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
      uid: 1,
    };
    const json = stockToJson(stock);
    expect(json.compat).toBeUndefined();
  });

  it('should preserve nonNegative alongside canBeModuleInput in compat roundtrip for Stock', () => {
    const stock: Stock = {
      type: 'stock',
      ident: 'pop',
      equation: { type: 'scalar', equation: '100' },
      documentation: '',
      units: '',
      inflows: [],
      outflows: [],
      nonNegative: true,
      canBeModuleInput: true,
      isPublic: false,
      activeInitial: undefined,
      dataSource: undefined,
      data: undefined,
      errors: undefined,
      unitErrors: undefined,
      uid: 1,
    };
    const json = stockToJson(stock);
    expect(json.compat?.nonNegative).toBe(true);
    expect(json.compat?.canBeModuleInput).toBe(true);
    expect(json.compat?.isPublic).toBeUndefined();

    const restored = stockFromJson(json);
    expect(restored.nonNegative).toBe(true);
    expect(restored.canBeModuleInput).toBe(true);
    expect(restored.isPublic).toBe(false);
  });

  it('should default canBeModuleInput and isPublic to false for Flow', () => {
    const json = { name: 'rate' };
    const flow = flowFromJson(json);
    expect(flow.canBeModuleInput).toBe(false);
    expect(flow.isPublic).toBe(false);
  });

  it('should read canBeModuleInput and isPublic from compat for Flow', () => {
    const json = {
      name: 'rate',
      compat: { canBeModuleInput: true, isPublic: true },
    };
    const flow = flowFromJson(json);
    expect(flow.canBeModuleInput).toBe(true);
    expect(flow.isPublic).toBe(true);
  });

  it('should write canBeModuleInput and isPublic to compat for Flow', () => {
    const flow: Flow = {
      type: 'flow',
      ident: 'rate',
      equation: { type: 'scalar', equation: '10' },
      documentation: '',
      units: '',
      gf: undefined,
      nonNegative: false,
      canBeModuleInput: true,
      isPublic: true,
      activeInitial: undefined,
      dataSource: undefined,
      data: undefined,
      errors: undefined,
      unitErrors: undefined,
      uid: 2,
    };
    const json = flowToJson(flow);
    expect(json.compat?.canBeModuleInput).toBe(true);
    expect(json.compat?.isPublic).toBe(true);
  });

  it('should preserve nonNegative alongside isPublic in compat roundtrip for Flow', () => {
    const flow: Flow = {
      type: 'flow',
      ident: 'rate',
      equation: { type: 'scalar', equation: '10' },
      documentation: '',
      units: '',
      gf: undefined,
      nonNegative: true,
      canBeModuleInput: false,
      isPublic: true,
      activeInitial: undefined,
      dataSource: undefined,
      data: undefined,
      errors: undefined,
      unitErrors: undefined,
      uid: 2,
    };
    const json = flowToJson(flow);
    expect(json.compat?.nonNegative).toBe(true);
    expect(json.compat?.isPublic).toBe(true);
    expect(json.compat?.canBeModuleInput).toBeUndefined();

    const restored = flowFromJson(json);
    expect(restored.nonNegative).toBe(true);
    expect(restored.isPublic).toBe(true);
    expect(restored.canBeModuleInput).toBe(false);
  });

  it('should default canBeModuleInput and isPublic to false for Aux', () => {
    const json = { name: 'param' };
    const aux = auxFromJson(json);
    expect(aux.canBeModuleInput).toBe(false);
    expect(aux.isPublic).toBe(false);
  });

  it('should read canBeModuleInput and isPublic from compat for Aux', () => {
    const json = {
      name: 'param',
      compat: { canBeModuleInput: true, isPublic: true },
    };
    const aux = auxFromJson(json);
    expect(aux.canBeModuleInput).toBe(true);
    expect(aux.isPublic).toBe(true);
  });

  it('should write canBeModuleInput and isPublic to compat for Aux', () => {
    const aux: Aux = {
      type: 'aux',
      ident: 'param',
      equation: { type: 'scalar', equation: '5' },
      documentation: '',
      units: '',
      gf: undefined,
      canBeModuleInput: true,
      isPublic: true,
      activeInitial: undefined,
      dataSource: undefined,
      data: undefined,
      errors: undefined,
      unitErrors: undefined,
      uid: 3,
    };
    const json = auxToJson(aux);
    expect(json.compat?.canBeModuleInput).toBe(true);
    expect(json.compat?.isPublic).toBe(true);
  });

  it('should not write false canBeModuleInput/isPublic to compat for Aux', () => {
    const aux: Aux = {
      type: 'aux',
      ident: 'param',
      equation: { type: 'scalar', equation: '5' },
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
      uid: 3,
    };
    const json = auxToJson(aux);
    expect(json.compat).toBeUndefined();
  });
});

describe('Arrayed Equations', () => {
  it('should roundtrip Stock with ApplyToAllEquation', () => {
    const stock: Stock = {
      type: 'stock',
      ident: 'inventory',
      equation: {
        type: 'applyToAll',
        dimensionNames: ['warehouses'],
        equation: '100',
      },
      documentation: 'Inventory by warehouse',
      units: 'items',
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
      uid: 1,
    };

    const json = stockToJson(stock);
    expect(json.arrayedEquation).toBeDefined();
    expect(json.arrayedEquation!.dimensions).toEqual(['warehouses']);
    expect(json.arrayedEquation!.equation).toBe('100');
    expect(json.initialEquation).toBeUndefined();

    const restored = stockFromJson(json);
    expect(restored.equation.type).toBe('applyToAll');
    const eq = restored.equation as ApplyToAllEquation;
    expect(eq.dimensionNames).toEqual(['warehouses']);
    expect(eq.equation).toBe('100');
  });

  it('should roundtrip Aux with ArrayedEquation', () => {
    const aux: Aux = {
      type: 'aux',
      ident: 'demand',
      equation: {
        type: 'arrayed',
        dimensionNames: ['regions'],
        elements: new Map<string, ArrayedElement>([
          ['north', { equation: '50', graphicalFunction: undefined, activeInitial: undefined }],
          ['south', { equation: '75', graphicalFunction: undefined, activeInitial: undefined }],
        ]),
        defaultEquation: undefined,
        hasExceptDefault: false,
      },
      documentation: 'Demand by region',
      units: '',
      gf: undefined,
      canBeModuleInput: false,
      isPublic: false,
      activeInitial: undefined,
      dataSource: undefined,
      data: undefined,
      errors: undefined,
      unitErrors: undefined,
      uid: 2,
    };

    const json = auxToJson(aux);
    expect(json.arrayedEquation).toBeDefined();
    expect(json.arrayedEquation!.dimensions).toEqual(['regions']);
    expect(json.arrayedEquation!.elements).toHaveLength(2);

    const restored = auxFromJson(json);
    expect(restored.equation.type).toBe('arrayed');
    const eq = restored.equation as ArrayedEquation;
    expect(eq.dimensionNames).toEqual(['regions']);
    expect(eq.elements.size).toBe(2);
    expect(eq.elements.get('north')?.equation).toBe('50');
    expect(eq.elements.get('south')?.equation).toBe('75');
  });

  // The engine serializes ApplyToAll with NO `elements` field and Arrayed with
  // `elements` PRESENT (even []). An Arrayed with elements:[] + a default +
  // hasExceptDefault:false means EVERY element is missing and evaluates to 0 (a
  // missing element uses the default only when apply_default_for_missing/
  // hasExceptDefault is true -- compiler/mod.rs -- else 0.0). Collapsing such a
  // payload to applyToAll would make every element use the default instead: a
  // real behavior change the next editor upsert would persist. So the read path
  // must route on `elements` PRESENCE, not on it being non-empty.
  it('keeps a zero-element arrayed aux arrayed (EXCEPT-excludes-all), not applyToAll', () => {
    const restored = auxFromJson({
      name: 'demand',
      arrayedEquation: {
        dimensions: ['regions'],
        equation: '99',
        elements: [],
        hasExceptDefault: false,
      },
    });
    expect(restored.equation.type).toBe('arrayed');
    const eq = restored.equation as ArrayedEquation;
    expect(eq.dimensionNames).toEqual(['regions']);
    expect(eq.elements.size).toBe(0);
    expect(eq.defaultEquation).toBe('99');
    expect(eq.hasExceptDefault).toBe(false);

    // Round-trips back to elements:[] + equation:'99' + hasExceptDefault:false,
    // which the engine reads as Arrayed(.., [], Some('99'), false).
    const out = auxToJson(restored);
    expect(out.arrayedEquation).toBeDefined();
    expect(out.arrayedEquation!.elements).toEqual([]);
    expect(out.arrayedEquation!.equation).toBe('99');
    expect(out.arrayedEquation!.hasExceptDefault).toBe(false);
  });

  it('keeps a zero-element arrayed stock arrayed (EXCEPT-excludes-all), not applyToAll', () => {
    const restored = stockFromJson({
      name: 'level',
      initialEquation: '',
      inflows: [],
      outflows: [],
      arrayedEquation: {
        dimensions: ['regions'],
        equation: '99',
        elements: [],
        hasExceptDefault: false,
      },
    });
    expect(restored.equation.type).toBe('arrayed');
    const eq = restored.equation as ArrayedEquation;
    expect(eq.dimensionNames).toEqual(['regions']);
    expect(eq.elements.size).toBe(0);
    expect(eq.defaultEquation).toBe('99');
    expect(eq.hasExceptDefault).toBe(false);
  });
});

describe('LinkViewElement multiPoint', () => {
  it('should roundtrip with multiPoint path', () => {
    const elem: LinkViewElement = {
      type: 'link',
      uid: 10,
      fromUid: 1,
      toUid: 2,
      arc: undefined,
      isStraight: false,
      multiPoint: [
        { x: 100, y: 100, attachedToUid: undefined },
        { x: 150, y: 75, attachedToUid: undefined },
        { x: 200, y: 100, attachedToUid: undefined },
      ],
      polarity: undefined,
      x: NaN,
      y: NaN,
      isZeroRadius: false,
      ident: undefined,
    };

    const json = stockFlowViewToJson({
      nextUid: 11,
      elements: [elem],
      viewBox: { x: 0, y: 0, width: 0, height: 0 },
      zoom: -1,
      useLetteredPolarity: false,
    });
    const linkJson = json.elements[0] as JsonLinkViewElement;
    expect(linkJson.type).toBe('link');
    expect(linkJson.arc).toBeUndefined();
    const multiPoints = linkJson.multiPoints;
    expect(multiPoints).toHaveLength(3);
    expect(multiPoints?.[1]).toEqual({ x: 150, y: 75 });

    const restored = linkViewElementFromJson(linkJson);
    expect(restored.arc).toBeUndefined();
    expect(restored.isStraight).toBe(false);
    expect(restored.multiPoint).toBeDefined();
    expect(restored.multiPoint!.length).toBe(3);
    expect(restored.multiPoint![1]?.x).toBe(150);
    expect(restored.multiPoint![1]?.y).toBe(75);
  });
});

describe('Model', () => {
  it('should roundtrip correctly', () => {
    const stock: Stock = {
      type: 'stock',
      ident: 'population',
      equation: { type: 'scalar', equation: '100' },
      documentation: '',
      units: 'people',
      inflows: ['births'],
      outflows: ['deaths'],
      nonNegative: false,
      canBeModuleInput: false,
      isPublic: false,
      activeInitial: undefined,
      dataSource: undefined,
      data: undefined,
      errors: undefined,
      unitErrors: undefined,
      uid: 1,
    };

    const flow: Flow = {
      type: 'flow',
      ident: 'births',
      equation: { type: 'scalar', equation: 'population * 0.03' },
      documentation: '',
      units: 'people/year',
      gf: undefined,
      nonNegative: false,
      canBeModuleInput: false,
      isPublic: false,
      activeInitial: undefined,
      dataSource: undefined,
      data: undefined,
      errors: undefined,
      unitErrors: undefined,
      uid: 2,
    };

    const model: Model = {
      name: 'main',
      variables: new Map<string, Variable>([
        ['population', stock],
        ['births', flow],
      ]),
      views: [],
      loopMetadata: [],
      groups: [],
    };

    const json = modelToJson(model);
    expect(json.name).toBe('main');
    expect(json.stocks).toHaveLength(1);
    expect(json.flows).toHaveLength(1);

    const restored = modelFromJson(json);
    expect(restored.name).toBe('main');
    expect(restored.variables.size).toBe(2);
    expect(restored.variables.get('population')?.type).toBe('stock');
    expect(restored.variables.get('births')?.type).toBe('flow');
  });
});

describe('Project', () => {
  it('should roundtrip a complete project', () => {
    const stock: Stock = {
      type: 'stock',
      ident: 'population',
      equation: { type: 'scalar', equation: '100' },
      documentation: 'Population level',
      units: 'people',
      inflows: ['births'],
      outflows: [],
      nonNegative: true,
      canBeModuleInput: false,
      isPublic: false,
      activeInitial: undefined,
      dataSource: undefined,
      data: undefined,
      errors: undefined,
      unitErrors: undefined,
      uid: 1,
    };

    const flow: Flow = {
      type: 'flow',
      ident: 'births',
      equation: { type: 'scalar', equation: 'population * birth_rate' },
      documentation: '',
      units: 'people/year',
      gf: undefined,
      nonNegative: true,
      canBeModuleInput: false,
      isPublic: false,
      activeInitial: undefined,
      dataSource: undefined,
      data: undefined,
      errors: undefined,
      unitErrors: undefined,
      uid: 2,
    };

    const aux: Aux = {
      type: 'aux',
      ident: 'birth_rate',
      equation: { type: 'scalar', equation: '0.03' },
      documentation: '',
      units: '1/year',
      gf: undefined,
      canBeModuleInput: false,
      isPublic: false,
      activeInitial: undefined,
      dataSource: undefined,
      data: undefined,
      errors: undefined,
      unitErrors: undefined,
      uid: 3,
    };

    const model: Model = {
      name: 'main',
      variables: new Map<string, Variable>([
        ['population', stock],
        ['births', flow],
        ['birth_rate', aux],
      ]),
      views: [],
      loopMetadata: [],
      groups: [],
    };

    const simSpecs: SimSpecs = {
      start: 0,
      stop: 100,
      dt: { value: 0.25, isReciprocal: false },
      saveStep: { value: 1, isReciprocal: false },
      simMethod: 'rk4',
      timeUnits: 'years',
    };

    const project: Project = {
      name: 'growth_model',
      simSpecs,
      models: new Map([['main', model]]),
      dimensions: new Map(),
      hasNoEquations: false,
      source: { extension: 'xmile', content: '<xmile/>' },
    };

    const json = projectToJson(project);
    expect(json.name).toBe('growth_model');
    expect(json.simSpecs.startTime).toBe(0);
    expect(json.simSpecs.endTime).toBe(100);
    expect(json.models).toHaveLength(1);
    expect(json.models[0].stocks).toHaveLength(1);
    expect(json.models[0].flows).toHaveLength(1);
    expect(json.models[0].auxiliaries).toHaveLength(1);
    expect(json.source?.extension).toBe('xmile');

    const restored = projectFromJson(json);
    expect(restored.name).toBe('growth_model');
    expect(restored.simSpecs.start).toBe(0);
    expect(restored.simSpecs.stop).toBe(100);
    expect(restored.models.size).toBe(1);

    const restoredModel = restored.models.get('main');
    expect(restoredModel).toBeDefined();
    expect(restoredModel!.variables.size).toBe(3);
    expect(restored.source?.extension).toBe('xmile');
  });

  it('should handle empty project', () => {
    const model: Model = {
      name: 'main',
      variables: new Map(),
      views: [],
      loopMetadata: [],
      groups: [],
    };

    const project: Project = {
      name: 'empty',
      simSpecs: {
        start: 0,
        stop: 10,
        dt: { value: 1, isReciprocal: false },
        saveStep: undefined,
        simMethod: 'euler',
        timeUnits: '',
      },
      models: new Map([['main', model]]),
      dimensions: new Map(),
      hasNoEquations: true,
      source: undefined,
    };

    const json = projectToJson(project);
    const restored = projectFromJson(json);

    expect(restored.models.get('main')?.variables.size).toBe(0);
    expect(restored.source).toBeUndefined();
  });
});

describe('projectAttachData', () => {
  const series = (name: string, values: number[]): Series => ({
    name,
    time: new Float64Array([0, 1, 2]),
    values: new Float64Array(values),
  });

  const arrayedAux = (ident: string, dimensionNames: string[]): Aux => ({
    type: 'aux',
    ident,
    equation: { type: 'applyToAll', dimensionNames, equation: '1' },
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
    uid: 1,
  });

  const projectWith = (variables: Variable[], dimensions: Dimension[]): Project => ({
    name: 'test',
    simSpecs: {
      start: 0,
      stop: 2,
      dt: { value: 1, isReciprocal: false },
      saveStep: undefined,
      simMethod: 'euler',
      timeUnits: '',
    },
    models: new Map([
      [
        'main',
        {
          name: 'main',
          variables: new Map<string, Variable>(variables.map((v) => [v.ident, v])),
          views: [],
          loopMetadata: [],
          groups: [],
        },
      ],
    ]),
    dimensions: new Map(dimensions.map((d) => [d.name, d])),
    hasNoEquations: false,
    source: undefined,
  });

  // Regression test for arrayed-variable sparklines: the simulation keys its
  // per-element series by CANONICALIZED element names (e.g.
  // `temperature[high_2xco2_sensitivity]`), but a dimension preserves the
  // model's ORIGINAL-case subscript names. projectAttachData must canonicalize
  // each element when building the lookup key, or every arrayed variable whose
  // dimension elements aren't already lowercase gets no data.
  it('attaches per-element data for a 1-D arrayed variable with original-case subscripts', () => {
    const project = projectWith(
      [arrayedAux('temperature', ['scenario'])],
      [{ name: 'scenario', subscripts: ['Deterministic', 'Low_2xCO2_sensitivity', 'High_2xCO2_sensitivity'] }],
    );
    const data = new Map<string, Series>([
      ['temperature[deterministic]', series('temperature[deterministic]', [1, 2, 3])],
      ['temperature[low_2xco2_sensitivity]', series('temperature[low_2xco2_sensitivity]', [4, 5, 6])],
      ['temperature[high_2xco2_sensitivity]', series('temperature[high_2xco2_sensitivity]', [7, 8, 9])],
    ]);

    const attached = projectAttachData(project, data, 'main');
    const v = defined(attached.models.get('main')).variables.get('temperature');

    expect(v?.data).toBeDefined();
    // ordered by the dimension's declared subscript order
    expect((v?.data ?? []).map((s) => Array.from(s.values))).toEqual([
      [1, 2, 3],
      [4, 5, 6],
      [7, 8, 9],
    ]);
  });

  it('attaches data for an already-canonical 1-D arrayed variable', () => {
    const project = projectWith(
      [arrayedAux('population', ['region'])],
      [{ name: 'region', subscripts: ['boston', 'nyc'] }],
    );
    const data = new Map<string, Series>([
      ['population[boston]', series('population[boston]', [10, 11])],
      ['population[nyc]', series('population[nyc]', [20, 21])],
    ]);

    const attached = projectAttachData(project, data, 'main');
    const v = defined(attached.models.get('main')).variables.get('population');

    expect((v?.data ?? []).length).toBe(2);
  });

  // The "plot all the series" behavior must also cover multi-dimensional
  // arrayed variables: the simulation emits one series per element of the
  // cartesian product (`flux[co2,deterministic]`, ...), and every one should
  // be attached so the chart/sparkline can draw them all.
  it('attaches all per-element series for a multi-dimensional arrayed variable', () => {
    const project = projectWith(
      [arrayedAux('flux', ['gas', 'scenario'])],
      [
        { name: 'gas', subscripts: ['CO2', 'CH4'] },
        { name: 'scenario', subscripts: ['Deterministic', 'High_2xCO2_sensitivity'] },
      ],
    );
    const data = new Map<string, Series>([
      ['flux[co2,deterministic]', series('flux[co2,deterministic]', [1, 1])],
      ['flux[co2,high_2xco2_sensitivity]', series('flux[co2,high_2xco2_sensitivity]', [2, 2])],
      ['flux[ch4,deterministic]', series('flux[ch4,deterministic]', [3, 3])],
      ['flux[ch4,high_2xco2_sensitivity]', series('flux[ch4,high_2xco2_sensitivity]', [4, 4])],
    ]);

    const attached = projectAttachData(project, data, 'main');
    const v = defined(attached.models.get('main')).variables.get('flux');

    expect((v?.data ?? []).length).toBe(4);
  });

  it('leaves a variable with no result series unchanged', () => {
    const project = projectWith(
      [arrayedAux('unused', ['scenario'])],
      [{ name: 'scenario', subscripts: ['Deterministic'] }],
    );
    const attached = projectAttachData(project, new Map<string, Series>(), 'main');
    const v = defined(attached.models.get('main')).variables.get('unused');

    expect(v?.data).toBeUndefined();
  });
});

// These tests pin the wire fields that an editor upsert (a full variable
// replace by UID) previously dropped, because datamodel.ts neither read them
// on the way in nor wrote them on the way out. The JSON key names are the
// source of truth in the Rust serializer src/simlin-engine/src/json.rs
// (all #[serde(rename_all = "camelCase")]):
//   - Compat: activeInitial, nonNegative, canBeModuleInput, isPublic, dataSource
//   - JsonDataSource: kind ("data"/"constants"/"lookups"/"subscript"), file,
//     tabOrDelimiter, rowOrCol, cell
//   - ArrayedEquation: dimensions, equation (the EXCEPT default), elements,
//     hasExceptDefault
//   - ElementEquation: subscript, equation, compat, graphicalFunction
// The engine-driven round-trip in datamodel-roundtrip-e2e.test.ts re-checks
// these names against the real serializer, so a wrong key here cannot pass
// silently.
describe('variable compat round-trip (silent data-loss regression)', () => {
  it('round-trips activeInitial on a stock through compat', () => {
    const json: JsonStock = {
      name: 'level',
      inflows: [],
      outflows: [],
      initialEquation: '0',
      compat: { activeInitial: 'starting_level' },
    };
    const restored = stockFromJson(json);
    expect(restored.activeInitial).toBe('starting_level');
    expect(stockToJson(restored).compat?.activeInitial).toBe('starting_level');
  });

  it('round-trips activeInitial on a flow and an aux through compat', () => {
    const flow = flowFromJson({ name: 'rate', equation: '1', compat: { activeInitial: '2' } });
    expect(flow.activeInitial).toBe('2');
    expect(flowToJson(flow).compat?.activeInitial).toBe('2');

    const aux = auxFromJson({ name: 'a', equation: '1', compat: { activeInitial: '3' } });
    expect(aux.activeInitial).toBe('3');
    expect(auxToJson(aux).compat?.activeInitial).toBe('3');
  });

  it('round-trips compat.dataSource (GET DIRECT DATA) on an aux', () => {
    const json: JsonAuxiliary = {
      name: 'imported',
      compat: {
        dataSource: {
          kind: 'data',
          file: 'data.xlsx',
          tabOrDelimiter: 'Sheet1',
          rowOrCol: 'A',
          cell: 'B2',
        },
      },
    };
    const restored = auxFromJson(json);
    const ds = restored.dataSource as DataSource;
    expect(ds.kind).toBe('data');
    expect(ds.file).toBe('data.xlsx');
    expect(ds.tabOrDelimiter).toBe('Sheet1');
    expect(ds.rowOrCol).toBe('A');
    expect(ds.cell).toBe('B2');

    const out = auxToJson(restored).compat?.dataSource;
    expect(out).toEqual(json.compat!.dataSource);
  });

  it('maps each dataSource kind and falls back to "data" for unknown', () => {
    for (const kind of ['data', 'constants', 'lookups', 'subscript'] as const) {
      const aux = auxFromJson({
        name: 'x',
        compat: { dataSource: { kind, file: 'f', tabOrDelimiter: '', rowOrCol: '', cell: '' } },
      });
      expect(aux.dataSource?.kind).toBe(kind);
    }
    const unknown = auxFromJson({
      name: 'x',
      compat: { dataSource: { kind: 'bogus', file: 'f', tabOrDelimiter: '', rowOrCol: '', cell: '' } },
    });
    expect(unknown.dataSource?.kind).toBe('data');
  });

  it('round-trips the EXCEPT default equation and hasExceptDefault flag', () => {
    const json: JsonAuxiliary = {
      name: 'arr',
      arrayedEquation: {
        dimensions: ['dim'],
        equation: '99',
        hasExceptDefault: true,
        elements: [{ subscript: 'a', equation: '1' }],
      },
    };
    const restored = auxFromJson(json);
    const eq = restored.equation as ArrayedEquation;
    expect(eq.defaultEquation).toBe('99');
    expect(eq.hasExceptDefault).toBe(true);

    const out = auxToJson(restored).arrayedEquation!;
    expect(out.equation).toBe('99');
    expect(out.hasExceptDefault).toBe(true);
  });

  it('infers hasExceptDefault from a present default when the flag is absent', () => {
    // Mirrors the engine: legacy JSON without the flag infers true when a
    // default equation is present.
    const restored = auxFromJson({
      name: 'arr',
      arrayedEquation: { dimensions: ['dim'], equation: '7', elements: [{ subscript: 'a', equation: '1' }] },
    });
    expect((restored.equation as ArrayedEquation).hasExceptDefault).toBe(true);

    // No default equation => no flag emitted on the way out.
    const noDefault = auxFromJson({
      name: 'arr',
      arrayedEquation: { dimensions: ['dim'], elements: [{ subscript: 'a', equation: '1' }] },
    });
    const eq = noDefault.equation as ArrayedEquation;
    expect(eq.defaultEquation).toBeUndefined();
    expect(eq.hasExceptDefault).toBe(false);
    const out = auxToJson(noDefault).arrayedEquation!;
    expect(out.equation).toBeUndefined();
    expect(out.hasExceptDefault).toBeUndefined();
  });

  it('round-trips per-element graphical functions and per-element activeInitial', () => {
    const json: JsonAuxiliary = {
      name: 'arr',
      arrayedEquation: {
        dimensions: ['dim'],
        elements: [
          {
            subscript: 'a',
            equation: '1',
            graphicalFunction: { yPoints: [0, 1, 2], xScale: { min: 0, max: 2 }, yScale: { min: 0, max: 2 } },
            compat: { activeInitial: 'a0' },
          },
          { subscript: 'b', equation: '2' },
        ],
      },
    };
    const restored = auxFromJson(json);
    const eq = restored.equation as ArrayedEquation;
    const a = eq.elements.get('a') as ArrayedElement;
    expect(a.equation).toBe('1');
    expect(a.graphicalFunction?.yPoints).toEqual([0, 1, 2]);
    expect(a.activeInitial).toBe('a0');
    const b = eq.elements.get('b') as ArrayedElement;
    expect(b.graphicalFunction).toBeUndefined();
    expect(b.activeInitial).toBeUndefined();

    const out = auxToJson(restored).arrayedEquation!;
    const outA = out.elements!.find((e) => e.subscript === 'a')!;
    expect(outA.graphicalFunction?.yPoints).toEqual([0, 1, 2]);
    expect(outA.compat?.activeInitial).toBe('a0');
    const outB = out.elements!.find((e) => e.subscript === 'b')!;
    expect(outB.graphicalFunction).toBeUndefined();
    expect(outB.compat).toBeUndefined();
  });
});

describe('findNonFiniteViewCoord (#818)', () => {
  const view = (elements: readonly ViewElement[], extra?: Partial<StockFlowView>): StockFlowView => ({
    nextUid: 100,
    elements,
    viewBox: { x: 0, y: 0, width: 0, height: 0 },
    zoom: 1,
    useLetteredPolarity: false,
    ...extra,
  });

  const stock = (uid: number, x: number, y: number): StockViewElement => ({
    type: 'stock',
    uid,
    name: `s${uid}`,
    ident: `s${uid}`,
    var: undefined,
    x,
    y,
    labelSide: 'top',
    isZeroRadius: false,
    inflows: [],
    outflows: [],
  });

  const flow = (uid: number, valve: { x: number; y: number }, points: readonly Point[]): FlowViewElement => ({
    type: 'flow',
    uid,
    name: `f${uid}`,
    ident: `f${uid}`,
    var: undefined,
    x: valve.x,
    y: valve.y,
    labelSide: 'bottom',
    points,
    isZeroRadius: false,
  });

  it('returns undefined when every coordinate is finite', () => {
    const v = view([stock(1, 10, 20), flow(2, { x: 30, y: 20 }, [{ x: 10, y: 20, attachedToUid: 1 }])]);
    expect(findNonFiniteViewCoord(v)).toBeUndefined();
  });

  it('detects a NaN element coordinate', () => {
    const v = view([stock(1, NaN, 20)]);
    expect(findNonFiniteViewCoord(v)).toContain('stock uid=1');
  });

  it('detects a NaN flow valve coordinate', () => {
    const v = view([flow(2, { x: NaN, y: 20 }, [{ x: 10, y: 20, attachedToUid: 1 }])]);
    expect(findNonFiniteViewCoord(v)).toContain('flow uid=2 valve');
  });

  it('detects a NaN flow point coordinate', () => {
    const v = view([flow(2, { x: 30, y: 20 }, [{ x: NaN, y: 20, attachedToUid: 1 }])]);
    expect(findNonFiniteViewCoord(v)).toContain('flow uid=2 point[0]');
  });

  it('detects Infinity as non-finite', () => {
    const v = view([stock(1, 10, Infinity)]);
    expect(findNonFiniteViewCoord(v)).toContain('stock uid=1');
  });

  it('detects a NaN viewBox dimension', () => {
    const v = view([stock(1, 10, 20)], { viewBox: { x: 0, y: 0, width: NaN, height: 10 } });
    expect(findNonFiniteViewCoord(v)).toContain('viewBox');
  });
});

describe('stockFlowViewFromJson coordinate sanitization (#818)', () => {
  it('repairs a null/missing element coordinate to a finite value on load', () => {
    const json = {
      elements: [{ type: 'stock', uid: 1, name: 'pop', x: null, y: 5, labelSide: 'top' }],
    } as unknown as Parameters<typeof stockFlowViewFromJson>[0];

    const view = stockFlowViewFromJson(json, new Map());
    const stock = view.elements[0];
    expect(Number.isFinite(stock.x)).toBe(true);
    expect(stock.y).toBe(5);
    // The repaired view passes the serialization guard.
    expect(findNonFiniteViewCoord(view)).toBeUndefined();
  });

  it('repairs a null flow point coordinate on load', () => {
    const json = {
      elements: [
        {
          type: 'flow',
          uid: 2,
          name: 'f',
          x: 10,
          y: 20,
          labelSide: 'bottom',
          points: [
            { x: null, y: 0 },
            { x: 5, y: 0, attachedToUid: 1 },
          ],
        },
      ],
    } as unknown as Parameters<typeof stockFlowViewFromJson>[0];

    const view = stockFlowViewFromJson(json, new Map());
    const flow = view.elements[0] as FlowViewElement;
    expect(flow.points.every((p) => Number.isFinite(p.x) && Number.isFinite(p.y))).toBe(true);
    expect(findNonFiniteViewCoord(view)).toBeUndefined();
  });

  it('repairs a non-finite zoom/viewBox on load', () => {
    const json = {
      elements: [],
      viewBox: { x: 0, y: 0, width: NaN, height: 10 },
      zoom: null,
    } as unknown as Parameters<typeof stockFlowViewFromJson>[0];

    const view = stockFlowViewFromJson(json, new Map());
    expect(Number.isFinite(view.viewBox.width)).toBe(true);
    expect(Number.isFinite(view.zoom)).toBe(true);
    expect(findNonFiniteViewCoord(view)).toBeUndefined();
  });
});

describe('variableHasError', () => {
  const base = auxFromJson({ name: 'x', equation: '1' });

  it('is false for a clean variable', () => {
    expect(variableHasError(base)).toBe(false);
  });

  it('is true when equation errors are present', () => {
    expect(variableHasError({ ...base, errors: [{ code: ErrorCode.EmptyEquation, start: 0, end: 0 }] })).toBe(true);
  });

  it('is true when unit errors are present', () => {
    expect(
      variableHasError({
        ...base,
        unitErrors: [{ code: ErrorCode.UnitMismatch, start: 0, end: 0, isConsistencyError: true, details: undefined }],
      }),
    ).toBe(true);
  });

  it('is true when connector errors are present (non-fatal sketch drift)', () => {
    expect(variableHasError({ ...base, connectorErrors: [{ kind: 'missingConnector', ident: 'a', name: 'a' }] })).toBe(
      true,
    );
  });
});

describe('display-only annotations never serialize', () => {
  // errors/unitErrors/connectorErrors are UI annotations attached after the
  // engine round-trip; if one ever leaked into toJson output it would be
  // persisted into saved projects. Pin the exclusion.
  const annotations = {
    errors: [{ code: ErrorCode.EmptyEquation, start: 0, end: 0 }],
    unitErrors: [{ code: ErrorCode.UnitMismatch, start: 0, end: 0, isConsistencyError: true, details: undefined }],
    connectorErrors: [{ kind: 'missingConnector' as const, ident: 'a', name: 'a' }],
  };

  const expectNoAnnotationKeys = (json: object): void => {
    expect(json).not.toHaveProperty('errors');
    expect(json).not.toHaveProperty('unitErrors');
    expect(json).not.toHaveProperty('connectorErrors');
  };

  it('excludes annotations from aux JSON', () => {
    expectNoAnnotationKeys(auxToJson({ ...auxFromJson({ name: 'x', equation: '1' }), ...annotations }));
  });

  it('excludes annotations from stock JSON', () => {
    expectNoAnnotationKeys(stockToJson({ ...stockFromJson({ name: 's', initialEquation: '1' }), ...annotations }));
  });

  it('excludes annotations from flow JSON', () => {
    expectNoAnnotationKeys(flowToJson({ ...flowFromJson({ name: 'f', equation: '1' }), ...annotations }));
  });
});
