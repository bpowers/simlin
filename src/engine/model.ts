// Copyright 2019 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { List, Map, Record, Set } from 'immutable';

import * as common from './common';

import { BinaryExpr, CallExpr, Constant, Ident, IfExpr, Node, ParenExpr, UnaryExpr, Visitor } from './ast';
import { BuiltinVisitor, PrintVisitor } from './builtins';
import { canonicalize, defined } from './common';
import {
  isModule,
  isTable,
  Model as varModel,
  Module,
  Ordinary,
  Project,
  setAST,
  Stock,
  Table,
  Variable,
} from './vars';
import {
  GF,
  Model as XmileModel,
  SimSpec,
  UID,
  Variable as XmileVariable,
  View,
  ViewElement,
  ViewElementType,
} from './xmile';

const modelDefaults = {
  valid: false,
  shouldPersist: false,
  xModel: (undefined as any) as XmileModel,
  modules: Map<string, Module>(),
  tables: Map<string, Table>(),
  vars: Map<string, Variable>(),
};

export class Model extends Record(modelDefaults) implements varModel {
  constructor(project: Project, xModel: XmileModel, shouldPersist = false) {
    const [vars, modules, tables] = parseVars(project, xModel.variables);

    super({
      xModel,
      valid: true,
      shouldPersist,
      vars,
      modules,
      tables,
    });
  }

  parse(project: Project): [Map<string, Variable>, Map<string, Module>, Map<string, Table>] {
    return parseVars(project, this.xModel.variables);
  }

  // this happens before populateNamedElements, so we can't reference namedElements
  // in here.
  getFlowEnds(): Map<string, [UID | undefined, UID | undefined]> {
    const stocks = this.xModel.getStocks();
    return this.xModel.getFlowEnds(stocks);
  }

  addStocksFlow(project: Project, stock: string, flow: string, dir: 'in' | 'out'): Model {
    const updatePath = ['xModel'];
    const model = this.updateIn(updatePath, (xModel: XmileModel) => {
      const variables = xModel.variables.map((v: XmileVariable) => {
        if (v.type === 'stock' && v.ident === stock) {
          if (dir === 'in') {
            v = v.set('inflows', (v.inflows || List()).push(flow));
          } else {
            v = v.set('outflows', (v.outflows || List()).push(flow));
          }
        }
        return v;
      });
      return xModel.merge({ variables });
    });

    // TODO: this should be made incremental
    const [vars, modules, tables] = parseVars(project, model.xModel.variables);

    return model.merge({
      vars,
      modules,
      tables,
    });
  }

  removeStocksFlow(project: Project, stock: string, flow: string, dir: 'in' | 'out'): Model {
    const updatePath = ['xModel'];
    const model = this.updateIn(updatePath, (xModel: XmileModel) => {
      const variables = xModel.variables.map((v: XmileVariable) => {
        if (v.type === 'stock' && v.ident === stock) {
          if (dir === 'in') {
            v = v.set(
              'inflows',
              (v.inflows || List()).filter((id) => id !== flow),
            );
          } else {
            v = v.set(
              'outflows',
              (v.outflows || List()).filter((id) => id !== flow),
            );
          }
        }
        return v;
      });
      return xModel.merge({ variables });
    });

    // TODO: this should be made incremental
    const [vars, modules, tables] = parseVars(project, model.xModel.variables);

    return model.merge({
      vars,
      modules,
      tables,
    });
  }

  addNewVariable(project: Project, type: ViewElementType, name: string): Model {
    if (!(type === 'flow' || type === 'stock' || type === 'aux')) {
      throw new Error(`unsupported new type (${type}) for ${name}`);
    }
    const updatePath = ['xModel'];
    const model = this.updateIn(updatePath, (xModel: XmileModel) => {
      const newVariable = new XmileVariable({
        type,
        name,
        eqn: '',
      });
      const variables = xModel.variables.push(newVariable);
      return xModel.merge({ variables });
    });

    // TODO: this should be made incremental
    const [vars, modules, tables] = parseVars(project, model.xModel.variables);

    return model.merge({
      vars,
      modules,
      tables,
    });
  }

  deleteVariables(project: Project, names: readonly string[]): Model {
    const toDelete = Set(names);
    const updatePath = ['xModel'];
    const model = this.updateIn(updatePath, (xModel: XmileModel) => {
      const variables = xModel.variables.filter((v: XmileVariable) => v.ident && !toDelete.contains(v.ident));
      return xModel.merge({ variables });
    });

    // TODO: this should be made incremental
    const [vars, modules, tables] = parseVars(project, model.xModel.variables);

    return model.merge({
      vars,
      modules,
      tables,
    });
  }

  setEquation(project: Project, ident: string, newEquation: string): Model {
    const updatePath = ['xModel'];
    const model = this.updateIn(updatePath, (xModel: XmileModel) => {
      const variables = xModel.variables.map((v: XmileVariable) => {
        // TODO: check deps; if this model depends on the old one, update the equation
        if (v.name && v.ident === ident) {
          v = v.set('eqn', newEquation);
        }
        return v;
      });
      return xModel.merge({ variables });
    });

    // TODO: this should be made incremental
    const [vars, modules, tables] = parseVars(project, model.xModel.variables);

    return model.merge({
      vars,
      modules,
      tables,
    });
  }

  setTable(project: Project, ident: string, newTable: { x: List<number>; y: List<number> } | null): Model {
    const updatePath = ['xModel'];
    const model = this.updateIn(updatePath, (xModel: XmileModel) => {
      const variables = xModel.variables.map((v: XmileVariable) => {
        if (v.name && v.ident === ident) {
          if (newTable === null) {
            v = v.set('gf', undefined);
          } else {
            const gf = (v.gf || new GF()).set('xPoints', newTable.x).set('yPoints', newTable.y);
            v = v.set('gf', gf);
          }
        }
        return v;
      });
      return xModel.merge({ variables });
    });

    // TODO: this should be made incremental
    const [vars, modules, tables] = parseVars(project, model.xModel.variables);

    return model.merge({
      vars,
      modules,
      tables,
    });
  }

  rename(project: Project, oldName: string, newName: string): Model {
    const oldIdent = canonicalize(oldName);
    const newIdent = canonicalize(newName);

    // first update connector `from` and `to` references, and the names of ViewElements
    let updatePath = ['xModel', 'views', 0];
    let model = this.updateIn(updatePath, (view: View) => {
      const elements = view.elements.map((element: ViewElement) => {
        // update the name of the ViewElement being renamed
        if (element.hasName && element.ident === oldIdent) {
          return element.set('name', newName);
        }

        // otherwise, the only other things to update are Connectors
        if (element.type !== 'connector') {
          return element;
        }

        if (element.from === oldIdent) {
          element = element.set('from', newIdent);
        }
        if (element.to === oldIdent) {
          element = element.set('to', newIdent);
        }

        return element;
      });
      return view.merge({ elements });
    });

    // next update the variable name + fixup any equations that referenced the old name.
    updatePath = ['xModel'];
    model = model.updateIn(updatePath, (xModel: XmileModel) => {
      const variables = xModel.variables.map((v: XmileVariable) => {
        // TODO: check deps; if this model depends on the old one, update the equation
        if (v.name && v.ident === oldIdent) {
          v = v.set('name', newIdent);
        }
        if (v.inflows) {
          v = v.set(
            'inflows',
            v.inflows.map((id) => (id === oldIdent ? newIdent : id)),
          );
        }
        if (v.outflows) {
          v = v.set(
            'outflows',
            v.outflows.map((id) => (id === oldIdent ? newIdent : id)),
          );
        }
        return handleRenameInAST(v, oldIdent, newIdent);
      });
      return xModel.merge({ variables });
    });

    // now that we've updated the XMILE data model, regenerate the variables +
    // friends.
    // TODO: this should be made incremental
    const [vars, modules, tables] = parseVars(project, model.xModel.variables);

    return model.merge({
      vars,
      modules,
      tables,
    });
  }

  view(index: number): View | undefined {
    return this.xModel.views.get(index);
  }

  toXmile(): XmileModel {
    return this.xModel;
  }

  get ident(): string {
    const name = !this.xModel.ident ? 'main' : this.xModel.ident;
    return common.canonicalize(name);
  }

  get simSpec(): SimSpec | undefined {
    return this.xModel.simSpec;
  }
}

export function isModel(model: any): model is Model {
  return model.constructor === Model;
}

// An AST visitor to deal with desugaring calls to builtin functions
// that are actually module instantiations
export class RenameVisitor implements Visitor<Node> {
  oldIdent: string;
  newIdent: string;
  n = 0;

  constructor(oldIdent: string, newIdent: string) {
    this.oldIdent = oldIdent;
    this.newIdent = newIdent;
  }

  get didRewrite(): boolean {
    return this.n > 0;
  }

  ident(n: Ident): Node {
    if (n.ident === this.oldIdent) {
      this.n++;
      return n.set('ident', this.newIdent);
    }
    return n;
  }
  table(n: Ident): Node {
    if (n.ident === this.oldIdent) {
      this.n++;
      return n.set('ident', this.newIdent);
    }
    return n;
  }
  constant(n: Constant): Node {
    return n;
  }
  call(n: CallExpr): Node {
    const args = n.args.map((arg) => arg.walk(this));
    const fun = n.fun.walk(this);

    return n.merge({
      args,
      fun,
    });
  }
  if(n: IfExpr): Node {
    const cond = n.cond.walk(this);
    const t = n.t.walk(this);
    const f = n.f.walk(this);
    return new IfExpr(n.ifPos, cond, n.thenPos, t, n.elsePos, f);
  }
  paren(n: ParenExpr): Node {
    const x = n.x.walk(this);
    return new ParenExpr(n.lPos, x, n.rPos);
  }
  unary(n: UnaryExpr): Node {
    const x = n.x.walk(this);
    return new UnaryExpr(n.opPos, n.op, x);
  }
  binary(n: BinaryExpr): Node {
    const l = n.l.walk(this);
    const r = n.r.walk(this);
    return new BinaryExpr(l, n.opPos, n.op, r);
  }
}

function handleRenameInAST(v: XmileVariable, oldIdent: string, newIdent: string): XmileVariable {
  const modelVar = instantiate(v);

  const updater = new RenameVisitor(oldIdent, newIdent);

  // check for builtins that require module instantiations
  if (!modelVar.ast) {
    return v;
  }

  const ast = modelVar.ast.walk(updater);
  if (updater.didRewrite) {
    const printer = new PrintVisitor();
    const eqn = ast.walk(printer);
    v = v.set('eqn', eqn);
  }

  // maybe update a stock's inflows and outflows
  if (v.inflows) {
    v = v.set(
      'inflows',
      v.inflows.map((flow) => {
        return flow === oldIdent ? newIdent : flow;
      }),
    );
  }
  if (v.outflows) {
    v = v.set(
      'outflows',
      v.outflows.map((flow) => {
        return flow === oldIdent ? newIdent : flow;
      }),
    );
  }

  return v;
}

function instantiate(v: XmileVariable): Variable {
  switch (v.type) {
    case 'module':
      return new Module(v);
    case 'stock':
      return new Stock(v);
    case 'aux':
      // FIXME: fix Variable/GF/Table nonsense
      let aux: Variable | undefined;
      if (v.gf) {
        const table = new Table(v);
        if (table.valid) {
          aux = table;
        }
      }
      if (!aux) {
        aux = new Ordinary(v);
      }
      return aux;
    case 'flow':
      let flow: Variable | undefined;
      if (v.gf) {
        const table = new Table(v);
        if (table.valid) {
          flow = table;
        }
      }
      if (!flow) {
        flow = new Ordinary(v);
      }
      return flow;
  }
  throw new Error('unreachable: unknown type "' + v.type + '"');
}

/**
 * Validates & figures out all necessary variable information.
 */
function parseVars(
  project: Project,
  variables: List<XmileVariable>,
): [Map<string, Variable>, Map<string, Module>, Map<string, Table>] {
  let vars = Map<string, Variable>();
  let modules = Map<string, Module>();
  let tables = Map<string, Table>();
  for (const v of variables) {
    // IMPORTANT: we need to use the canonicalized
    // identifier, not the 'xmile name', which is
    // what I like to think of as the display name.
    const ident = v.ident;
    if (ident === undefined) {
      throw new Error('variable without a name');
    }

    // FIXME: is this too simplistic?
    if (vars.has(ident)) {
      throw new Error(`Variable '${ident}' already exists.`);
    }

    const modelVar = instantiate(v);
    vars = vars.set(ident, modelVar);
    if (isTable(modelVar)) {
      tables = tables.set(ident, modelVar);
    }
    if (isModule(modelVar)) {
      modules = modules.set(ident, modelVar);
    }
  }

  let vars2;
  let modules2;
  try {
    [vars2, modules2] = instantiateImplicitModules(project, vars);
  } catch (err) {
    throw new Error(`instantiateImplicitModules: ${err.message}`);
  }

  return [defined(vars2), defined(modules2).merge(modules), tables];
}

function instantiateImplicitModules(
  project: Project,
  vars: Map<string, Variable>,
): [Map<string, Variable>, Map<string, Module>] {
  let modules = Map<string, Module>();
  let additionalVars = Map<string, Variable>();
  vars = vars.map(
    (v: Variable): Variable => {
      const visitor = new BuiltinVisitor(project, v);

      // check for builtins that require module instantiations
      if (!v.ast) {
        return v;
      }

      const ast = v.ast.walk(visitor);
      if (visitor.didRewrite) {
        v = setAST(v, ast);
      }

      for (const [name, v] of visitor.vars) {
        if (vars.has(name)) {
          throw new Error('builtin walk error, duplicate ' + name);
        }
        additionalVars = additionalVars.set(name, v);
      }
      for (const [name, mod] of visitor.modules) {
        if (modules.has(name)) {
          throw new Error('builtin walk error, duplicate ' + name);
        }
        modules = modules.set(name, mod);
      }
      return v;
    },
  );
  vars = vars.merge(additionalVars);

  return [vars, modules];
}
