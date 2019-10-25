// Copyright 2015 Bobby Powers. All rights reserved.
// Use of this source code is governed by the MIT
// license that can be found in the LICENSE file.

'use strict';

import * as chai from 'chai';
import { SourceLoc, Token, TokenType } from '../lib/token';
import { Lexer } from '../lib/lex';

const expect = chai.expect;

interface LexTestData {
  in: string;
  out: Token[];
}

const loc = new SourceLoc(0, 0);

const LEX_TESTS: LexTestData[] = [
  {
    in: 'a',
    out: [new Token('a', TokenType.IDENT, loc, loc)],
  },
  {
    in: 'å',
    out: [new Token('å', TokenType.IDENT, loc, loc)],
  },
  {
    in: 'a1_åbc________',
    out: [new Token('a1_åbc________', TokenType.IDENT, loc, loc)],
  },
  {
    in: 'IF value THEN MAX(flow, 1) ELSE flow',
    out: [
      new Token('if', TokenType.RESERVED, loc, loc),
      new Token('value', TokenType.IDENT, loc, loc),
      new Token('then', TokenType.RESERVED, loc, loc),
      new Token('max', TokenType.IDENT, loc, loc),
      new Token('(', TokenType.TOKEN, loc, loc),
      new Token('flow', TokenType.IDENT, loc, loc),
      new Token(',', TokenType.TOKEN, loc, loc),
      new Token('1', TokenType.NUMBER, loc, loc),
      new Token(')', TokenType.TOKEN, loc, loc),
      new Token('else', TokenType.RESERVED, loc, loc),
      new Token('flow', TokenType.IDENT, loc, loc),
    ],
  },
  {
    in: 'if a < 1 then 1 else 0',
    out: [
      new Token('if', TokenType.RESERVED, loc, loc),
      new Token('a', TokenType.IDENT, loc, loc),
      new Token('<', TokenType.TOKEN, loc, loc),
      new Token('1', TokenType.NUMBER, loc, loc),
      new Token('then', TokenType.RESERVED, loc, loc),
      new Token('1', TokenType.NUMBER, loc, loc),
      new Token('else', TokenType.RESERVED, loc, loc),
      new Token('0', TokenType.NUMBER, loc, loc),
    ],
  },
  {
    in: 'IF a = b THEN 1 ELSE 0',
    out: [
      new Token('if', TokenType.RESERVED, loc, loc),
      new Token('a', TokenType.IDENT, loc, loc),
      new Token('=', TokenType.TOKEN, loc, loc),
      new Token('b', TokenType.IDENT, loc, loc),
      new Token('then', TokenType.RESERVED, loc, loc),
      new Token('1', TokenType.NUMBER, loc, loc),
      new Token('else', TokenType.RESERVED, loc, loc),
      new Token('0', TokenType.NUMBER, loc, loc),
    ],
  },
  {
    in: 'IF a >= 1 THEN b ELSE c',
    out: [
      new Token('if', TokenType.RESERVED, loc, loc),
      new Token('a', TokenType.IDENT, loc, loc),
      new Token('≥', TokenType.TOKEN, loc, loc),
      new Token('1', TokenType.NUMBER, loc, loc),
      new Token('then', TokenType.RESERVED, loc, loc),
      new Token('b', TokenType.IDENT, loc, loc),
      new Token('else', TokenType.RESERVED, loc, loc),
      new Token('c', TokenType.IDENT, loc, loc),
    ],
  },
  // exponent 'e' is case insensitive
  {
    in: '5E4',
    out: [new Token('5e4', TokenType.NUMBER, loc, loc)],
  },
  {
    in: '5e4',
    out: [new Token('5e4', TokenType.NUMBER, loc, loc)],
  },
  {
    in: '5.0000000000000e4.00000000000000',
    out: [new Token('5.0000000000000e4.00000000000000', TokenType.NUMBER, loc, loc)],
  },
  {
    in: '3',
    out: [new Token('3', TokenType.NUMBER, loc, loc)],
  },
  {
    in: '3.1.1e.1.1e1e1',
    out: [
      new Token('3.1', TokenType.NUMBER, loc, loc),
      new Token('.1e.1', TokenType.NUMBER, loc, loc),
      new Token('.1e1', TokenType.NUMBER, loc, loc),
      new Token('e1', TokenType.IDENT, loc, loc),
    ],
  },
  {
    in: '-3.222\n',
    out: [new Token('-', TokenType.TOKEN, loc, loc), new Token('3.222', TokenType.NUMBER, loc, loc)],
  },
  {
    in: '-30000.222',
    out: [new Token('-', TokenType.TOKEN, loc, loc), new Token('30000.222', TokenType.NUMBER, loc, loc)],
  },
  {
    in: '5.3e4.',
    out: [new Token('5.3e4.', TokenType.NUMBER, loc, loc)],
  },
  {
    in: '3 == 4 \n\n= 1',
    out: [
      new Token('3', TokenType.NUMBER, loc, loc),
      new Token('==', TokenType.TOKEN, loc, loc),
      new Token('4', TokenType.NUMBER, loc, loc),
      new Token('=', TokenType.TOKEN, loc, loc),
      new Token('1', TokenType.NUMBER, loc, loc),
    ],
  },
  {
    in: '3 <> 4',
    out: [
      new Token('3', TokenType.NUMBER, loc, loc),
      new Token('≠', TokenType.TOKEN, loc, loc),
      new Token('4', TokenType.NUMBER, loc, loc),
    ],
  },
  {
    in: '3 >< 4',
    out: [
      new Token('3', TokenType.NUMBER, loc, loc),
      new Token('>', TokenType.TOKEN, loc, loc),
      new Token('<', TokenType.TOKEN, loc, loc),
      new Token('4', TokenType.NUMBER, loc, loc),
    ],
  },
  {
    in: '3 <= 4',
    out: [
      new Token('3', TokenType.NUMBER, loc, loc),
      new Token('≤', TokenType.TOKEN, loc, loc),
      new Token('4', TokenType.NUMBER, loc, loc),
    ],
  },
  {
    in: '3 AND 4',
    out: [
      new Token('3', TokenType.NUMBER, loc, loc),
      new Token('&', TokenType.TOKEN, loc, loc),
      new Token('4', TokenType.NUMBER, loc, loc),
    ],
  },
  {
    in: '3 OR 4',
    out: [
      new Token('3', TokenType.NUMBER, loc, loc),
      new Token('|', TokenType.TOKEN, loc, loc),
      new Token('4', TokenType.NUMBER, loc, loc),
    ],
  },
  {
    in: 'NOT 0',
    out: [new Token('!', TokenType.TOKEN, loc, loc), new Token('0', TokenType.NUMBER, loc, loc)],
  },
  {
    in: '3 >= 4',
    out: [
      new Token('3', TokenType.NUMBER, loc, loc),
      new Token('≥', TokenType.TOKEN, loc, loc),
      new Token('4', TokenType.NUMBER, loc, loc),
    ],
  },
  {
    in: 'hares * birth_fraction',
    out: [
      new Token('hares', TokenType.IDENT, loc, loc),
      new Token('*', TokenType.TOKEN, loc, loc),
      new Token('birth_fraction', TokenType.IDENT, loc, loc),
    ],
  },
  {
    in: '',
    out: [],
  },
  {
    in: '\n',
    out: [],
  },
  {
    in: '{comment}',
    out: [],
  },
  {
    in: '{unclosed comment',
    out: [],
  },
  {
    in: '{comment before num}3',
    out: [new Token('3', TokenType.NUMBER, loc, loc)],
  },
  {
    in: '{}',
    out: [], // empty comment
  },
  {
    in: 'pulse(size_of_1_time_lynx_harvest, 4, 1e3)\n',
    out: [
      new Token('pulse', TokenType.IDENT, loc, loc),
      new Token('(', TokenType.TOKEN, loc, loc),
      new Token('size_of_1_time_lynx_harvest', TokenType.IDENT, loc, loc),
      new Token(',', TokenType.TOKEN, loc, loc),
      new Token('4', TokenType.NUMBER, loc, loc),
      new Token(',', TokenType.TOKEN, loc, loc),
      new Token('1e3', TokenType.NUMBER, loc, loc),
      new Token(')', TokenType.TOKEN, loc, loc),
    ],
  },
  {
    in: '"hares" * "birth fraction"',
    out: [
      new Token('"hares"', TokenType.IDENT, loc, loc),
      new Token('*', TokenType.TOKEN, loc, loc),
      new Token('"birth fraction"', TokenType.IDENT, loc, loc),
    ],
  },
  {
    in: 'sales[pizza, spinach]',
    out: [
      new Token('sales', TokenType.IDENT, loc, loc),
      new Token('[', TokenType.TOKEN, loc, loc),
      new Token('pizza', TokenType.IDENT, loc, loc),
      new Token(',', TokenType.TOKEN, loc, loc),
      new Token('spinach', TokenType.IDENT, loc, loc),
      new Token(']', TokenType.TOKEN, loc, loc),
    ],
  },
  {
    in: 'sales[pizza, *]',
    out: [
      new Token('sales', TokenType.IDENT, loc, loc),
      new Token('[', TokenType.TOKEN, loc, loc),
      new Token('pizza', TokenType.IDENT, loc, loc),
      new Token(',', TokenType.TOKEN, loc, loc),
      new Token('*', TokenType.TOKEN, loc, loc),
      new Token(']', TokenType.TOKEN, loc, loc),
    ],
  },
];

describe('lex', function(): void {
  LEX_TESTS.forEach(function(t: LexTestData): void {
    it('should lex ' + t.in, function(done): void {
      let lexer = new Lexer(t.in);
      let count = 0;
      for (let tok = lexer.nextTok(); tok !== null; tok = lexer.nextTok()) {
        let expected = t.out[count];
        expect(tok.type).to.equal(expected.type);
        if (tok.type !== expected.type) {
          console.log(`errrr: ${tok} -- ${expected}`);
        }
        expect(tok.tok).to.equal(expected.tok);
        count++;
      }
      expect(count).to.equal(t.out.length);
      done();
    });
  });
});
