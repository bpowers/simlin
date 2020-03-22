// Copyright 2019 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

/* eslint-disable @typescript-eslint/no-unused-vars */

import { List, Map, Record } from 'immutable';

import { canonicalize, defined, exists } from './common';

export const FlowSource = 0;
export const FlowSink = 1;

export const camelCase = (s: string): string => {
  let i = 0;
  while ((i = s.indexOf('_')) >= 0 && i < s.length - 1) {
    s = s.slice(0, i) + s.slice(i + 1, i + 2).toUpperCase() + s.slice(i + 2);
  }
  return s;
};

export const splitOnComma = (str: string): List<string> => {
  return List(str.split(',').map((el) => el.trim()));
};

export const numberize = (arr: List<string>): List<number> => {
  return List(arr.map((el) => parseFloat(el)));
};

export const i32 = (n: number): number => {
  return n | 0;
};

declare function isFinite(n: string | number): boolean;

// expects name to be lowercase
const attr = (el: Element, name: string): string | undefined => {
  for (let i = 0; i < el.attributes.length; i++) {
    const attr = el.attributes.item(i);
    if (!attr) {
      continue;
    }
    if (attr.name.toLowerCase() === name) {
      return attr.value;
    }
  }
  return undefined;
};

const content = (el: Element): string => {
  let text = '';
  if (el.hasChildNodes()) {
    for (let i = 0; i < el.childNodes.length; i++) {
      const child = el.childNodes.item(i);
      if (!child) {
        continue;
      }
      switch (child.nodeType) {
        case 3: // Text
          text += exists(child.nodeValue).trim();
          break;
        case 4: // CData
          text += child.nodeValue;
          break;
      }
    }
  }
  return text;
};

const num = (v: any): [number, undefined] | [number, Error] => {
  if (typeof v === 'undefined' || v === null) {
    return [0, undefined];
  }
  if (typeof v === 'number') {
    return [v, undefined];
  }
  const n = parseFloat(v);
  if (isFinite(n)) {
    return [n, undefined];
  }
  return [NaN, new Error('not number: ' + v)];
};

const bool = (v: any): [boolean, undefined] | [false, Error] => {
  if (typeof v === 'undefined' || v === null) {
    return [false, undefined];
  }
  if (typeof v === 'boolean') {
    return [v, undefined];
  }
  if (typeof v === 'string') {
    if (v === 'true') {
      return [true, undefined];
    } else if (v === 'false') {
      return [false, undefined];
    }
  }
  // XXX: should we accept 0 or 1?
  return [false, new Error('not boolean: ' + v)];
};

type XNode = {};

const PointDefaults = {
  x: -1,
  y: -1,
  uid: undefined as number | undefined,
};

// when constructing a point, we always want an x and a y, but the UID is optional
interface PointConstruction {
  x: number;
  y: number;
  uid?: number;
}

export class Point extends Record(PointDefaults) implements XNode {
  toJSON(): any {
    return {
      '@class': 'Point',
      data: super.toJSON(),
    };
  }

  static FromJSON(obj: any): Point {
    if (obj['@class'] !== 'Point' || !obj.data) {
      throw new Error('bad object');
    }
    return new Point(obj.data);
  }

  static FromXML(el: Element): [Point, undefined] | [undefined, Error] {
    let x: number | undefined;
    let y: number | undefined;
    let err: Error | undefined;

    for (let i = 0; i < el.attributes.length; i++) {
      const attr = el.attributes.item(i);
      if (!attr) {
        continue;
      }
      switch (attr.name.toLowerCase()) {
        case 'x':
          [x, err] = num(attr.value);
          if (err) {
            return [undefined, new Error(`x not num: ${err}`)];
          }
          break;
        case 'y':
          [y, err] = num(attr.value);
          if (err) {
            return [undefined, new Error(`y not num: ${err}`)];
          }
          break;
      }
    }
    if (x === undefined || y === undefined) {
      return [undefined, new Error(`expected both x and y on a Point`)];
    }

    return [new Point({ x, y }), undefined];
  }
}

const FileDefaults = {
  version: '1.0',
  namespace: 'https://docs.oasis-open.org/xmile/ns/XMILE/v1.0',
  header: undefined as Header | undefined,
  simSpec: undefined as SimSpec | undefined,
  dimensions: undefined as List<Dimension> | undefined,
  units: undefined as List<Unit> | undefined,
  behavior: undefined as Behavior | undefined,
  style: undefined as Style | undefined,
  models: List<Model>(),
};

export class File extends Record(FileDefaults) implements XNode {
  toJSON(): any {
    return {
      '@class': 'File',
      data: super.toJSON(),
    };
  }

  static FromJSON(obj: any): File {
    if (obj['@class'] !== 'File' || !obj.data) {
      throw new Error('bad object');
    }
    return new File(obj.data);
  }

  static FromXML(el: Element): [File, undefined] | [undefined, Error] {
    const file = Object.assign({}, FileDefaults);

    for (let i = 0; i < el.attributes.length; i++) {
      const attr = el.attributes.item(i);
      if (!attr) {
        continue;
      }
      switch (attr.name.toLowerCase()) {
        case 'version':
          file.version = exists(attr.value);
          break;
        case 'xmlns':
          file.namespace = exists(attr.value);
          break;
      }
    }

    for (let i = 0; i < el.childNodes.length; i++) {
      const child = el.childNodes.item(i) as Element;
      if (child.nodeType !== 1) {
        // Element
        continue;
      }
      switch (child.nodeName.toLowerCase()) {
        case 'header': {
          const [header, err] = Header.FromXML(child);
          if (err || !header) {
            return [undefined, new Error('Header: ' + err)];
          }
          file.header = header;
          break;
        }
        case 'sim_specs': {
          const [simSpec, err] = SimSpec.FromXML(child);
          if (err || !simSpec) {
            return [undefined, new Error('SimSpec: ' + err)];
          }
          file.simSpec = simSpec;
          break;
        }
        case 'model': {
          const [model, err] = Model.FromXML(child);
          if (err || !model) {
            return [undefined, new Error('SimSpec: ' + err)];
          }
          if (!file.models) {
            file.models = List();
          }
          file.models = defined(file.models).push(defined(model));
          break;
        }
      }
    }

    return [new File(file), undefined];
  }

  toXml(doc: XMLDocument, parent: Element): boolean {
    return true;
  }
}

const SimSpecDefaults = {
  start: 0,
  stop: 1,
  dt: 1,
  dtReciprocal: undefined as number | undefined, // the original reciprocal DT
  saveStep: 0 as number | undefined,
  method: 'euler' as string | undefined,
  timeUnits: undefined as string | undefined,
};

export class SimSpec extends Record(SimSpecDefaults) implements XNode {
  toJSON(): any {
    return {
      '@class': 'SimSpec',
      data: super.toJSON(),
    };
  }

  static FromXML(el: Element): [SimSpec, undefined] | [undefined, Error] {
    const simSpec = Object.assign({}, SimSpecDefaults);

    for (let i = 0; i < el.childNodes.length; i++) {
      const child = el.childNodes.item(i) as Element;
      if (child.nodeType !== 1) {
        // Element
        continue;
      }
      let name = camelCase(child.nodeName.toLowerCase());
      // XXX: hack for compat with some old models of mine
      if (name === 'savestep') {
        name = 'saveStep';
      }
      if (!simSpec.hasOwnProperty(name)) {
        continue;
      }

      if (name === 'method' || name === 'timeUnits') {
        simSpec[name] = content(child).toLowerCase();
      } else {
        const [val, err] = num(content(child));
        if (err || val === undefined) {
          return [undefined, new Error(child.nodeName + ': ' + err)];
        }
        (simSpec as any)[name] = val;
        if (name === 'dt') {
          if (attr(child, 'reciprocal') === 'true') {
            simSpec.dtReciprocal = simSpec.dt;
            simSpec.dt = 1 / simSpec.dt;
          }
        }
      }
    }

    if (!simSpec.saveStep) {
      simSpec.saveStep = simSpec.dt;
    }

    switch (simSpec.method) {
      // supported
      case 'euler':
        break;
      // valid, but not implemented
      case 'rk4':
      case 'rk2':
      case 'rk45':
      case 'gear':
        // FIXME:
        console.log(`valid but unsupported integration method: ${simSpec.method}; using 'euler'`);
        simSpec.method = 'euler';
        break;
      // unknown
      default:
        return [undefined, new Error(`unknown integration method ${simSpec.method}`)];
    }

    return [new SimSpec(simSpec), undefined];
  }

  toXml(doc: XMLDocument, parent: Element): boolean {
    return true;
  }
}

const UnitDefaults = {
  name: '',
  eqn: '',
  alias: undefined as string | undefined,
};

export class Unit extends Record(UnitDefaults) implements XNode {
  toJSON(): any {
    return {
      '@class': 'Unit',
      data: super.toJSON(),
    };
  }

  static FromXML(el: Element): [Unit, undefined] | [undefined, Error] {
    const unit = Object.assign({}, UnitDefaults);
    console.log('TODO: unit');
    return [new Unit(unit), undefined];
  }

  toXml(doc: XMLDocument, parent: Element): boolean {
    return true;
  }
}

const ProductDefaults = {
  name: 'unknown',
  lang: 'English',
  version: '',
};

export class Product extends Record(ProductDefaults) implements XNode {
  toJSON(): any {
    return {
      '@class': 'Product',
      data: super.toJSON(),
    };
  }

  static FromXML(el: Element): [Product, undefined] | [undefined, Error] {
    const product = Object.assign({}, ProductDefaults);
    product.name = content(el);
    for (let i = 0; i < el.attributes.length; i++) {
      const attr = el.attributes.item(i);
      if (!attr) {
        continue;
      }
      switch (attr.name.toLowerCase()) {
        case 'version':
          product.version = attr.value;
          break;
        case 'lang':
          product.lang = attr.value;
          break;
      }
    }
    return [new Product(product), undefined];
  }

  toXml(doc: XMLDocument, parent: Element): boolean {
    return true;
  }
}

const HeaderDefaults = {
  vendor: undefined as string | undefined,
  product: undefined as Product | undefined,
  options: undefined as Options | undefined,
  name: '' as string,
  version: undefined as string | undefined,
  caption: undefined as string | undefined, // WTF is this
  // image:    Image;
  author: undefined as string | undefined,
  affiliation: undefined as string | undefined,
  client: undefined as string | undefined,
  copyright: undefined as string | undefined,
  // contact:  Contact;
  created: undefined as string | undefined, // ISO 8601 date format, e.g. “ 2014-08-10”
  modified: undefined as string | undefined, // ISO 8601 date format
  uuid: undefined as string | undefined, // IETF RFC4122 format (84-4-4-12 hex digits with the dashes)
  // includes: List<Include>;
};

export class Header extends Record(HeaderDefaults) implements XNode {
  toJSON(): any {
    return {
      '@class': 'Header',
      data: super.toJSON(),
    };
  }

  static FromXML(el: Element): [Header, undefined] | [undefined, Error] {
    const header = Object.assign({}, HeaderDefaults);
    let err: Error | undefined;

    for (let i = 0; i < el.childNodes.length; i++) {
      const child = el.childNodes.item(i) as Element;
      if (child.nodeType !== 1) {
        // Element
        continue;
      }
      switch (child.nodeName.toLowerCase()) {
        case 'vendor':
          header.vendor = content(child);
          break;
        case 'product':
          [header.product, err] = Product.FromXML(child);
          if (err) {
            return [undefined, new Error('Product: ' + err)];
          }
          break;
        case 'options':
          [header.options, err] = Options.FromXML(child);
          if (err) {
            return [undefined, new Error('Options: ' + err)];
          }
          break;
        case 'name':
          header.name = content(child);
          break;
        case 'version':
          header.version = content(child);
          break;
        case 'caption':
          header.caption = content(child);
          break;
        case 'author':
          header.author = content(child);
          break;
        case 'affiliation':
          header.affiliation = content(child);
          break;
        case 'client':
          header.client = content(child);
          break;
        case 'copyright':
          header.copyright = content(child);
          break;
        case 'created':
          header.created = content(child);
          break;
        case 'modified':
          header.modified = content(child);
          break;
        case 'uuid':
          header.uuid = content(child);
          break;
      }
    }
    return [new Header(header), undefined];
  }

  toXml(doc: XMLDocument, parent: Element): boolean {
    return true;
  }
}

const DimensionDefaults = {
  name: '',
  size: '',
};

export class Dimension extends Record(DimensionDefaults) implements XNode {
  toJSON(): any {
    return {
      '@class': 'Dimension',
      data: super.toJSON(),
    };
  }

  static FromXML(el: Element): [Dimension, undefined] | [undefined, Error] {
    const dim = Object.assign({}, DimensionDefaults);
    // TODO: implement
    return [new Dimension(dim), undefined];
  }

  toXml(doc: XMLDocument, parent: Element): boolean {
    return true;
  }
}

const OptionsDefaults = {
  namespaces: undefined as List<string> | undefined,
  usesArrays: undefined as boolean | undefined,
  usesMacros: undefined as boolean | undefined,
  usesConveyor: undefined as boolean | undefined,
  usesQueue: undefined as boolean | undefined,
  usesSubmodels: undefined as boolean | undefined,
  usesEventPosters: undefined as boolean | undefined,
  hasModelView: undefined as boolean | undefined,
  usesOutputs: undefined as boolean | undefined,
  usesInputs: undefined as boolean | undefined,
  usesAnnotation: undefined as boolean | undefined,

  // arrays
  maximumDimensions: undefined as number | undefined,
  invalidIndexValue: undefined as number | undefined, // only 0 or NaN
  // macros
  recursiveMacros: undefined as boolean | undefined,
  optionFilters: undefined as boolean | undefined,
  // conveyors
  arrest: undefined as boolean | undefined,
  leak: undefined as boolean | undefined,
  // queues
  overflow: undefined as boolean | undefined,
  // event posters
  messages: undefined as boolean | undefined,
  // outputs
  numericDisplay: undefined as boolean | undefined,
  lamp: undefined as boolean | undefined,
  gauge: undefined as boolean | undefined,
  // inputs
  numericInput: undefined as boolean | undefined,
  list: undefined as boolean | undefined,
  graphicalInput: undefined as boolean | undefined,
};

export class Options extends Record(OptionsDefaults) implements XNode {
  toJSON(): any {
    return {
      '@class': 'Options',
      data: super.toJSON(),
    };
  }

  // avoids an 'implicit any' error when setting options in
  // FromXML below 'indexName' to avoid a spurious tslint
  // 'shadowed name' error.
  // [indexName: string]: any;

  static FromXML(el: Element): [Options, undefined] | [undefined, Error] {
    const options = Object.assign({}, OptionsDefaults);

    for (let i = 0; i < el.attributes.length; i++) {
      const attr = el.attributes.item(i);
      if (!attr) {
        continue;
      }
      switch (attr.name.toLowerCase()) {
        case 'namespace':
          options.namespaces = splitOnComma(attr.value);
          break;
      }
    }

    for (let i = 0; i < el.childNodes.length; i++) {
      const child = el.childNodes.item(i) as Element;
      if (child.nodeType !== 1) {
        // Element
        continue;
      }
      let name = child.nodeName.toLowerCase();
      let plen: number | undefined;
      if (name.startsWith('uses_')) {
        plen = 4;
      } else if (name.startsWith('has_')) {
        plen = 3;
      }
      if (!plen) {
        continue;
      }
      // use slice here even for the single char we
      // are camel-casing to avoid having to check
      // the length of the string
      name = camelCase(name);
      if (!options.hasOwnProperty(name)) {
        continue;
      }

      (options as any)[name] = true;

      if (name === 'usesArrays') {
        let val: string | undefined;
        val = attr(child, 'maximum_dimensions');
        if (val) {
          const [n, err] = num(val);
          if (err || !n) {
            // FIXME: real logging
            console.log('bad max_dimensions( ' + val + '): ' + err);
            options.maximumDimensions = 1;
          } else {
            if (n !== i32(n)) {
              console.log('non-int max_dimensions: ' + val);
            }
            options.maximumDimensions = i32(n);
          }
        }
        val = attr(child, 'invalid_index_value');
        if (val === 'NaN') {
          options.invalidIndexValue = NaN;
        }
      }
    }
    return [new Options(options), undefined];
  }

  toXml(doc: XMLDocument, parent: Element): boolean {
    return true;
  }
}

const BehaviorDefaults = {
  allNonNegative: undefined as boolean | undefined,
  stockNonNegative: undefined as boolean | undefined,
  flowNonNegative: undefined as boolean | undefined,
};

export class Behavior extends Record(BehaviorDefaults) implements XNode {
  toJSON(): any {
    return {
      '@class': 'Behavior',
      data: super.toJSON(),
    };
  }

  static FromXML(el: Element): [Behavior, undefined] | [undefined, Error] {
    const behavior = Object.assign({}, BehaviorDefaults);
    // TODO
    return [new Behavior(behavior), undefined];
  }

  toXml(doc: XMLDocument, parent: Element): boolean {
    return true;
  }
}

const DataDefaults = {};

export class Data extends Record(DataDefaults) implements XNode {
  toJSON(): any {
    return {
      '@class': 'Data',
      data: super.toJSON(),
    };
  }

  static FromXML(el: Element): [Data, undefined] | [undefined, Error] {
    const data = Object.assign({}, DataDefaults);
    console.log('TODO: data');
    return [new Data(data), undefined];
  }

  toXml(doc: XMLDocument, parent: Element): boolean {
    return true;
  }
}

const ModelDefaults = {
  name: 'main',
  run: undefined as boolean | undefined,
  namespaces: undefined as List<string> | undefined,
  resource: undefined as string | undefined, // path or URL to separate resource file
  simSpec: undefined as SimSpec | undefined,
  // behavior: Behavior;
  variables: List<Variable>(),
  views: List<View>(),
};

export function cloudFor(flow: ViewElement, dir: 'source' | 'sink', uid: number): ViewElement {
  let x: number | undefined;
  let y: number | undefined;
  if (flow.pts !== undefined && flow.pts.size > 0) {
    switch (dir) {
      case 'source':
        x = defined(flow.pts.get(0)).x;
        y = defined(flow.pts.get(0)).y;
        break;
      case 'sink':
        x = defined(flow.pts.get(flow.pts.size - 1)).x;
        y = defined(flow.pts.get(flow.pts.size - 1)).y;
        break;
    }
  }

  const element = new ViewElement({
    type: 'cloud',
    x: defined(x),
    y: defined(y),
    flowUid: flow.uid,
    uid,
  } as any);

  return element;
}

export class Model extends Record(ModelDefaults) implements XNode {
  toJSON(): any {
    return {
      '@class': 'Model',
      data: super.toJSON(),
    };
  }

  // this happens before populateNamedElements, so we can't reference namedElements
  // in here.
  getFlowEnds(stocks: Map<string, Variable>): Map<string, [UID | undefined, UID | undefined]> {
    const view = defined(this.views.get(0));
    const displayElements = view.elements.filter((e) => e.type === 'stock' || e.type === 'flow');
    const flows = displayElements.filter((e) => e.type === 'flow');
    const result = Map(
      flows.map((el: ViewElement): [string, [UID | undefined, UID | undefined]] => {
        return [el.ident, [undefined, undefined]];
      }),
    );

    for (const element of displayElements.filter((e) => e.type === 'stock')) {
      const stock = defined(stocks.get(element.ident));
      for (const inflow of stock.inflows || List<string>()) {
        if (!inflow) {
          console.log(`failed connecting ${inflow} in to ${element.ident}`);
          continue;
        }
        const ends = result.get(inflow);
        if (!ends) {
          console.log(`failed (2) connecting ${inflow} in to ${element.ident}`);
          continue;
        }
        if (ends[FlowSink] !== undefined) {
          console.log(`WARNING: multiple ends for same flow?`);
        }
        ends[FlowSink] = element.uid;
      }

      for (const outflow of stock.outflows || List<string>()) {
        if (!outflow) {
          console.log(`failed connecting ${outflow} out to ${element.ident}`);
          continue;
        }
        const ends = result.get(outflow);
        if (!ends) {
          console.log(`failed (2) connecting ${outflow} out to ${element.ident}`);
          continue;
        }
        if (ends[FlowSource] !== undefined) {
          console.log(`WARNING: multiple begins for same flow?`);
        }
        ends[FlowSource] = element.uid;
      }
    }

    return result;
  }

  getStocks(): Map<string, Variable> {
    return this.variables
      .filter((v) => v.type === 'stock')
      .toMap()
      .mapKeys((_, v) => defined(v.ident));
  }

  fixupClouds(): Model {
    // if we don't have any views, there is nothing to fixup
    if (this.views.size === 0) {
      return this;
    }
    const stocks = this.getStocks();
    const flowEnds = this.getFlowEnds(stocks);
    const views = this.views.map((view) => {
      let nextUid = view.nextUid || 1;
      const byUid = view.elements.toMap().mapKeys((_, e) => e.uid);
      let clouds = List<ViewElement>();

      const elements = view.elements.map((element) => {
        if (element.type !== 'flow') {
          return element;
        }
        // already fixed, no need to fixup further
        if (element.pts && element.pts.size > 0 && defined(element.pts.get(0)).uid) {
          return element;
        }
        const ends = defined(flowEnds.get(element.ident));
        const sourceId = ends[FlowSource];
        let source: ViewElement;
        if (sourceId) {
          source = defined(byUid.get(sourceId));
        } else {
          source = cloudFor(element, 'source', nextUid++);
          clouds = clouds.push(source);
        }

        const sinkId = ends[FlowSink];
        let sink: ViewElement;
        if (sinkId) {
          sink = defined(byUid.get(sinkId));
        } else {
          sink = cloudFor(element, 'sink', nextUid++);
          clouds = clouds.push(sink);
        }

        const pts = (element.pts || List<Point>()).map((pt, i) => {
          if (i === 0) {
            pt = pt.set('uid', source.uid);
          } else if (i === (element.pts ? element.pts.size - 1 : 0)) {
            pt = pt.set('uid', sink.uid);
          }
          return pt;
        });

        return element.set('pts', pts);
      });

      return view.merge({
        elements: elements.concat(clouds),
        nextUid,
      });
    });
    return this.set('views', views);
  }

  static FromXML(el: Element): [Model, undefined] | [undefined, Error] {
    const model = Object.assign({}, ModelDefaults);

    for (let i = 0; i < el.attributes.length; i++) {
      const attr = el.attributes.item(i);
      if (!attr) {
        continue;
      }
      switch (attr.name.toLowerCase()) {
        case 'name':
          model.name = attr.value;
          break;
      }
    }

    for (let i = 0; i < el.childNodes.length; i++) {
      const child = el.childNodes.item(i) as Element;
      if (child.nodeType !== 1) {
        // Element
        continue;
      }
      switch (child.nodeName.toLowerCase()) {
        case 'variables':
          for (let j = 0; j < child.childNodes.length; j++) {
            const vchild = child.childNodes.item(j) as Element;
            if (vchild.nodeType !== 1) {
              // Element
              continue;
            }
            if (typeof vchild.prefix !== 'undefined' && vchild.prefix === 'isee') {
              // isee specific info
              continue;
            }
            const [v, err] = Variable.FromXML(vchild);
            // FIXME: real logging
            if (err || !v) {
              return [undefined, new Error(child.nodeName + ' var: ' + err)];
            }
            model.variables = model.variables.push(v);
          }
          break;
        case 'views':
          for (let j = 0; j < child.childNodes.length; j++) {
            const vchild = child.childNodes.item(j) as Element;
            if (vchild.nodeType !== 1) {
              // Element
              continue;
            }
            // TODO: style parsing
            if (vchild.nodeName.toLowerCase() !== 'view') {
              continue;
            }
            const [view, err] = View.FromXML(vchild);
            // FIXME: real logging
            if (err || !view) {
              return [undefined, new Error('view: ' + err)];
            }
            model.views = model.views.push(view);
          }
          break;
      }
    }
    return [new Model(model), undefined];
  }

  get ident(): string {
    return canonicalize(this.name);
  }

  toXml(doc: XMLDocument, parent: Element): boolean {
    return true;
  }
}

const ArrayElementDefaults = {
  subscript: undefined as List<string> | undefined,
  eqn: undefined as string | undefined,
  gf: undefined as GF | undefined,
};

// the 'Element' name is defined by the TypeScript lib.d.ts, so we're
// forced to be more verbose.
export class ArrayElement extends Record(ArrayElementDefaults) implements XNode {
  toJSON(): any {
    return {
      '@class': 'ArrayElement',
      data: super.toJSON(),
    };
  }

  static FromXML(el: Element): [ArrayElement, undefined] | [undefined, Error] {
    const arrayEl = Object.assign({}, ArrayElementDefaults);
    console.log('TODO: array element');
    return [new ArrayElement(arrayEl), undefined];
  }

  toXml(doc: XMLDocument, parent: Element): boolean {
    return true;
  }
}

const RangeDefaults = {
  min: undefined as number | undefined,
  max: undefined as number | undefined,
  // auto + group only valid on 'scale' tags
  auto: undefined as boolean | undefined,
  group: undefined as number | undefined, // 'unique number identifier'
};

// Section 4.1.1 - Ranges, Scales, Number Formats
export class Range extends Record(RangeDefaults) implements XNode {
  toJSON(): any {
    return {
      '@class': 'Range',
      data: super.toJSON(),
    };
  }

  static FromXML(el: Element): [Range, undefined] | [undefined, Error] {
    const range = Object.assign({}, RangeDefaults);
    console.log('TODO: range element');
    return [new Range(range), undefined];
  }
  toXml(doc: XMLDocument, parent: Element): boolean {
    return true;
  }
}

const FormatDefaults = {
  precision: undefined as string | undefined, // "default: best guess based on the scale of the variable"
  scaleBy: undefined as string | undefined,
  displayAs: undefined as 'number' | 'currency' | 'percent' | undefined,
  delimit000s: undefined as boolean | undefined, // include thousands separator
};

// Section 4.1.1 - Ranges, Scales, Number Formats
export class Format extends Record(FormatDefaults) implements XNode {
  toJSON(): any {
    return {
      '@class': 'Format',
      data: super.toJSON(),
    };
  }

  static FromXML(el: Element): [Format, undefined] | [undefined, Error] {
    const fmt = Object.assign({}, FormatDefaults);
    console.log('TODO: format element');
    return [new Format(fmt), undefined];
  }

  toXml(doc: XMLDocument, parent: Element): boolean {
    return true;
  }
}

type VariableType = 'flow' | 'module' | 'stock' | 'aux' | 'connector' | 'reference';

const VariableDefaults = {
  type: 'aux' as VariableType,
  name: undefined as string | undefined,
  model: undefined as string | undefined,
  eqn: undefined as string | undefined,
  gf: undefined as GF | undefined,
  // mathml        Node;
  // arrayed-vars
  dimensions: undefined as List<Dimension> | undefined, // REQUIRED for arrayed vars
  elements: undefined as List<ArrayElement> | undefined, // non-A2A
  // modules
  connections: undefined as List<Connection> | undefined,
  resource: undefined as string | undefined, // path or URL to model XMILE file
  // access:       string;         // TODO: not sure if should implement
  // autoExport:   boolean;        // TODO: not sure if should implement
  units: undefined as Unit | undefined,
  doc: undefined as string | undefined, // 'or HTML', but HTML is not valid XML.  string-only.
  // eventPoster   EventPoster;
  range: undefined as Range | undefined,
  scale: undefined as Range | undefined,
  format: undefined as Format | undefined,
  // stocks
  nonNegative: undefined as boolean | undefined,
  inflows: undefined as List<string> | undefined,
  outflows: undefined as List<string> | undefined,
  // flows
  // multiplier:   string; // expression used on downstream side of stock to convert units
  // queues
  // overflow:     boolean;
  // leak:         string;
  // leakIntegers: boolean;
  // leakStart:    number;
  // leakEnd:      number;
  // auxiliaries
  flowConcept: undefined as boolean | undefined, // :(
};

// TODO: split into multiple subclasses?
export class Variable extends Record(VariableDefaults) implements XNode {
  toJSON(): any {
    return {
      '@class': 'Variable',
      data: super.toJSON(),
    };
  }

  static FromXML(el: Element): [Variable, undefined] | [undefined, Error] {
    const v = Object.assign({}, VariableDefaults);

    const typename = el.nodeName.toLowerCase();
    if (
      typename === 'aux' ||
      typename === 'stock' ||
      typename === 'flow' ||
      typename === 'module' ||
      typename === 'connector'
    ) {
      v.type = typename;
    } else {
      return [undefined, new Error(`unknown variable type: ${typename}`)];
    }

    for (let i = 0; i < el.attributes.length; i++) {
      const attr = el.attributes.item(i);
      if (!attr) {
        continue;
      }
      switch (attr.name.toLowerCase()) {
        case 'name':
          v.name = attr.value;
          break;
        case 'resource':
          v.resource = attr.value;
          break;
      }
    }

    for (let i = 0; i < el.childNodes.length; i++) {
      const child = el.childNodes.item(i) as Element;
      if (child.nodeType !== 1) {
        // Element
        continue;
      }
      switch (child.nodeName.toLowerCase()) {
        case 'eqn':
          v.eqn = content(child);
          break;
        case 'inflow':
          if (!v.inflows) {
            v.inflows = List();
          }
          v.inflows = v.inflows.push(canonicalize(content(child)));
          break;
        case 'outflow':
          if (!v.outflows) {
            v.outflows = List();
          }
          v.outflows = v.outflows.push(canonicalize(content(child)));
          break;
        case 'gf': {
          const [gf, err] = GF.FromXML(child);
          if (err || !gf) {
            return [undefined, new Error(v.name + ' GF: ' + err)];
          }
          v.gf = gf;
          break;
        }
        case 'connect': {
          const [conn, err] = Connection.FromXML(child);
          if (err || !conn) {
            return [undefined, new Error(v.name + ' conn: ' + err)];
          }
          if (!v.connections) {
            v.connections = List<Connection>();
          }
          v.connections = v.connections.push(conn);
          break;
        }
      }
    }

    return [new Variable(v), undefined];
  }

  get ident(): string | undefined {
    return this.name ? canonicalize(this.name) : undefined;
  }

  toXml(doc: XMLDocument, parent: Element): boolean {
    return true;
  }
}

const ShapeDefaults = {
  type: 'circle' as 'rectangle' | 'circle' | 'name_only',
  width: undefined as number | undefined,
  height: undefined as number | undefined,
  radius: undefined as number | undefined,
};

export class Shape extends Record(ShapeDefaults) implements XNode {
  toJSON(): any {
    return {
      '@class': 'Shape',
      data: super.toJSON(),
    };
  }

  static FromXML(el: Element): [Shape, undefined] | [undefined, Error] {
    const shape = Object.assign({}, ShapeDefaults);
    let err: Error | undefined;

    for (let i = 0; i < el.attributes.length; i++) {
      const attr = el.attributes.item(i);
      if (!attr) {
        continue;
      }
      switch (attr.name.toLowerCase()) {
        case 'type': {
          const type = attr.value.toLowerCase();
          if (type !== 'rectangle' && type !== 'circle' && type !== 'name_only') {
            return [undefined, new Error(`bad shape type: ${type}`)];
          }
          shape.type = type;
          break;
        }
        case 'width':
          [shape.width, err] = num(attr.value);
          if (err) {
            return [undefined, new Error('bad width: ' + err)];
          }
          break;
        case 'height':
          [shape.height, err] = num(attr.value);
          if (err) {
            return [undefined, new Error('bad height: ' + err)];
          }
          break;
        case 'radius':
          [shape.radius, err] = num(attr.value);
          if (err) {
            return [undefined, new Error('bad radius: ' + err)];
          }
          break;
      }
    }
    return [new Shape(shape), undefined];
  }

  toXml(doc: XMLDocument, parent: Element): boolean {
    return true;
  }
}

const StyleDefaults = {
  background: undefined as string | undefined,
  color: undefined as string | undefined,
  fontFamily: undefined as string | undefined,
  fontSize: undefined as string | undefined,
  fontWeight: undefined as string | undefined,
  textAlign: undefined as string | undefined,
  textDecoration: undefined as string | undefined,
  margin: undefined as string | undefined,
  padding: undefined as string | undefined,
  borderColor: undefined as string | undefined,
  borderStyle: undefined as string | undefined,
  borderWidth: undefined as string | undefined,
};

export class Style extends Record(StyleDefaults) implements XNode {
  toJSON(): any {
    return {
      '@class': 'Style',
      data: super.toJSON(),
    };
  }

  static FromXML(el: Element): [Style, undefined] | [undefined, Error] {
    return [new Style(Object.assign({}, StyleDefaults)), undefined];
  }
}

export type ViewElementType = VariableType | 'cloud' | 'style' | 'alias';

const ViewElementDefaults = {
  type: 'aux' as ViewElementType,
  name: undefined as string | undefined,
  uid: -1 as UID, // int
  x: undefined as number | undefined,
  y: undefined as number | undefined,
  width: undefined as number | undefined,
  height: undefined as number | undefined,
  style: undefined as Style | undefined,
  shape: undefined as Shape | undefined,
  zIndex: undefined as number | undefined, // default of -1, range of -1 to INT_MAX
  labelSide: undefined as 'top' | 'left' | 'center' | 'bottom' | 'right' | undefined,
  labelAngle: undefined as number | undefined, // degrees where 0 is 3 o'clock, counter-clockwise.
  // connectors
  from: undefined as string | undefined, // ident
  to: undefined as string | undefined, // ident
  angle: undefined as number | undefined, // degrees
  // flows + multi-point connectors
  pts: undefined as List<Point> | undefined,
  // alias
  of: undefined as string | undefined,
  // cloud
  flowUid: undefined as number | undefined,
  // for moving flows + canvas
  isZeroRadius: false,
};

export class ViewElement extends Record(ViewElementDefaults) implements XNode {
  get hasUid(): boolean {
    return !!(this.uid && this.uid !== -1);
  }

  // while we don't say it in our default types, we maintain this invariant elsewhere.
  // get uid(): number {
  //   if (super.uid === undefined) {
  //     throw new Error(`element '${this.name}' with no UID`);
  //   }
  //   return super.uid;
  // }

  toJSON(): any {
    return {
      '@class': 'ViewElement',
      data: super.toJSON(),
    };
  }

  static FromXML(el: Element): [ViewElement, undefined] | [undefined, Error] | [undefined, undefined] {
    const viewEl = Object.assign({}, ViewElementDefaults);
    let err: Error | undefined;

    const typename = el.nodeName.toLowerCase();
    if (
      typename === 'aux' ||
      typename === 'stock' ||
      typename === 'flow' ||
      typename === 'module' ||
      typename === 'connector' ||
      typename === 'style' ||
      typename === 'alias'
    ) {
      viewEl.type = typename;
    } else if (
      typename === 'text_box' ||
      typename === 'stacked_container' ||
      typename === 'graph' ||
      typename === 'table' ||
      typename === 'button' ||
      typename === 'isee:loop_indicator' ||
      typename === 'graphics_frame' ||
      typename === 'slider'
    ) {
      // TODO(bpowers)
      return [undefined, undefined];
    } else {
      return [undefined, new Error(`unknown variable type: ${typename}`)];
    }

    for (let i = 0; i < el.attributes.length; i++) {
      const attr = el.attributes.item(i);
      if (!attr) {
        continue;
      }
      switch (attr.name.toLowerCase()) {
        case 'name':
          // display-name, not canonicalized
          viewEl.name = attr.value;
          break;
        case 'uid':
          [viewEl.uid, err] = num(attr.value);
          if (err) {
            return [undefined, new Error('uid: ' + err)];
          }
          break;
        case 'x':
          [viewEl.x, err] = num(attr.value);
          if (err) {
            return [undefined, new Error('x: ' + err)];
          }
          break;
        case 'y':
          [viewEl.y, err] = num(attr.value);
          if (err) {
            return [undefined, new Error('y: ' + err)];
          }
          break;
        case 'width':
          [viewEl.width, err] = num(attr.value);
          if (err) {
            return [undefined, new Error('width: ' + err)];
          }
          break;
        case 'height':
          [viewEl.height, err] = num(attr.value);
          if (err) {
            return [undefined, new Error('height: ' + err)];
          }
          break;
        case 'label_side': {
          const val = attr.value.toLowerCase();
          if (val !== 'left' && val !== 'right' && val !== 'center' && val !== 'top' && val !== 'bottom') {
            return [undefined, new Error(`unknown label_side: ${val}`)];
          }
          viewEl.labelSide = val;
          break;
        }
        case 'label_angle':
          [viewEl.labelAngle, err] = num(attr.value);
          if (err) {
            return [undefined, new Error('label_angle: ' + err)];
          }
          break;
        case 'color':
          break;
        case 'angle':
          [viewEl.angle, err] = num(attr.value);
          if (err) {
            return [undefined, new Error('angle: ' + err)];
          }
          break;
      }
    }

    for (let i = 0; i < el.childNodes.length; i++) {
      const child = el.childNodes.item(i) as Element;
      if (child.nodeType !== 1) {
        // Element
        continue;
      }

      switch (child.nodeName.toLowerCase()) {
        case 'to':
          viewEl.to = canonicalize(content(child));
          break;
        case 'from':
          viewEl.from = canonicalize(content(child));
          break;
        case 'of':
          viewEl.of = canonicalize(content(child));
          break;
        case 'pts':
          for (let j = 0; j < child.childNodes.length; j++) {
            const vchild = child.childNodes.item(j) as Element;
            if (vchild.nodeType !== 1) {
              // Element
              continue;
            }
            if (vchild.nodeName.toLowerCase() !== 'pt') {
              continue;
            }
            const [pt, err] = Point.FromXML(vchild);
            // FIXME: real logging
            if (err || !pt) {
              return [undefined, new Error('pt: ' + err)];
            }
            if (!viewEl.pts) {
              viewEl.pts = List<Point>();
            }
            viewEl.pts = viewEl.pts.push(defined(pt));
          }
          break;
        case 'shape':
          const [shape, err] = Shape.FromXML(child);
          if (err || !shape) {
            return [undefined, new Error('shape: ' + err)];
          }
          viewEl.shape = shape;
          break;
      }
    }

    return [new ViewElement(viewEl), undefined];
  }

  get hasName(): boolean {
    return this.name !== undefined;
  }

  get ident(): string {
    return canonicalize(defined(this.name));
  }

  get cx(): number {
    switch (this.type) {
      case 'aux':
      case 'flow':
      case 'module':
      case 'cloud':
        return defined(this.x);
      case 'stock':
        if (this.width) {
          return defined(this.x) + 0.5 * defined(this.width);
        } else {
          return defined(this.x);
        }
    }
    return NaN;
  }

  get cy(): number {
    switch (this.type) {
      case 'aux':
      case 'flow':
      case 'module':
      case 'cloud':
        return defined(this.y);
      case 'stock':
        if (this.width) {
          return defined(this.y) + 0.5 * defined(this.height);
        } else {
          return defined(this.y);
        }
    }
    return NaN;
  }

  toXml(doc: XMLDocument, parent: Element): boolean {
    return true;
  }
}

export const ViewDefaults = {
  type: 'stock_flow',
  order: undefined as number | undefined,
  width: undefined as number | undefined,
  height: undefined as number | undefined,
  zoom: 100,
  scrollX: 0,
  scrollY: 0,
  background: undefined as string | undefined,
  pageWidth: undefined as number | undefined,
  pageHeight: undefined as number | undefined,
  pageSequence: undefined as 'row' | 'column' | undefined,
  pageOrientation: undefined as 'landscape' | 'portrait' | undefined,
  showPages: undefined as boolean | undefined,
  homePage: undefined as number | undefined,
  homeView: undefined as boolean | undefined,
  elements: List<ViewElement>(),
  nextUid: 1,
};

export class View extends Record(ViewDefaults) implements XNode {
  constructor(view: typeof ViewDefaults) {
    const uids = view.elements.filter((e) => e.hasUid).map((e) => e.uid);
    let nextUid = uids.reduce((a, b) => Math.max(a, b), 0) + 1;
    view.elements = view.elements.map((e) => {
      if (e.hasUid) {
        return e;
      }
      return e.set('uid', nextUid++);
    });
    view.nextUid = nextUid;

    super(view);
  }

  toJSON(): any {
    return {
      '@class': 'View',
      data: super.toJSON(),
    };
  }

  static FromXML(el: Element): [View, undefined] | [undefined, Error] {
    const view: typeof ViewDefaults = Object.assign({}, ViewDefaults);
    let err: Error | undefined;

    for (let i = 0; i < el.attributes.length; i++) {
      const attr = el.attributes.item(i);
      if (!attr) {
        continue;
      }
      switch (attr.name.toLowerCase()) {
        case 'type':
          view.type = attr.value.toLowerCase();
          break;
        case 'order':
          [view.order, err] = num(attr.value);
          if (err) {
            return [undefined, new Error('order: ' + err)];
          }
          break;
        case 'width':
          [view.width, err] = num(attr.value);
          if (err) {
            return [undefined, new Error('width: ' + err)];
          }
          break;
        case 'height':
          [view.height, err] = num(attr.value);
          if (err) {
            return [undefined, new Error('height: ' + err)];
          }
          break;
        case 'zoom':
          [view.zoom, err] = num(attr.value);
          if (err) {
            return [undefined, new Error('zoom: ' + err)];
          }
          break;
        case 'scroll_x':
          [view.scrollX, err] = num(attr.value);
          if (err) {
            return [undefined, new Error('scroll_x: ' + err)];
          }
          break;
        case 'scroll_y':
          [view.scrollY, err] = num(attr.value);
          if (err) {
            return [undefined, new Error('scroll_y: ' + err)];
          }
          break;
        case 'background':
          view.background = attr.value.toLowerCase();
          break;
        case 'page_width':
          [view.pageWidth, err] = num(attr.value);
          if (err) {
            return [undefined, new Error('page_width: ' + err)];
          }
          break;
        case 'page_height':
          [view.pageHeight, err] = num(attr.value);
          if (err) {
            return [undefined, new Error('page_height: ' + err)];
          }
          break;
        case 'page_sequence': {
          const val = attr.value.toLowerCase();
          if (val !== 'row' && val !== 'column') {
            return [undefined, new Error(`unknown page_sequence type: ${val}`)];
          }
          view.pageSequence = val;
          break;
        }
        case 'page_orientation': {
          const val = attr.value.toLowerCase();
          if (val !== 'landscape' && val !== 'portrait') {
            return [undefined, new Error(`unknown page_sequence type: ${val}`)];
          }
          view.pageOrientation = val;
          break;
        }
        case 'show_pages':
          [view.showPages, err] = bool(attr.value);
          if (err) {
            return [undefined, new Error('show_pages: ' + err)];
          }
          break;
        case 'home_page':
          [view.homePage, err] = num(attr.value);
          if (err) {
            return [undefined, new Error('home_page: ' + err)];
          }
          break;
        case 'home_view':
          [view.homeView, err] = bool(attr.value);
          if (err) {
            return [undefined, new Error('home_view: ' + err)];
          }
          break;
      }
    }

    for (let i = 0; i < el.childNodes.length; i++) {
      const child = el.childNodes.item(i) as Element;
      if (child.nodeType !== 1) {
        // Element
        continue;
      }

      // ignore isee children and weird old things
      if (child.prefix === 'isee' || child.nodeName === 'simulation_delay') {
        continue;
      }

      let viewEl: ViewElement | undefined;
      [viewEl, err] = ViewElement.FromXML(child);
      if (err) {
        return [undefined, new Error('viewEl: ' + err)];
      } else if (!viewEl) {
        continue;
      }
      if (!view.elements) {
        view.elements = List<ViewElement>();
      }
      view.elements = view.elements.push(viewEl);
    }

    return [new View(view), undefined];
  }

  toXml(doc: XMLDocument, parent: Element): boolean {
    return true;
  }
}

type GFType = 'continuous' | 'extrapolate' | 'discrete';

const GFDefaults = {
  name: '' as string | undefined,
  type: 'continuous' as GFType,
  xPoints: undefined as List<number> | undefined,
  yPoints: undefined as List<number> | undefined,
  xScale: undefined as Scale | undefined,
  yScale: undefined as Scale | undefined, // only affects the scale of the graph in the UI
};

export class GF extends Record(GFDefaults) implements XNode {
  toJSON(): any {
    return {
      '@class': 'GF',
      data: super.toJSON(),
    };
  }

  static FromXML(el: Element): [GF, undefined] | [undefined, Error] {
    const table: typeof GFDefaults = Object.assign({}, GFDefaults);
    let err: Error | undefined;

    for (let i = 0; i < el.attributes.length; i++) {
      const attr = el.attributes.item(i);
      if (!attr) {
        continue;
      }
      switch (attr.name.toLowerCase()) {
        case 'type':
          const kind = attr.value.toLowerCase();
          if (kind === 'discrete' || kind === 'continuous' || kind === 'extrapolate') {
            table.type = kind;
          } else {
            return [undefined, new Error(`bad GF type: ${kind}`)];
          }
          break;
      }
    }

    for (let i = 0; i < el.childNodes.length; i++) {
      const child = el.childNodes.item(i) as Element;
      if (child.nodeType !== 1) {
        // Element
        continue;
      }
      switch (child.nodeName.toLowerCase()) {
        case 'xscale':
          [table.xScale, err] = Scale.FromXML(child);
          if (err) {
            return [undefined, new Error(`xscale: ${err}`)];
          }
          break;
        case 'yscale':
          [table.yScale, err] = Scale.FromXML(child);
          if (err) {
            return [undefined, new Error(`yscale: ${err}`)];
          }
          break;
        case 'xpts':
          table.xPoints = numberize(splitOnComma(content(child)));
          break;
        case 'ypts':
          table.yPoints = numberize(splitOnComma(content(child)));
          break;
      }
    }

    if (table.yPoints === undefined) {
      return [undefined, new Error('table missing ypts')];
    }

    // FIXME: handle
    if (table.type && table.type !== 'continuous') {
      console.log('WARN: unimplemented table type: ' + table.type);
    }

    return [new GF(table), undefined];
  }
}

const ScaleDefaults = {
  min: -1,
  max: -1,
};

export class Scale extends Record(ScaleDefaults) implements XNode {
  toJSON(): any {
    return {
      '@class': 'Scale',
      data: super.toJSON(),
    };
  }

  static FromXML(el: Element): [Scale, undefined] | [undefined, Error] {
    let min: number | undefined;
    let max: number | undefined;
    let err: Error | undefined;

    for (let i = 0; i < el.attributes.length; i++) {
      const attr = el.attributes.item(i);
      if (!attr) {
        continue;
      }
      switch (attr.name.toLowerCase()) {
        case 'min':
          [min, err] = num(attr.value);
          if (err) {
            return [undefined, new Error(`bad min: ${attr.value}`)];
          }
          break;
        case 'max':
          [max, err] = num(attr.value);
          if (err) {
            return [undefined, new Error(`bad max: ${attr.value}`)];
          }
          break;
      }
    }

    if (min === undefined || max === undefined) {
      return [undefined, new Error('scale requires both min and max')];
    }

    return [new Scale({ min, max }), undefined];
  }
}

const ConnectionDefaults = {
  from: '',
  to: '',
};

export class Connection extends Record(ConnectionDefaults) implements XNode {
  toJSON(): any {
    return {
      '@class': 'Connection',
      data: super.toJSON(),
    };
  }

  static FromXML(el: Element): [Connection, undefined] | [undefined, Error] {
    let from: string | undefined;
    let to: string | undefined;

    for (let i = 0; i < el.attributes.length; i++) {
      const attr = el.attributes.item(i);
      if (!attr) {
        continue;
      }
      switch (attr.name.toLowerCase()) {
        case 'to':
          to = canonicalize(attr.value);
          break;
        case 'from':
          from = canonicalize(attr.value);
          break;
      }
    }

    if (to === undefined || from === undefined) {
      return [undefined, new Error('connect requires both to and from')];
    }

    return [new Connection({ from, to }), undefined];
  }

  toXml(doc: XMLDocument, parent: Element): boolean {
    return true;
  }
}

const TypeRegistry = Map<string, XNode>([
  ['Point', Point],
  ['File', File],
  ['SimSpec', SimSpec],
  ['Unit', Unit],
  ['Product', Product],
  ['Header', Header],
  ['Dimension', Dimension],
  ['Options', Options],
  ['Behavior', Behavior],
  ['Data', Data],
  ['Model', Model],
  ['ArrayElement', ArrayElement],
  ['Range', Range],
  ['Format', Format],
  ['Variable', Variable],
  ['ViewElement', ViewElement],
  ['View', View],
  ['GF', GF],
  ['Scale', Scale],
  ['Connection', Connection],
]);

export type UID = number;

const FromJSON = (json: any): [any, undefined] | [undefined, Error] => {
  switch (typeof json) {
    case 'number':
    case 'boolean':
    case 'string':
    case 'symbol':
    case 'undefined':
    case 'function':
      return [json, undefined];
  }

  if (Array.isArray(json)) {
    const result = [];
    for (const item of json) {
      const [obj, err] = FromJSON(item);
      if (err !== undefined) {
        return [undefined, err];
      } else {
        result.push(obj);
      }
    }
    return [List(result), undefined];
  }

  const className: string | undefined = json['@class'];
  if (className === undefined) {
    return [undefined, new Error(`no class`)];
  } else if (!TypeRegistry.has(json['@class'])) {
    return [undefined, new Error(`unknown class '${className}'`)];
  } else if (json.data === undefined) {
    return [undefined, new Error(`no data for class ${className}`)];
  }

  const data = Object.assign({}, json.data);
  for (const key in data) {
    if (!data.hasOwnProperty(key)) {
      continue;
    }
    const value = data[key];
    if (value === undefined || value === null) {
      continue;
    }

    const [newValue, err] = FromJSON(value);
    if (err !== undefined) {
      return [undefined, err];
    }
    data[key] = newValue;
  }
  const Kind: any = defined(TypeRegistry.get(className));
  const object = new Kind(data);
  return [object, undefined];
};

export function FileFromJSON(json: any): File {
  const [file, err] = FromJSON(json);
  if (err) {
    throw err;
  }

  return file;
}
