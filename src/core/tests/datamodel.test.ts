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
  simSpecsFromJson,
  simSpecsToJson,
  dimensionFromJson,
  dimensionToJson,
  sourceFromJson,
  sourceToJson,
  loopMetadataFromJson,
  loopMetadataToJson,
  modelFromJson,
  modelToJson,
  projectFromJson,
  projectToJson,
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
  SimSpecs,
  Dimension,
  Source,
  StockFlowView,
  StockViewElement,
  FlowViewElement,
  LinkViewElement,
  CloudViewElement,
  AuxViewElement,
  LoopMetadata,
  Model,
  Project,
  Variable,
} from '../datamodel';

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
    expect(json.nonNegative).toBe(true);
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
    const stockJson = json.elements[0];
    expect(stockJson.type).toBe('stock');
    expect((stockJson as any).name).toBe('Population');
    expect((stockJson as any).x).toBe(100);
    expect((stockJson as any).y).toBe(200);
    expect((stockJson as any).labelSide).toBe('top');

    const restored = stockViewElementFromJson(stockJson as any);
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
    const flowJson = json.elements[0] as any;
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
    const linkJson = json.elements[0] as any;
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
    const linkJson = json.elements[0] as any;
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
    const cloudJson = json.elements[0] as any;
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
        elements: new Map<string, string>([
          ['north', '50'],
          ['south', '75'],
        ]),
      },
      documentation: 'Demand by region',
      units: '',
      gf: undefined,
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
    expect(eq.elements.get('north')).toBe('50');
    expect(eq.elements.get('south')).toBe('75');
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
    const linkJson = json.elements[0] as any;
    expect(linkJson.type).toBe('link');
    expect(linkJson.arc).toBeUndefined();
    expect(linkJson.multiPoints).toHaveLength(3);
    expect(linkJson.multiPoints[1]).toEqual({ x: 150, y: 75 });

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
