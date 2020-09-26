// Copyright 2019 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { List, Map } from 'immutable';

import {
  BinaryExpr,
  CallExpr,
  Constant,
  Ident,
  IfExpr,
  isIdent,
  Node,
  ParenExpr,
  Table,
  TableFrom,
  UnaryExpr,
  Visitor,
} from './ast';
import { builtins, defined } from './common';
import { ident, Module, Ordinary, Project, Variable } from './vars';
import { Connection, Variable as XmileVariable } from './xmile';

const stdlibArgs = Map<string, List<string>>([
  ['smth1', List(['input', 'delay_time', 'initial_value'])],
  ['smth3', List(['input', 'delay_time', 'initial_value'])],
  ['delay1', List(['input', 'delay_time', 'initial_value'])],
  ['delay3', List(['input', 'delay_time', 'initial_value'])],
  ['trend', List(['input', 'delay_time', 'initial_value'])],
]);

// An AST visitor to deal with desugaring calls to builtin functions
// that are actually module instantiations
export class BuiltinVisitor implements Visitor<Node> {
  readonly project: Project;
  readonly variable: Variable;
  modules: Map<string, Module> = Map();
  vars: Map<string, Variable> = Map();
  n = 0;

  constructor(project: Project, v: Variable) {
    this.project = project;
    this.variable = v;
  }

  get didRewrite(): boolean {
    return this.n > 0;
  }

  ident(n: Ident): Node {
    return n;
  }
  table(n: Ident): Node {
    return n;
  }
  constant(n: Constant): Node {
    return n;
  }
  call(n: CallExpr): Node {
    let args = n.args.map((arg) => arg.walk(this));

    if (!isIdent(n.fun)) {
      throw new Error('// for now, only idents can be used as fns.');
    }

    const fn = n.fun.ident;
    if (builtins.has(fn)) {
      if (fn === 'lookup') {
        args = args.update(0, defined(args.get(0)), (arg) => TableFrom(arg));
        this.n++;
      }
      return new CallExpr(n.fun, n.lParenPos, args, n.rParenPos);
    }

    const model = this.project.model('stdlib·' + fn);
    if (!model) {
      console.warn('unknown builtin: ' + fn);
      // throw new Error('unknown builtin: ' + fn);
      return new Constant(n.pos, '0.0');
    }

    let identArgs = List<string>();
    args.forEach((arg, i) => {
      if (isIdent(arg)) {
        identArgs = identArgs.push(arg.ident);
      } else {
        const id = defined(ident(this.variable));
        const xVar = new XmileVariable({
          type: 'aux',
          name: `$·${id}·${this.n}·arg${i}`,
          eqn: arg.walk(new PrintVisitor()),
        });
        const proxyVar = new Ordinary(xVar);
        this.vars = this.vars.set(defined(proxyVar.ident), proxyVar);
        identArgs.push(defined(proxyVar.ident));
      }
    });

    const modId = defined(ident(this.variable));
    const modName = `$·${modId}·${this.n}·${fn}`;
    let xMod = new XmileVariable({
      type: 'module',
      name: modName,
      model: `stdlib·${fn}`,
      connections: List<Connection>(),
    });

    if (!stdlibArgs.has(fn)) {
      throw new Error(`unknown function or builtin ${fn}`);
    }

    const stdlibVars = defined(stdlibArgs.get(fn));

    identArgs.forEach((identArg, i) => {
      const conn = new Connection({
        to: defined(stdlibVars.get(i)),
        from: '.' + identArg,
      });
      xMod = xMod.update('connections', (conns) => (conns || List()).push(conn));
    });

    const module = new Module(xMod);
    this.modules = this.modules.set(modName, module);
    this.vars = this.vars.set(modName, module);

    this.n++;

    return new Ident(n.fun.pos, modName + '.output');
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

// An AST visitor to deal with desugaring calls to builtin functions
// that are actually module instantiations
export class PrintVisitor implements Visitor<string> {
  ident(n: Ident): string {
    return n.ident;
  }
  table(n: Table): string {
    return n.ident;
  }
  constant(n: Constant): string {
    return `${n.value}`;
  }
  call(n: CallExpr): string {
    const fun = n.fun.walk(this);
    const args = n.args.map((arg) => arg.walk(this)).join(',');
    return `${fun}(${args})`;
  }
  if(n: IfExpr): string {
    const cond = n.cond.walk(this);
    const t = n.t.walk(this);
    const f = n.f.walk(this);
    return `IF (${cond}) THEN (${t}) ELSE (${f})`;
  }
  paren(n: ParenExpr): string {
    const x = n.x.walk(this);
    return `(${x})`;
  }
  unary(n: UnaryExpr): string {
    const x = n.x.walk(this);
    return `${n.op}${x}`;
  }
  binary(n: BinaryExpr): string {
    const l = n.l.walk(this);
    const r = n.r.walk(this);
    return `${l}${n.op}${r}`;
  }
}
