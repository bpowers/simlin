// Copyright 2019 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { List } from 'immutable';

import { expect } from '@jest/globals';
import * as parse from '../src/engine/parse';
import { SourceLoc } from '../src/engine/token';
import { Node, BinaryExpr, ParenExpr, IfExpr, CallExpr, Ident, Constant } from '../src/engine/ast';

interface ParseTestData {
  in: string;
  out: Node;
}

function l(line: number, pos: number): SourceLoc {
  'use strict';
  return new SourceLoc(line, pos);
}

const PARSE_TESTS: ParseTestData[] = [
  {
    in: 'a',
    out: new Ident(l(0, 0), 'a'),
  },
  {
    in: '3.2 <> åbc',
    out: new BinaryExpr(new Constant(l(0, 0), '3.2'), l(0, 4), '≠', new Ident(l(0, 7), 'åbc')),
  },
  {
    in: 'hares * birth_fraction',
    out: new BinaryExpr(new Ident(l(0, 0), 'hares'), l(0, 6), '*', new Ident(l(0, 8), 'birth_fraction')),
  },
  {
    in: '(5. * åbc)',
    out: new ParenExpr(
      l(0, 0),
      new BinaryExpr(new Constant(l(0, 1), '5.'), l(0, 4), '*', new Ident(l(0, 6), 'åbc')),
      l(0, 9),
    ),
  },
  {
    in: '(5. * åbc4)',
    out: new ParenExpr(
      l(0, 0),
      new BinaryExpr(new Constant(l(0, 1), '5.'), l(0, 4), '*', new Ident(l(0, 6), 'åbc4')),
      l(0, 10),
    ),
  },
  {
    in: 'smooth()',
    out: new CallExpr(new Ident(l(0, 0), 'smooth'), l(0, 6), List(), l(0, 7)),
  },
  {
    in: 'smooth(1, 2 + 3, d)',
    out: new CallExpr(
      new Ident(l(0, 0), 'smooth'),
      l(0, 6),
      List([
        new Constant(l(0, 7), '1'),
        new BinaryExpr(new Constant(l(0, 10), '2'), l(0, 12), '+', new Constant(l(0, 14), '3')),
        new Ident(l(0, 17), 'd'),
      ]),
      l(0, 18),
    ),
  },
  {
    in: 'IF a THEN b ELSE c',
    out: new IfExpr(
      l(0, 0),
      new Ident(l(0, 3), 'a'),
      l(0, 5),
      new Ident(l(0, 10), 'b'),
      l(0, 12),
      new Ident(l(0, 17), 'c'),
    ),
  },
  {
    in: 'a > 1',
    out: new BinaryExpr(new Ident(l(0, 0), 'a'), l(0, 2), '>', new Constant(l(0, 4), '1')),
  },
  {
    in: 'a = 1',
    out: new BinaryExpr(new Ident(l(0, 0), 'a'), l(0, 2), '=', new Constant(l(0, 4), '1')),
  },
  {
    in: 'IF a > 0 THEN b ELSE c',
    out: new IfExpr(
      l(0, 0),
      new BinaryExpr(new Ident(l(0, 3), 'a'), l(0, 5), '>', new Constant(l(0, 7), '0')),
      l(0, 9),
      new Ident(l(0, 14), 'b'),
      l(0, 16),
      new Ident(l(0, 21), 'c'),
    ),
  },
  {
    in: 'IF 0 > a THEN b ELSE c',
    out: new IfExpr(
      l(0, 0),
      new BinaryExpr(new Constant(l(0, 3), '0'), l(0, 5), '>', new Ident(l(0, 7), 'a')),
      l(0, 9),
      new Ident(l(0, 14), 'b'),
      l(0, 16),
      new Ident(l(0, 21), 'c'),
    ),
  },
  {
    in: 'IF 1 >= a THEN b ELSE c',
    out: new IfExpr(
      l(0, 0),
      new BinaryExpr(new Constant(l(0, 3), '1'), l(0, 5), '≥', new Ident(l(0, 8), 'a')),
      l(0, 10),
      new Ident(l(0, 15), 'b'),
      l(0, 17),
      new Ident(l(0, 22), 'c'),
    ),
  },
  {
    in: '4 - 5 + 6',
    out: new BinaryExpr(
      new BinaryExpr(new Constant(l(0, 0), '4'), l(0, 2), '-', new Constant(l(0, 4), '5')),
      l(0, 6),
      '+',
      new Constant(l(0, 8), '6'),
    ),
  },
  {
    in: '6 + 0 * 8',
    out: new BinaryExpr(
      new Constant(l(0, 0), '6'),
      l(0, 2),
      '+',
      new BinaryExpr(new Constant(l(0, 4), '0'), l(0, 6), '*', new Constant(l(0, 8), '8')),
    ),
  },
];

const PARSE_TEST_FAILURES = [
  '(',
  '(3',
  '3 +',
  '3 *',
  '(3 +)',
  'call(a,',
  'call(a,1+',
  'if if',
  'if 1 then',
  'if then',
  'if 1 then 2 else',
];

describe('parse', function (): void {
  PARSE_TESTS.forEach(function (t: ParseTestData): void {
    it('should parse ' + t.in, function (done): void {
      let [node, err] = parse.eqn(t.in);
      if (err) {
        for (let i = 0; i < err.length; i++) console.log(err[i]);
      }
      expect(node).not.toBeNull();
      expect(err).toBeNull();
      expect(t.out.equals(node)).toBeTruthy();
      done();
    });
  });
});

describe('parse-failures', function (): void {
  PARSE_TEST_FAILURES.forEach(function (eqn: string): void {
    it("shouldn't parse " + eqn, function (done): void {
      let [node, err] = parse.eqn(eqn);
      expect(node).toBeNull();
      expect(err).not.toBeNull();
      done();
    });
  });
});
