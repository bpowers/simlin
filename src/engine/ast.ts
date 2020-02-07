// Copyright 2019 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { List, Record } from 'immutable';

import { canonicalize } from './common';
import { SourceLoc, UnknownSourceLoc } from './token';

// this is a gross hack to work around the mismatch of TypeScript
// and immutable JS.
const defaultNode: Node = (null as any) as Node;

export interface Node {
  readonly pos: SourceLoc;
  readonly end: SourceLoc; // the char after this token

  walk<T>(v: Visitor<T>): T;
  equals(other: any): boolean; // provided by Record

  toJS(): any;
}

export interface Visitor<T> {
  ident(n: Ident): T;
  constant(n: Constant): T;
  call(n: CallExpr): T;
  if(n: IfExpr): T;
  paren(n: ParenExpr): T;
  unary(n: UnaryExpr): T;
  binary(n: BinaryExpr): T;
  table(n: Table): T;
}

const identDefaults = {
  ident: '' as string,
  pos: UnknownSourceLoc,
  len: 0,
};

export class Ident extends Record(identDefaults) implements Node {
  constructor(pos: SourceLoc, name: string) {
    // this.name is canonicalized, so we need to store the
    // original length.
    super({
      ident: canonicalize(name),
      len: name.length,
      pos,
    });
  }

  get end(): SourceLoc {
    return this.pos.off(this.len);
  }

  walk<T>(v: Visitor<T>): T {
    return v.ident(this);
  }
}

export function isIdent(n: Node): n is Ident {
  return n.constructor === Ident;
}

export class Table extends Record(identDefaults) implements Node {
  constructor(pos: SourceLoc, name: string) {
    // this.name is canonicalized, so we need to store the
    // original length.
    super({
      ident: canonicalize(name),
      len: name.length,
      pos,
    });
  }

  get end(): SourceLoc {
    return this.pos.off(this.len);
  }

  walk<T>(v: Visitor<T>): T {
    return v.table(this);
  }
}

export function TableFrom(n: Node): Node {
  if (!isIdent(n)) {
    throw new Error(`expected first arg of lookup to be Ident, not ${n.toJS()}`);
  }

  return new Table(n.pos, n.ident);
}

export function isTable(n: Node): n is Ident {
  return n.constructor === Table;
}

const constantDefaults = {
  value: NaN,
  len: -1,
  pos: UnknownSourceLoc,
};

export class Constant extends Record(constantDefaults) implements Node {
  constructor(pos: SourceLoc, value: string) {
    super({
      value: parseFloat(value),
      len: value.length,
      pos,
    });
  }

  get end(): SourceLoc {
    return this.pos.off(this.len);
  }

  walk<T>(v: Visitor<T>): T {
    return v.constant(this);
  }
}

const parenDefaults = {
  x: defaultNode,
  lPos: UnknownSourceLoc,
  rPos: UnknownSourceLoc,
};

export class ParenExpr extends Record(parenDefaults) implements Node {
  constructor(lPos: SourceLoc, x: Node, rPos: SourceLoc) {
    super({ lPos, rPos, x });
  }

  get pos(): SourceLoc {
    return this.lPos;
  }
  get end(): SourceLoc {
    return this.rPos.off(1);
  }

  walk<T>(v: Visitor<T>): T {
    return v.paren(this);
  }
}

const callDefaults = {
  fun: defaultNode,
  args: List<Node>(),
  lParenPos: UnknownSourceLoc,
  rParenPos: UnknownSourceLoc,
};

export class CallExpr extends Record(callDefaults) implements Node {
  constructor(fun: Node, lParenPos: SourceLoc, args: List<Node>, rParenPos: SourceLoc) {
    super({ fun, args, lParenPos, rParenPos });
  }

  get pos(): SourceLoc {
    return this.fun.pos;
  }
  get end(): SourceLoc {
    return this.rParenPos.off(1);
  }

  walk<T>(v: Visitor<T>): T {
    return v.call(this);
  }
}

const unaryDefaults = {
  op: '',
  x: defaultNode,
  opPos: UnknownSourceLoc,
};

export class UnaryExpr extends Record(unaryDefaults) implements Node {
  constructor(opPos: SourceLoc, op: string, x: Node) {
    super({ op, x, opPos });
  }

  get pos(): SourceLoc {
    return this.opPos;
  }
  get end(): SourceLoc {
    return this.x.end;
  }

  walk<T>(v: Visitor<T>): T {
    return v.unary(this);
  }
}

const binaryDefaults = {
  op: '',
  l: defaultNode,
  r: defaultNode,
  opPos: UnknownSourceLoc,
};

export class BinaryExpr extends Record(binaryDefaults) implements Node {
  constructor(l: Node, opPos: SourceLoc, op: string, r: Node) {
    super({ l, op, r, opPos });
  }

  get pos(): SourceLoc {
    return this.l.pos;
  }
  get end(): SourceLoc {
    return this.r.end;
  }

  walk<T>(v: Visitor<T>): T {
    return v.binary(this);
  }
}

const ifDefaults = {
  cond: defaultNode,
  t: defaultNode,
  f: defaultNode,
  ifPos: UnknownSourceLoc,
  thenPos: UnknownSourceLoc,
  elsePos: UnknownSourceLoc,
};

export class IfExpr extends Record(ifDefaults) implements Node {
  constructor(ifPos: SourceLoc, cond: Node, thenPos: SourceLoc, t: Node, elsePos: SourceLoc, f: Node) {
    super({ cond, t, f, ifPos, thenPos, elsePos });
  }

  get pos(): SourceLoc {
    return this.ifPos;
  }
  get end(): SourceLoc {
    return this.f.end;
  }

  walk<T>(v: Visitor<T>): T {
    return v.if(this);
  }
}
