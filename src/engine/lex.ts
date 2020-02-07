// Copyright 2019 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { Map } from 'immutable';

import { reserved } from './common';
import { SourceLoc, Token, TokenType } from './token';
import { defined, exists } from './util';

const OP: Map<string, string> = Map({
  not: '!',
  and: '&',
  or: '|',
  mod: '%',
});

function isWhitespace(ch: string | null): boolean {
  if (ch === null) {
    return false;
  }
  return /\s/.test(ch);
}
function isNumberStart(ch: string | null): boolean {
  if (ch === null) {
    return false;
  }
  return /[\d.]/.test(ch);
}
// For use in isIdentifierStart.  See below.
function isOperator(ch: string | null): boolean {
  if (ch === null) {
    return false;
  }
  // eslint-disable-next-line
  return /[=><\[\]\(\)\^\+\-\*\/,]/.test(ch);
}
// It is the year 2015, but JS regex's don't support Unicode. The \w
// character class only matches Latin1.  Work around this by sort of
// fuzzing this test - instead of checking for \w, check that we're
// not an operator or number or space.  I think this should be ok, but
// I can also imagine it missing something important.
function isIdentifierStart(ch: string): boolean {
  return !isNumberStart(ch) && !isWhitespace(ch) && (/[_"]/.test(ch) || !isOperator(ch));
}

// TODO(bp) better errors
export class Lexer {
  text: string;
  orig: string; // keep original string for error diagnostics

  private len: number;
  private pos: number;
  private line: number;
  private lstart: number;

  private rpeek: string | null; // single rune
  private tpeek: Token | null; // next token

  constructor(text: string) {
    this.text = text.toLowerCase();
    this.orig = text;

    this.len = text.length;
    this.pos = 0;
    this.line = 0;
    this.lstart = 0;

    this.rpeek = this.text[0];
    this.tpeek = null;
  }

  peek(): Token | null {
    if (!this.tpeek) {
      this.tpeek = this.nextTok();
    }

    return this.tpeek;
  }

  nextTok(): Token | null {
    if (this.tpeek) {
      const tpeek = this.tpeek;
      this.tpeek = null;
      return tpeek;
    }

    this.skipWhitespace();
    const peek = this.rpeek;

    // at the end of the input, peek is null.
    if (peek === null || peek === undefined) {
      return null;
    }

    // keep track of the start of the token, relative to the start of
    // the current line.
    const start: number = this.pos - this.lstart;
    const startLoc = new SourceLoc(this.line, start);

    if (isNumberStart(peek)) {
      return this.lexNumber(startLoc);
    }

    if (isIdentifierStart(peek)) {
      return this.lexIdentifier(startLoc);
    }

    const pos = this.pos;
    let len = 1;

    // match two-char tokens; if its not a 2 char token return the
    // single char tok.
    switch (peek) {
      case '=':
        this.nextRune();
        if (this.rpeek === '=') {
          this.nextRune();
          len++;
        }
        break;
      case '<':
        this.nextRune();
        if (this.rpeek === '=' || this.rpeek === '>') {
          this.nextRune();
          len++;
        }
        break;
      case '>':
        this.nextRune();
        if (this.rpeek === '=') {
          this.nextRune();
          len++;
        }
        break;
      default:
        this.nextRune();
        break;
    }

    let op = this.text.substring(pos, pos + len);
    // replace common multi-run ops with single-rune
    // equivalents.
    switch (op) {
      case '>=':
        op = '≥';
        break;
      case '<=':
        op = '≤';
        break;
      case '<>':
        op = '≠';
        break;
      default:
        break;
    }

    return new Token(op, TokenType.TOKEN, startLoc, startLoc.off(len));
  }

  private nextRune(): string | null {
    if (this.pos < this.len - 1) {
      this.rpeek = this.text[this.pos + 1];
    } else {
      this.rpeek = null;
    }
    this.pos++;

    return this.rpeek;
  }

  private skipWhitespace(): void {
    let inComment = false;
    do {
      if (this.rpeek === '\n') {
        this.line++;
        this.lstart = this.pos + 1;
      }
      if (inComment) {
        if (this.rpeek === '}') {
          inComment = false;
        }
        continue;
      }
      if (this.rpeek === '{') {
        inComment = true;
        continue;
      }
      if (!isWhitespace(this.rpeek)) {
        break;
      }
    } while (this.nextRune() !== null);
  }

  private fastForward(num: number): void {
    this.pos += num;
    if (this.pos < this.len) {
      this.rpeek = this.text[this.pos];
    } else {
      this.rpeek = null;
    }
  }

  private lexIdentifier(startPos: SourceLoc): Token {
    const quoted = this.rpeek === '"';

    const pos = this.pos;

    if (quoted) {
      this.nextRune();
    }

    while (true) {
      const r = this.nextRune();
      if (r === null) {
        break;
      }
      if ((isIdentifierStart(r) && r !== '"') || /\d/.test(r)) {
        continue;
      }
      if (quoted) {
        if (r === '"') {
          // eat closing "
          this.nextRune();
          break;
        }
        // any utf-8 chars are valid inside quotes
        continue;
      }
      break;
    }

    const len = this.pos - pos;
    let ident = this.text.substring(pos, pos + len);

    let type = TokenType.IDENT;

    if (reserved.has(ident)) {
      type = TokenType.RESERVED;
    } else if (OP.has(ident)) {
      type = TokenType.TOKEN;
      ident = defined(OP.get(ident));
    }

    return new Token(ident, type, startPos, startPos.off(len));
  }

  private lexNumber(startPos: SourceLoc): Token {
    // we do a .toLowerCase before the string gets to here, so we
    // don't need to match for lower and upper cased 'e's.
    const numStr = exists(/\d*(\.\d*)?(e(\d?(\.\d*)?)?)?/.exec(this.text.substring(this.pos)))[0];
    const len = numStr.length;
    this.fastForward(len);
    return new Token(numStr, TokenType.NUMBER, startPos, new SourceLoc(startPos.line, startPos.pos + len));
  }
}
