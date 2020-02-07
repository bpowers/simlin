// Copyright 2019 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { List } from 'immutable';

import { BinaryExpr, CallExpr, Constant, Ident, IfExpr, Node, ParenExpr, UnaryExpr } from './ast';
import { Lexer } from './lex';
import { SourceLoc, Token, TokenType } from './token';

// eslint-disable-next-line @typescript-eslint/no-unused-vars
const _WORD_OPS = {
  not: '!',
  and: '&',
  or: '|',
  mod: '%',
};

const UNARY = '+-!';

const BINARY = [
  '^',
  '!', // FIXME(bp) right-associativity
  '*/%',
  '+-',
  '><≥≤',
  '=≠',
  '&',
  '|',
];

export function eqn(eqn: string): [Node | null, string[] | null] {
  const p = new Parser(eqn);
  const ast = p.expr();
  if (p.errs && p.errs.length) {
    return [null, p.errs];
  }
  return [ast, null];
}

function binaryLevel(n: number, p: Parser, ops: string): (maxLevel: number) => Node | null {
  return (maxLevel: number): Node | null => {
    const t = p.lexer.peek();
    // Ensure that we don't inadvertently mess up operator
    // precedence when recursively calling back into
    // binaryLevel.
    if (n >= maxLevel) {
      return p.factor();
    }

    if (!t) {
      return null;
    }

    const next = p.levels[n + 1];
    let lhs = next(maxLevel);
    if (!lhs) {
      return null;
    }
    // its ok if we didn't have a binary operator
    for (let op = p.consumeAnyOf(ops); op; op = p.consumeAnyOf(ops)) {
      // must call the next precedence level to
      // preserve left-associativity

      // find a right hand term, make sure that term isn't a compound
      // term containing an operator of higher precedence then ours
      const rhs = p.levels[0](n);
      if (!rhs) {
        p.errs.push('expected rhs of expr after "' + op.tok + '"');
        return null;
      }

      lhs = new BinaryExpr(lhs, op.startLoc, op.tok, rhs);
    }
    return lhs;
  };
}

class Parser {
  lexer: Lexer;
  errs: string[] = [];
  levels: ((maxLevel: number) => Node | null)[] = [];

  constructor(eqn: string) {
    this.lexer = new Lexer(eqn);
    for (let i = 0; i < BINARY.length; i++) {
      this.levels.push(binaryLevel(i, this, BINARY[i]));
    }

    // after all of the binary operator precedence levels,
    // look for lower precedence factors (if,call,etc)
    this.levels.push((maxLevel: number): Node | null => {
      return this.factor();
    });
  }
  get errors(): string[] {
    return this.errs;
  }

  expr(): Node | null {
    return this.levels[0](BINARY.length);
  }
  factor(): Node | null {
    let lhs: Node | null;
    let pos: SourceLoc | null;
    if ((pos = this.consumeTok('('))) {
      lhs = this.expr();
      if (!lhs) {
        this.errs.push('expected an expression after an opening paren');
        return null;
      }
      let closing: SourceLoc | null;
      if (!(closing = this.consumeTok(')'))) {
        this.errs.push('expected ")", not end-of-equation');
        return null;
      }
      return new ParenExpr(pos, lhs, closing);
    }

    let op: Token | null;
    if ((op = this.consumeAnyOf(UNARY))) {
      lhs = this.expr();
      if (!lhs) {
        this.errs.push('unary operator "' + op.tok + '" without operand.');
        return null;
      }
      return new UnaryExpr(op.startLoc, op.tok, lhs);
    }

    if ((lhs = this.num())) {
      return lhs;
    }

    let ifLoc: SourceLoc | null;
    if ((ifLoc = this.consumeReserved('if'))) {
      const cond = this.expr();
      if (!cond) {
        this.errs.push('expected an expr to follow "IF"');
        return null;
      }
      let thenLoc: SourceLoc | null;
      if (!(thenLoc = this.consumeReserved('then'))) {
        this.errs.push('expected "THEN"');
        return null;
      }
      const t = this.expr();
      if (!t) {
        this.errs.push('expected an expr to follow "THEN"');
        return null;
      }
      let elseLoc: SourceLoc | null;
      if (!(elseLoc = this.consumeReserved('else'))) {
        this.errs.push('expected "ELSE"');
        return null;
      }
      const f = this.expr();
      if (!f) {
        this.errs.push('expected an expr to follow "ELSE"');
        return null;
      }
      return new IfExpr(ifLoc, cond, thenLoc, t, elseLoc, f);
    }

    if ((lhs = this.ident())) {
      // check if this is a function call
      let lParenLoc: SourceLoc | null;
      if ((lParenLoc = this.consumeTok('('))) {
        return this.call(lhs, lParenLoc);
      } else if ((lhs as Ident).ident === 'nan') {
        return new Constant(lhs.pos, (lhs as Ident).ident);
      } else {
        return lhs;
      }
    }

    // an empty expression isn't necessarily an error
    return null;
  }

  consumeAnyOf(ops: string): Token | null {
    const peek = this.lexer.peek();
    if (!peek || peek.type !== TokenType.TOKEN) {
      return null;
    }
    for (const tok of ops) {
      if (peek.tok === tok) {
        return this.lexer.nextTok();
      }
    }
    return null;
  }

  consumeTok(s: string): SourceLoc | null {
    const t = this.lexer.peek();
    if (!t || t.type !== TokenType.TOKEN || t.tok !== s) {
      return null;
    }
    // consume match
    this.lexer.nextTok();
    return t.startLoc;
  }

  consumeReserved(s: string): SourceLoc | null {
    const t = this.lexer.peek();
    if (!t || t.type !== TokenType.RESERVED || t.tok !== s) {
      return null;
    }
    // consume match
    this.lexer.nextTok();
    return t.startLoc;
  }

  num(): Node | null {
    const t = this.lexer.peek();
    if (!t || t.type !== TokenType.NUMBER) {
      return null;
    }
    // consume number
    this.lexer.nextTok();
    return new Constant(t.startLoc, t.tok);
  }

  ident(): Node | null {
    const t = this.lexer.peek();
    if (!t || t.type !== TokenType.IDENT) {
      return null;
    }
    // consume ident
    this.lexer.nextTok();
    return new Ident(t.startLoc, t.tok);
  }

  call(fn: Node, lParenLoc: SourceLoc): Node | null {
    let args = List<Node>();

    // no-arg call - simplifies logic to special case this.
    let rParenLoc: SourceLoc | null;
    if ((rParenLoc = this.consumeTok(')'))) {
      return new CallExpr(fn, lParenLoc, args, rParenLoc);
    }

    while (true) {
      const arg = this.expr();
      if (!arg) {
        this.errs.push('expected expression as arg in function call');
        return null;
      }
      args = args.push(arg);
      if (this.consumeTok(',')) {
        continue;
      }
      if ((rParenLoc = this.consumeTok(')'))) {
        break;
      }
      this.errs.push('call: expected "," or ")"');
      return null;
    }

    return new CallExpr(fn, lParenLoc, args, rParenLoc);
  }
}
