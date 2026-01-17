// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { List, Map } from 'immutable';

import {
  GraphicalFunctionScale,
  GraphicalFunction,
  Stock,
  Flow,
  Aux,
  Module,
  ModuleReference,
  AuxViewElement,
  StockViewElement,
  FlowViewElement,
  LinkViewElement,
  ModuleViewElement,
  AliasViewElement,
  CloudViewElement,
  Point,
  Rect,
  StockFlowView,
  Model,
  SimSpecs,
  Dt,
  Dimension,
  Source,
  Project,
  ScalarEquation,
  ApplyToAllEquation,
  ArrayedEquation,
  LoopMetadata,
} from '../datamodel';

describe('GraphicalFunctionScale', () => {
  it('should roundtrip correctly', () => {
    const scale = new GraphicalFunctionScale({ min: -10, max: 100 });
    const json = scale.toJson();
    const restored = GraphicalFunctionScale.fromJson(json);
    expect(restored.min).toBe(scale.min);
    expect(restored.max).toBe(scale.max);
  });
});

describe('GraphicalFunction', () => {
  it('should roundtrip with points', () => {
    const gf = new GraphicalFunction({
      kind: 'continuous',
      xPoints: List([0, 1, 2]),
      yPoints: List([10, 20, 30]),
      xScale: new GraphicalFunctionScale({ min: 0, max: 2 }),
      yScale: new GraphicalFunctionScale({ min: 0, max: 50 }),
    });
    const json = gf.toJson();
    expect(json.points).toHaveLength(3);
    expect(json.points![0]).toEqual([0, 10]);
    expect(json.points![1]).toEqual([1, 20]);
    expect(json.points![2]).toEqual([2, 30]);

    const restored = GraphicalFunction.fromJson(json);
    expect(restored.kind).toBe('continuous');
    expect(restored.xPoints?.toArray()).toEqual([0, 1, 2]);
    expect(restored.yPoints.toArray()).toEqual([10, 20, 30]);
  });

  it('should roundtrip with yPoints only', () => {
    const gf = new GraphicalFunction({
      kind: 'extrapolate',
      xPoints: undefined,
      yPoints: List([5, 10, 15, 20]),
      xScale: new GraphicalFunctionScale({ min: 0, max: 3 }),
      yScale: new GraphicalFunctionScale({ min: 0, max: 25 }),
    });
    const json = gf.toJson();
    expect(json.yPoints).toEqual([5, 10, 15, 20]);
    expect(json.points).toBeUndefined();

    const restored = GraphicalFunction.fromJson(json);
    expect(restored.yPoints.toArray()).toEqual([5, 10, 15, 20]);
    expect(restored.xPoints).toBeUndefined();
  });
});

describe('Stock', () => {
  it('should roundtrip correctly', () => {
    const stock = new Stock({
      ident: 'population',
      equation: new ScalarEquation({ equation: '100' }),
      documentation: 'Population of the system',
      units: 'people',
      inflows: List(['births']),
      outflows: List(['deaths']),
      nonNegative: true,
      data: undefined,
      errors: undefined,
      unitErrors: undefined,
      uid: 1,
    });

    const json = stock.toJson();
    expect(json.name).toBe('population');
    expect(json.initialEquation).toBe('100');
    expect(json.inflows).toEqual(['births']);
    expect(json.outflows).toEqual(['deaths']);
    expect(json.nonNegative).toBe(true);
    expect(json.uid).toBe(1);

    const restored = Stock.fromJson(json);
    expect(restored.ident).toBe('population');
    expect((restored.equation as ScalarEquation).equation).toBe('100');
    expect(restored.inflows.toArray()).toEqual(['births']);
    expect(restored.outflows.toArray()).toEqual(['deaths']);
    expect(restored.nonNegative).toBe(true);
    expect(restored.uid).toBe(1);
  });
});

describe('Flow', () => {
  it('should roundtrip correctly', () => {
    const flow = new Flow({
      ident: 'births',
      equation: new ScalarEquation({ equation: 'population * birth_rate' }),
      documentation: 'Birth rate',
      units: 'people/year',
      gf: undefined,
      nonNegative: true,
      data: undefined,
      errors: undefined,
      unitErrors: undefined,
      uid: 2,
    });

    const json = flow.toJson();
    expect(json.name).toBe('births');
    expect(json.equation).toBe('population * birth_rate');

    const restored = Flow.fromJson(json);
    expect(restored.ident).toBe('births');
    expect((restored.equation as ScalarEquation).equation).toBe('population * birth_rate');
  });
});

describe('Aux', () => {
  it('should roundtrip correctly', () => {
    const aux = new Aux({
      ident: 'birth_rate',
      equation: new ScalarEquation({ equation: '0.03' }),
      documentation: 'Annual birth rate',
      units: '1/year',
      gf: undefined,
      data: undefined,
      errors: undefined,
      unitErrors: undefined,
      uid: 3,
    });

    const json = aux.toJson();
    expect(json.name).toBe('birth_rate');
    expect(json.equation).toBe('0.03');

    const restored = Aux.fromJson(json);
    expect(restored.ident).toBe('birth_rate');
  });

  it('should roundtrip with graphical function', () => {
    const gf = new GraphicalFunction({
      kind: 'continuous',
      xPoints: List([0, 50, 100]),
      yPoints: List([0, 0.5, 1]),
      xScale: new GraphicalFunctionScale({ min: 0, max: 100 }),
      yScale: new GraphicalFunctionScale({ min: 0, max: 1 }),
    });

    const aux = new Aux({
      ident: 'effect',
      equation: new ScalarEquation({ equation: 'input' }),
      documentation: '',
      units: '',
      gf,
      data: undefined,
      errors: undefined,
      unitErrors: undefined,
      uid: 4,
    });

    const json = aux.toJson();
    expect(json.graphicalFunction).toBeDefined();
    expect(json.graphicalFunction!.points).toHaveLength(3);

    const restored = Aux.fromJson(json);
    expect(restored.gf).toBeDefined();
    expect(restored.gf!.yPoints.toArray()).toEqual([0, 0.5, 1]);
  });
});

describe('Module', () => {
  it('should roundtrip correctly', () => {
    const mod = new Module({
      ident: 'sector',
      modelName: 'SectorModel',
      documentation: 'A sector submodel',
      units: '',
      references: List([
        new ModuleReference({ src: 'input_var', dst: 'sector_input' }),
      ]),
      data: undefined,
      errors: undefined,
      unitErrors: undefined,
      uid: 5,
    });

    const json = mod.toJson();
    expect(json.name).toBe('sector');
    expect(json.modelName).toBe('SectorModel');
    expect(json.references).toHaveLength(1);
    expect(json.references![0].src).toBe('input_var');

    const restored = Module.fromJson(json);
    expect(restored.ident).toBe('sector');
    expect(restored.modelName).toBe('SectorModel');
    expect(restored.references.size).toBe(1);
  });
});

describe('View Elements', () => {
  it('should roundtrip StockViewElement', () => {
    const elem = new StockViewElement({
      uid: 1,
      name: 'Population',
      ident: 'population',
      var: undefined,
      x: 100,
      y: 200,
      labelSide: 'top',
      inflows: List(),
      outflows: List(),
    });

    const json = elem.toJson();
    expect(json.type).toBe('stock');
    expect(json.name).toBe('Population');
    expect(json.x).toBe(100);
    expect(json.y).toBe(200);
    expect(json.labelSide).toBe('top');

    const restored = StockViewElement.fromJson(json);
    expect(restored.uid).toBe(1);
    expect(restored.name).toBe('Population');
    expect(restored.x).toBe(100);
    expect(restored.y).toBe(200);
  });

  it('should roundtrip FlowViewElement', () => {
    const elem = new FlowViewElement({
      uid: 2,
      name: 'Births',
      ident: 'births',
      var: undefined,
      x: 50,
      y: 200,
      labelSide: 'bottom',
      points: List([
        new Point({ x: 0, y: 200, attachedToUid: undefined }),
        new Point({ x: 100, y: 200, attachedToUid: 1 }),
      ]),
      isZeroRadius: false,
    });

    const json = elem.toJson();
    expect(json.type).toBe('flow');
    expect(json.points).toHaveLength(2);
    expect(json.points[1].attachedToUid).toBe(1);

    const restored = FlowViewElement.fromJson(json);
    expect(restored.points.size).toBe(2);
    expect(restored.points.get(1)?.attachedToUid).toBe(1);
  });

  it('should roundtrip LinkViewElement with arc', () => {
    const elem = new LinkViewElement({
      uid: 3,
      fromUid: 1,
      toUid: 2,
      arc: 30,
      isStraight: false,
      multiPoint: undefined,
    });

    const json = elem.toJson();
    expect(json.type).toBe('link');
    expect(json.arc).toBe(30);
    expect((json as any).multiPoints).toBeUndefined();

    const restored = LinkViewElement.fromJson(json);
    expect(restored.arc).toBe(30);
    expect(restored.isStraight).toBe(false);
  });

  it('should roundtrip LinkViewElement straight line', () => {
    const elem = new LinkViewElement({
      uid: 4,
      fromUid: 1,
      toUid: 3,
      arc: undefined,
      isStraight: true,
      multiPoint: undefined,
    });

    const json = elem.toJson();
    expect(json.type).toBe('link');
    expect(json.arc).toBeUndefined();

    const restored = LinkViewElement.fromJson(json);
    expect(restored.isStraight).toBe(true);
    expect(restored.arc).toBeUndefined();
  });

  it('should roundtrip CloudViewElement', () => {
    const elem = new CloudViewElement({
      uid: 5,
      flowUid: 2,
      x: 0,
      y: 200,
    });

    const json = elem.toJson();
    expect(json.type).toBe('cloud');
    expect(json.flowUid).toBe(2);

    const restored = CloudViewElement.fromJson(json);
    expect(restored.flowUid).toBe(2);
  });
});

describe('SimSpecs', () => {
  it('should roundtrip correctly', () => {
    const specs = new SimSpecs({
      start: 0,
      stop: 100,
      dt: new Dt({ value: 0.25, isReciprocal: false }),
      saveStep: new Dt({ value: 1, isReciprocal: false }),
      simMethod: 'rk4',
      timeUnits: 'years',
    });

    const json = specs.toJson();
    expect(json.startTime).toBe(0);
    expect(json.endTime).toBe(100);
    expect(json.dt).toBe('0.25');
    expect(json.saveStep).toBe(1);
    expect(json.method).toBe('rk4');

    const restored = SimSpecs.fromJson(json);
    expect(restored.start).toBe(0);
    expect(restored.stop).toBe(100);
    expect(restored.dt.value).toBe(0.25);
  });

  it('should handle reciprocal dt', () => {
    const specs = new SimSpecs({
      start: 0,
      stop: 10,
      dt: new Dt({ value: 4, isReciprocal: true }),
      saveStep: undefined,
      simMethod: 'euler',
      timeUnits: 'days',
    });

    const json = specs.toJson();
    expect(json.dt).toBe('1/4');

    const restored = SimSpecs.fromJson(json);
    expect(restored.dt.isReciprocal).toBe(true);
    expect(restored.dt.value).toBe(4);
  });
});

describe('Dimension', () => {
  it('should roundtrip with elements', () => {
    const dim = new Dimension({
      name: 'regions',
      subscripts: List(['north', 'south', 'east', 'west']),
    });

    const json = dim.toJson();
    expect(json.name).toBe('regions');
    expect(json.elements).toEqual(['north', 'south', 'east', 'west']);

    const restored = Dimension.fromJson(json);
    expect(restored.name).toBe('regions');
    expect(restored.subscripts.toArray()).toEqual(['north', 'south', 'east', 'west']);
  });
});

describe('Source', () => {
  it('should roundtrip correctly', () => {
    const source = new Source({
      extension: 'xmile',
      content: '<xmile>...</xmile>',
    });

    const json = source.toJson();
    expect(json.extension).toBe('xmile');
    expect(json.content).toBe('<xmile>...</xmile>');

    const restored = Source.fromJson(json);
    expect(restored.extension).toBe('xmile');
    expect(restored.content).toBe('<xmile>...</xmile>');
  });

  it('should handle empty source', () => {
    const source = new Source({
      extension: undefined,
      content: '',
    });

    const json = source.toJson();
    const restored = Source.fromJson(json);
    expect(restored.extension).toBeUndefined();
    expect(restored.content).toBe('');
  });
});

describe('LoopMetadata', () => {
  it('should roundtrip correctly', () => {
    const loop = new LoopMetadata({
      uids: List([1, 2, 3, 4, 1]),
      deleted: false,
      name: 'Growth Loop',
      description: 'Main reinforcing loop',
    });

    const json = loop.toJson();
    expect(json.uids).toEqual([1, 2, 3, 4, 1]);
    expect(json.name).toBe('Growth Loop');
    expect(json.description).toBe('Main reinforcing loop');
    expect(json.deleted).toBeUndefined();

    const restored = LoopMetadata.fromJson(json);
    expect(restored.uids.toArray()).toEqual([1, 2, 3, 4, 1]);
    expect(restored.name).toBe('Growth Loop');
    expect(restored.deleted).toBe(false);
  });

  it('should handle deleted loop', () => {
    const loop = new LoopMetadata({
      uids: List([1, 2]),
      deleted: true,
      name: 'Deleted Loop',
      description: '',
    });

    const json = loop.toJson();
    expect(json.deleted).toBe(true);

    const restored = LoopMetadata.fromJson(json);
    expect(restored.deleted).toBe(true);
  });
});

describe('StockFlowView', () => {
  it('should roundtrip with various element types', () => {
    const stockElem = new StockViewElement({
      uid: 1,
      name: 'Population',
      ident: 'population',
      var: undefined,
      x: 200,
      y: 200,
      labelSide: 'top',
      inflows: List([2]),
      outflows: List([]),
    });

    const flowElem = new FlowViewElement({
      uid: 2,
      name: 'Births',
      ident: 'births',
      var: undefined,
      x: 100,
      y: 200,
      labelSide: 'bottom',
      points: List([
        new Point({ x: 50, y: 200, attachedToUid: undefined }),
        new Point({ x: 150, y: 200, attachedToUid: 1 }),
      ]),
      isZeroRadius: false,
    });

    const auxElem = new AuxViewElement({
      uid: 3,
      name: 'BirthRate',
      ident: 'birth_rate',
      var: undefined,
      x: 100,
      y: 100,
      labelSide: 'right',
    });

    const linkElem = new LinkViewElement({
      uid: 4,
      fromUid: 3,
      toUid: 2,
      arc: 45,
      isStraight: false,
      multiPoint: undefined,
    });

    const cloudElem = new CloudViewElement({
      uid: 5,
      flowUid: 2,
      x: 50,
      y: 200,
    });

    const view = new StockFlowView({
      elements: List([stockElem, flowElem, auxElem, linkElem, cloudElem]),
      nextUid: 6,
      viewBox: new Rect({ x: 0, y: 0, width: 800, height: 600 }),
      zoom: 1.5,
    });

    const json = view.toJson();
    expect(json.elements).toHaveLength(5);
    expect(json.viewBox).toEqual({ x: 0, y: 0, width: 800, height: 600 });
    expect(json.zoom).toBe(1.5);

    const restored = StockFlowView.fromJson(json, Map());
    expect(restored.elements.size).toBe(5);
    expect(restored.viewBox.width).toBe(800);
    expect(restored.zoom).toBe(1.5);
  });
});

describe('Arrayed Equations', () => {
  it('should roundtrip Stock with ApplyToAllEquation', () => {
    const stock = new Stock({
      ident: 'inventory',
      equation: new ApplyToAllEquation({
        dimensionNames: List(['warehouses']),
        equation: '100',
      }),
      documentation: 'Inventory by warehouse',
      units: 'items',
      inflows: List([]),
      outflows: List([]),
      nonNegative: false,
      data: undefined,
      errors: undefined,
      unitErrors: undefined,
      uid: 1,
    });

    const json = stock.toJson();
    expect(json.arrayedEquation).toBeDefined();
    expect(json.arrayedEquation!.dimensions).toEqual(['warehouses']);
    expect(json.arrayedEquation!.equation).toBe('100');
    expect(json.initialEquation).toBeUndefined();

    const restored = Stock.fromJson(json);
    expect(restored.equation).toBeInstanceOf(ApplyToAllEquation);
    const eq = restored.equation as ApplyToAllEquation;
    expect(eq.dimensionNames.toArray()).toEqual(['warehouses']);
    expect(eq.equation).toBe('100');
  });

  it('should roundtrip Aux with ArrayedEquation', () => {
    const aux = new Aux({
      ident: 'demand',
      equation: new ArrayedEquation({
        dimensionNames: List(['regions']),
        elements: Map<string, string>([
          ['north', '50'],
          ['south', '75'],
        ]),
      }),
      documentation: 'Demand by region',
      units: '',
      gf: undefined,
      data: undefined,
      errors: undefined,
      unitErrors: undefined,
      uid: 2,
    });

    const json = aux.toJson();
    expect(json.arrayedEquation).toBeDefined();
    expect(json.arrayedEquation!.dimensions).toEqual(['regions']);
    expect(json.arrayedEquation!.elements).toHaveLength(2);

    const restored = Aux.fromJson(json);
    expect(restored.equation).toBeInstanceOf(ArrayedEquation);
    const eq = restored.equation as ArrayedEquation;
    expect(eq.dimensionNames.toArray()).toEqual(['regions']);
    expect(eq.elements.size).toBe(2);
    expect(eq.elements.get('north')).toBe('50');
    expect(eq.elements.get('south')).toBe('75');
  });
});

describe('LinkViewElement multiPoint', () => {
  it('should roundtrip with multiPoint path', () => {
    const elem = new LinkViewElement({
      uid: 10,
      fromUid: 1,
      toUid: 2,
      arc: undefined,
      isStraight: false,
      multiPoint: List([
        new Point({ x: 100, y: 100, attachedToUid: undefined }),
        new Point({ x: 150, y: 75, attachedToUid: undefined }),
        new Point({ x: 200, y: 100, attachedToUid: undefined }),
      ]),
    });

    const json = elem.toJson();
    expect(json.type).toBe('link');
    expect(json.arc).toBeUndefined();
    expect((json as any).multiPoints).toHaveLength(3);
    expect((json as any).multiPoints[1]).toEqual({ x: 150, y: 75 });

    const restored = LinkViewElement.fromJson(json);
    expect(restored.arc).toBeUndefined();
    expect(restored.isStraight).toBe(false);
    expect(restored.multiPoint).toBeDefined();
    expect(restored.multiPoint!.size).toBe(3);
    expect(restored.multiPoint!.get(1)?.x).toBe(150);
    expect(restored.multiPoint!.get(1)?.y).toBe(75);
  });
});

describe('Model', () => {
  it('should roundtrip correctly', () => {
    const stock = new Stock({
      ident: 'population',
      equation: new ScalarEquation({ equation: '100' }),
      documentation: '',
      units: 'people',
      inflows: List(['births']),
      outflows: List(['deaths']),
      nonNegative: false,
      data: undefined,
      errors: undefined,
      unitErrors: undefined,
      uid: 1,
    });

    const flow = new Flow({
      ident: 'births',
      equation: new ScalarEquation({ equation: 'population * 0.03' }),
      documentation: '',
      units: 'people/year',
      gf: undefined,
      nonNegative: false,
      data: undefined,
      errors: undefined,
      unitErrors: undefined,
      uid: 2,
    });

    const model = new Model({
      name: 'main',
      variables: Map<string, any>([
        ['population', stock],
        ['births', flow],
      ]),
      views: List(),
      loopMetadata: List(),
    });

    const json = model.toJson();
    expect(json.name).toBe('main');
    expect(json.stocks).toHaveLength(1);
    expect(json.flows).toHaveLength(1);

    const restored = Model.fromJson(json);
    expect(restored.name).toBe('main');
    expect(restored.variables.size).toBe(2);
    expect(restored.variables.get('population')).toBeInstanceOf(Stock);
    expect(restored.variables.get('births')).toBeInstanceOf(Flow);
  });
});

describe('Project', () => {
  it('should roundtrip a complete project', () => {
    const stock = new Stock({
      ident: 'population',
      equation: new ScalarEquation({ equation: '100' }),
      documentation: 'Population level',
      units: 'people',
      inflows: List(['births']),
      outflows: List([]),
      nonNegative: true,
      data: undefined,
      errors: undefined,
      unitErrors: undefined,
      uid: 1,
    });

    const flow = new Flow({
      ident: 'births',
      equation: new ScalarEquation({ equation: 'population * birth_rate' }),
      documentation: '',
      units: 'people/year',
      gf: undefined,
      nonNegative: true,
      data: undefined,
      errors: undefined,
      unitErrors: undefined,
      uid: 2,
    });

    const aux = new Aux({
      ident: 'birth_rate',
      equation: new ScalarEquation({ equation: '0.03' }),
      documentation: '',
      units: '1/year',
      gf: undefined,
      data: undefined,
      errors: undefined,
      unitErrors: undefined,
      uid: 3,
    });

    const model = new Model({
      name: 'main',
      variables: Map<string, any>([
        ['population', stock],
        ['births', flow],
        ['birth_rate', aux],
      ]),
      views: List(),
      loopMetadata: List(),
    });

    const simSpecs = new SimSpecs({
      start: 0,
      stop: 100,
      dt: new Dt({ value: 0.25, isReciprocal: false }),
      saveStep: new Dt({ value: 1, isReciprocal: false }),
      simMethod: 'rk4',
      timeUnits: 'years',
    });

    const project = new Project({
      name: 'growth_model',
      simSpecs,
      models: Map([['main', model]]),
      dimensions: Map(),
      hasNoEquations: false,
      source: new Source({ extension: 'xmile', content: '<xmile/>' }),
    });

    const json = project.toJson();
    expect(json.name).toBe('growth_model');
    expect(json.simSpecs.startTime).toBe(0);
    expect(json.simSpecs.endTime).toBe(100);
    expect(json.models).toHaveLength(1);
    expect(json.models[0].stocks).toHaveLength(1);
    expect(json.models[0].flows).toHaveLength(1);
    expect(json.models[0].auxiliaries).toHaveLength(1);
    expect(json.source?.extension).toBe('xmile');

    const restored = Project.fromJson(json);
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
    const model = new Model({
      name: 'main',
      variables: Map(),
      views: List(),
      loopMetadata: List(),
    });

    const project = new Project({
      name: 'empty',
      simSpecs: new SimSpecs({
        start: 0,
        stop: 10,
        dt: new Dt({ value: 1, isReciprocal: false }),
        saveStep: undefined,
        simMethod: 'euler',
        timeUnits: '',
      }),
      models: Map([['main', model]]),
      dimensions: Map(),
      hasNoEquations: true,
      source: undefined,
    });

    const json = project.toJson();
    const restored = Project.fromJson(json);

    expect(restored.models.get('main')?.variables.size).toBe(0);
    expect(restored.source).toBeUndefined();
  });
});
