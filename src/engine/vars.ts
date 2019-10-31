// Copyright 2019 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { List, Map, Record, Set } from 'immutable';

import { builtins, defined } from './common';
import { titleCase } from './util';

import * as ast from './ast';
import { eqn as parseEqn } from './parse';
import * as xmile from './xmile';

// FIXME: this seems to fix a bug in Typescript 1.5
declare function isFinite(n: string | number): boolean;

export interface Project {
  readonly name: string;
  readonly simSpec: xmile.SimSpec;
  readonly main: Module | undefined;

  model(name?: string): Model | undefined;
  toFile(): xmile.File;
}

export interface Model {
  readonly ident: string;
  readonly valid: boolean;
  readonly modules: Map<string, Module>;
  readonly tables: Map<string, Table>;
  readonly vars: Map<string, Variable>;

  readonly simSpec?: xmile.SimSpec;

  parse(project: Project): [Map<string, Variable>, Map<string, Module>, Map<string, Table>];
  view(index: number): xmile.View | undefined;
}

interface ModelDefProps {
  model: Model | undefined;
  modules: Set<Module>;
}

const modelDefDefaults: ModelDefProps = {
  model: undefined,
  modules: Set<Module>(),
};

export class ModelDef extends Record(modelDefDefaults) {
  constructor(params: ModelDefProps) {
    super(params);
  }

  get<T extends keyof ModelDefProps>(key: T): ModelDefProps[T] {
    return super.get(key);
  }

  monomorphizations(): Map<Set<string>, string> {
    let n = 0;
    let mms = Map<Set<string>, string>();

    for (const module of this.modules) {
      const inputs = Set(module.refs.keys());
      if (!mms.has(inputs)) {
        const mononame = titleCase(defined(this.model).ident + '_' + n);
        // console.log(`// mono: ${defined(this.model).ident}<${inputs.join(',')}>: ${n}`);
        mms = mms.set(inputs, mononame);
        n++;
      }
    }

    return mms;
  }
}

const contextDefaults = {
  project: (null as any) as Project,
  models: List<Model>(),
  isInitials: false,
};

export class Context extends Record(contextDefaults) {
  constructor(project: Project, model: Model, isInitials: boolean, prevContext?: Context) {
    const models = prevContext ? prevContext.models : List<Model>();
    super({
      project,
      models: models.push(model),
      isInitials,
    });
  }

  get parent(): Model {
    return defined(this.models.last());
  }

  get mainModel(): Model {
    const main = defined(this.project.main);
    return defined(this.project.model(main.modelName));
  }

  lookup(ident: string): Variable | undefined {
    if (ident[0] === '.') {
      ident = ident.substr(1);
      return new Context(this.project, this.mainModel, this.isInitials).lookup(ident);
    }

    const model = this.parent;
    if (model.vars.has(ident)) {
      return model.vars.get(ident);
    }
    const parts = ident.split('.');
    const module = model.modules.get(parts[0]);
    if (!module) {
      return undefined;
    }
    const nextModel = this.project.model(module.modelName);
    if (!nextModel) {
      return undefined;
    }
    return new Context(this.project, nextModel, this.isInitials).lookup(parts.slice(1).join('.'));
  }
}

export type VariableKind = 'ordinary' | 'stock' | 'table' | 'module' | 'reference';

interface OrdinaryProps {
  kind: VariableKind;
  xmile?: xmile.Variable;
  valid: boolean;
  ast?: ast.Node;
  errors: List<string>;
  deps: Set<string>;
}

const variableDefaults: OrdinaryProps = {
  kind: 'ordinary',
  xmile: undefined,
  valid: false,
  ast: undefined,
  errors: List<string>(),
  deps: Set<string>(),
};

function variableFrom(xVar: xmile.Variable | undefined, kind: VariableKind): OrdinaryProps {
  const variable = Object.assign({}, variableDefaults);
  variable.kind = kind;
  variable.xmile = xVar;

  const ident = xVar && xVar.name ? xVar.ident : undefined;
  const eqn: string | undefined = xVar && xVar.eqn;

  if (eqn !== undefined) {
    const [ast, errs] = parseEqn(eqn);
    if (ast) {
      variable.ast = ast || undefined;
      variable.valid = true;
    }
    if (errs) {
      variable.errors = variable.errors.concat(List(errs));
    }
  }

  if (!eqn) {
    variable.errors = variable.errors.push('Missing equation');
  }

  // for a flow or aux, we depend on variables that aren't built-in
  // functions in the equation.
  if (xVar && xVar.type === 'module') {
    variable.deps = Set<string>();
    if (xVar.connections) {
      for (const conn of xVar.connections) {
        const ref = new Reference(conn);
        variable.deps = variable.deps.add(ref.ptr);
      }
    }
  } else {
    variable.deps = identifierSet(variable.ast);
  }

  return variable;
}

// Ordinary variables are either auxiliaries or flows -- they are
// represented the same way.
export class Ordinary extends Record(variableDefaults) {
  constructor(xVar?: xmile.Variable) {
    const variable = variableFrom(xVar, 'ordinary');
    super(variable);
  }

  get ident(): string | undefined {
    return this.xmile ? this.xmile.ident : undefined;
  }
}

const stockOnlyDefaults = {
  inflows: List<string>(),
  outflows: List<string>(),
};
const stockDefaults = {
  ...variableDefaults,
  ...stockOnlyDefaults,
};

export class Stock extends Record(stockDefaults) {
  constructor(xVar: xmile.Variable) {
    const variable = variableFrom(xVar, 'stock');
    const stock = {
      ...variable,
      inflows: xVar.inflows || List(),
      outflows: xVar.outflows || List(),
    };
    super(stock);
  }

  get ident(): string | undefined {
    return this.xmile ? this.xmile.ident : undefined;
  }
}

const tableOnlyDefaults = {
  x: List<number>(),
  y: List<number>(),
};
const tableDefaults = {
  ...variableDefaults,
  ...tableOnlyDefaults,
};

// An ordinary variable with an attached table
export class Table extends Record(tableDefaults) {
  constructor(xVar: xmile.Variable) {
    const variable = variableFrom(xVar, 'table');

    const gf = defined(xVar.gf);
    const ypts = gf.yPoints;

    // FIXME(bp) unit test
    const xpts = gf.xPoints;
    const xscale = gf.xScale;
    const xmin = xscale ? xscale.min : 0;
    const xmax = xscale ? xscale.max : 0;

    let xList = List<number>();
    let yList = List<number>();
    let ok = true;

    if (ypts) {
      for (let i = 0; i < ypts.size; i++) {
        let x: number;
        // either the x points have been explicitly specified, or
        // it is a linear mapping of points between xmin and xmax,
        // inclusive
        if (xpts) {
          x = defined(xpts.get(i));
        } else {
          x = (i / (ypts.size - 1)) * (xmax - xmin) + xmin;
        }
        xList = xList.push(x);
        yList = yList.push(defined(ypts.get(i)));
      }
    } else {
      ok = false;
    }

    const table = {
      ...variable,
      x: xList,
      y: yList,
      valid: variable.valid && ok,
    };
    super(table);
  }

  get ident(): string | undefined {
    return this.xmile ? this.xmile.ident : undefined;
  }
}

const moduleOnlyDefaults = {
  refs: Map<string, Reference>(),
};
const moduleDefaults = {
  ...variableDefaults,
  ...moduleOnlyDefaults,
};

export class Module extends Record(moduleDefaults) {
  constructor(xVar: xmile.Variable) {
    const variable = variableFrom(xVar, 'module');

    let refs = Map<string, Reference>();
    if (xVar.connections) {
      for (const conn of xVar.connections) {
        const ref = new Reference(conn);
        refs = refs.set(defined(ident(ref)), ref);
      }
    }

    const mod = {
      ...variable,
      refs,
    };

    super(mod);
  }

  // This is a deviation from the XMILE spec, but is the only thing
  // that makes sense -- having a 1 to 1 relationship between model
  // name and module name would be insane.
  get modelName(): string {
    if (!this.xmile) {
      throw new Error('modelName called on Module without xmile');
    }
    return this.xmile.model ? this.xmile.model : defined(this.xmile.ident);
  }
}

const referenceOnlyDefaults = {
  xmileConn: (undefined as any) as xmile.Connection,
};
const referenceDefaults = {
  ...variableDefaults,
  ...referenceOnlyDefaults,
};

export class Reference extends Record(referenceDefaults) {
  constructor(conn: xmile.Connection) {
    const variable = variableFrom(new xmile.Variable({ name: conn.to }), 'reference');
    const reference = {
      ...variable,
      xmileConn: conn,
    };
    super(reference);
  }

  get ptr(): string {
    return this.xmileConn.from;
  }

  get ident(): string | undefined {
    return this.ptr;
  }
}

export type Variable = Ordinary | Stock | Table | Module | Reference;

// ---------------------------------------------------------------------

const JsOps: Map<string, string> = Map({
  '&': '&&',
  '|': '||',
  '≥': '>=',
  '≤': '<=',
  '≠': '!==',
  '=': '===',
});

const codegenVisitorDefaults = {
  offsets: Map<string, number>(),
  isMain: false,
  scope: 'curr' as 'curr' | 'globalCurr',
};

// Converts an AST into a string of JavaScript
export class CodegenVisitor extends Record(codegenVisitorDefaults) implements ast.Visitor<string> {
  constructor(offsets: Map<string, number>, isMain: boolean) {
    super({
      offsets,
      isMain,
      scope: isMain ? 'curr' : 'globalCurr',
    });
  }

  ident(n: ast.Ident): string {
    if (n.ident === 'time') {
      return this.refTime();
    } else if (this.offsets.has(n.ident)) {
      return this.refDirect(n.ident);
    } else {
      return this.refIndirect(n.ident);
    }
  }

  table(n: ast.Table): string {
    return `this.tables['${n.ident}']`;
  }

  constant(n: ast.Constant): string {
    return `${n.value}`;
  }

  call(n: ast.CallExpr): string {
    if (!ast.isIdent(n.fun)) {
      throw new Error(`only idents can be used as fns, not ${n.fun}`);
    }

    const fn = n.fun.ident;
    if (!builtins.has(fn)) {
      throw new Error(`unknown builtin: ${fn}`);
    }

    let code = `${fn}(`;
    const builtin = defined(builtins.get(fn));
    if (builtin.usesTime) {
      code += `dt, ${this.refTime()}`;
      if (n.args.size) {
        code += ', ';
      }
    }

    code += n.args.map(arg => arg.walk(this)).join(', ');
    code += ')';

    return code;
  }

  if(n: ast.IfExpr): string {
    const cond = n.cond.walk(this);
    const t = n.t.walk(this);
    const f = n.f.walk(this);

    // use the ternary operator for if statements
    return `(${cond} ? ${t} : ${f})`;
  }

  paren(n: ast.ParenExpr): string {
    const x = n.x.walk(this);
    return `(${x})`;
  }

  unary(n: ast.UnaryExpr): string {
    // if we're doing 'not', explicitly convert the result
    // back to a number.
    const op = n.op === '!' ? '+!' : n.op;
    const x = n.x.walk(this);
    return `${op}${x}`;
  }

  binary(n: ast.BinaryExpr): string {
    // exponentiation isn't a builtin operator in JS, it
    // is implemented as a function in the Math module.
    if (n.op === '^') {
      const l = n.l.walk(this);
      const r = n.r.walk(this);
      return `Math.pow(${l}, ${r})`;
    } else if (n.op === '=' && n.l instanceof ast.Constant && isNaN(n.l.value)) {
      const r = n.r.walk(this);
      return `isNaN(${r})`;
    } else if (n.op === '=' && n.r instanceof ast.Constant && isNaN(n.r.value)) {
      const l = n.l.walk(this);
      return `isNaN(${l})`;
    }

    let op = n.op;
    // only need to convert some of them
    if (JsOps.has(n.op)) {
      op = defined(JsOps.get(n.op));
    }

    const l = n.l.walk(this);
    const r = n.r.walk(this);
    return `${l} ${op} ${r}`;
  }

  // the value of time in the current simulation step
  private refTime(): string {
    return `${this.scope}[0]`;
  }

  // the value of an aux, stock, or flow in the current module
  private refDirect(ident: string): string {
    return `curr[${defined(this.offsets.get(ident))}]`;
  }

  // the value of an overridden module input
  private refIndirect(ident: string): string {
    return `globalCurr[this.ref['${ident}']]`;
  }
}

export function isConst(variable: Variable): boolean {
  return variable.xmile !== undefined && variable.xmile.eqn !== undefined && isFinite(variable.xmile.eqn);
}

export function setAST(variable: Variable, node: ast.Node): Variable {
  // FIXME :\
  const v: any = variable;
  return v.set('ast', node).set('deps', identifierSet(node));
}

function isOrdinary(variable: Variable): variable is Ordinary {
  return variable.kind === 'ordinary';
}

export function isStock(variable: Variable): variable is Stock {
  return variable.kind === 'stock';
}

export function isTable(variable: Variable): variable is Table {
  return variable.kind === 'table';
}

export function isModule(variable: Variable): variable is Module {
  return variable.kind === 'module';
}

function isReference(variable: Variable): variable is Reference {
  return variable.kind === 'reference';
}

function simpleEvalCode(parent: Model, offsets: Map<string, number>, node: ast.Node | undefined): string | undefined {
  if (!node) {
    // return 'NaN';
    throw new Error('simpleEvalCode called with undefined ast.Node');
  }
  const visitor = new CodegenVisitor(offsets, parent.ident === 'main');

  try {
    return defined(node).walk(visitor);
  } catch (e) {
    console.log('// codegen failed!');
    return '';
  }
}

export function code(parent: Model, offsets: Map<string, number>, variable: Variable): string | undefined {
  if (isOrdinary(variable)) {
    if (isConst(variable)) {
      return "this.initials['" + variable.ident + "']";
    }
    return simpleEvalCode(parent, offsets, variable.ast);
  } else if (isStock(variable)) {
    let eqn = 'curr[' + defined(offsets.get(defined(variable.ident))) + '] + (';
    if (variable.inflows.size > 0) {
      // FIXME(bpowers): this shouldn't require converting to a set + back again
      eqn += List(Set(variable.inflows))
        .sort()
        .map(s => {
          if (offsets.has(s)) {
            return `curr[${defined(offsets.get(s))}]`;
          } else {
            return `globalCurr[this.ref['${s}']]`;
          }
        })
        .join('+');
    }
    if (variable.outflows.size > 0) {
      eqn += '- (';
      eqn += List(Set(variable.outflows))
        .sort()
        .map(s => {
          if (offsets.has(s)) {
            return `curr[${defined(offsets.get(s))}]`;
          } else {
            return `globalCurr[this.ref['${s}']]`;
          }
        })
        .join('+');
      eqn += ')';
    }
    // stocks can have no inflows or outflows and still be valid
    if (variable.inflows.size === 0 && variable.outflows.size === 0) {
      eqn += '0';
    }
    eqn += ')*dt';
    return eqn;
  } else if (isTable(variable)) {
    if (!variable.xmile || !variable.xmile.eqn) {
      return undefined;
    }
    const indexExpr = defined(simpleEvalCode(parent, offsets, variable.ast));
    return "lookup(this.tables['" + variable.ident + "'], " + indexExpr + ')';
  } else if (isModule(variable)) {
    throw new Error('code called for Module');
  } else if (isReference(variable)) {
    return `curr["${variable.ptr}"]`;
  } else {
    throw new Error('unreachable');
  }
}

export function ident(variable: Variable): string | undefined {
  if (isOrdinary(variable)) {
    return variable.ident;
  } else if (isStock(variable)) {
    return variable.ident;
  } else if (isTable(variable)) {
    return variable.ident;
  } else if (isModule(variable)) {
    return variable.xmile && variable.xmile.ident;
  } else if (isReference(variable)) {
    return variable.xmile && variable.xmile.ident;
  } else {
    throw new Error('unreachable');
  }
}

export function initialEquation(parent: Model, offsets: Map<string, number>, variable: Variable): string | undefined {
  // returns a string of this variables initial equation. suitable for
  // exec()'ing
  if (isOrdinary(variable)) {
    return code(parent, offsets, variable);
  } else if (isStock(variable)) {
    return simpleEvalCode(parent, offsets, variable.ast);
  } else if (isTable(variable)) {
    return code(parent, offsets, variable);
  } else if (isModule(variable)) {
    return code(parent, offsets, variable);
  } else if (isReference(variable)) {
    return code(parent, offsets, variable);
  } else {
    throw new Error('unreachable');
  }
}

function getModuleDeps(context: Context, variable: Module): Set<string> {
  let allDeps = Set<string>();
  for (let ident of variable.deps) {
    if (ident[0] === '.') {
      if (context.parent === context.mainModel) {
        ident = ident.substr(1);
      } else {
        // we aren't the root model, so we don't care
        continue;
      }
    }

    if (allDeps.has(ident)) {
      continue;
    }

    const v = context.lookup(ident);
    if (!v) {
      throw new Error(`couldn't find ${ident}`);
    }
    // if we hit a Stock the dependencies 'stop'
    if (!(v instanceof Stock)) {
      allDeps = allDeps.add(ident.split('.')[0]);
      allDeps = allDeps.merge(getDeps(context, v));
    }
  }
  return allDeps;
}

export function getDeps(context: Context, variable: Variable): Set<string> {
  if (isModule(variable)) {
    return getModuleDeps(context, variable);
  }

  let allDeps = Set<string>();
  for (let ident of variable.deps) {
    if (ident[0] === '.') {
      if (context.parent === context.mainModel) {
        ident = ident.substr(1);
      } else {
        // we aren't the root model, so we don't care
        continue;
      }
    }
    // we only care about dependencies in our current model's scope
    // (because this is being used for ordering variables)
    ident = ident.split('.')[0];

    if (allDeps.has(ident)) {
      continue;
    }

    // if a user has written an invalid equation, don't blow stack
    if (ident === variable.ident) {
      continue;
    }

    allDeps = allDeps.add(ident);
    const v = context.parent.vars.get(ident);
    if (!v) {
      continue;
    }
    allDeps = allDeps.merge(getDeps(context, v));
  }
  return allDeps;
}

export function referencedModels(project: Project, mod: Module, all?: Map<string, ModelDef>): Map<string, ModelDef> {
  if (!all) {
    all = Map();
  }
  const mdl = defined(project.model(mod.modelName));
  const name = mdl.ident;
  if (all.has(name)) {
    const def = defined(all.get(name)).update('modules', (modules: Set<Module>) => modules.add(mod));
    all = all.set(name, def);
  } else {
    all = all.set(
      name,
      new ModelDef({
        model: mdl,
        modules: Set<Module>([mod]),
      }),
    );
  }
  for (const [name, module] of mdl.modules) {
    all = referencedModels(project, module, all);
  }
  return all;
}

// An AST visitor to deal with desugaring calls to builtin functions
// that are actually module instantiations
export class IdentifierSetVisitor implements ast.Visitor<Set<string>> {
  ident(n: ast.Ident): Set<string> {
    return Set<string>([n.ident]);
  }
  table(n: ast.Table): Set<string> {
    // TODO: I don't think this is necessary, but it can't hurt
    return Set<string>([n.ident]);
  }
  constant(n: ast.Constant): Set<string> {
    return Set<string>();
  }
  call(n: ast.CallExpr): Set<string> {
    let set = Set<string>();
    for (const arg of n.args) {
      set = set.union(arg.walk(this));
    }

    return set;
  }
  if(n: ast.IfExpr): Set<string> {
    const condIdents = n.cond.walk(this);
    const trueIdents = n.t.walk(this);
    const falseIdents = n.f.walk(this);
    return condIdents.union(trueIdents).union(falseIdents);
  }
  paren(n: ast.ParenExpr): Set<string> {
    return n.x.walk(this);
  }
  unary(n: ast.UnaryExpr): Set<string> {
    return n.x.walk(this);
  }
  binary(n: ast.BinaryExpr): Set<string> {
    const leftIdents = n.l.walk(this);
    const rightIdents = n.r.walk(this);
    return leftIdents.union(rightIdents);
  }
}

/**
 * For a given AST node string, returns a set of the identifiers
 * referenced.  Identifiers exclude keywords (such as 'if' and 'then')
 * as well as builtin functions ('pulse', 'max', etc).
 *
 * @param root An AST node.
 * @return A set of all identifiers.
 */
export const identifierSet = (root: ast.Node | undefined): Set<string> => {
  if (!root) {
    return Set<string>();
  }

  return root.walk(new IdentifierSetVisitor());
};
