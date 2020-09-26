// Parse.h - just utility definitions for parsing - there is no
// class associated with this but it is used in the parsing
// for the different languages all of which eventually resolve
// to a common set of classes and structures
#ifndef _XMUTIL_PARSE_H
#define _XMUTIL_PARSE_H

/* define all the types - either with forward class declarations or
  as arbitray structure pointers so that all look different to the
  compiler */

class SymbolList;
class SymbolListList;
class Expression;
class ExpressionList;
class ExpressionTable;
class Equation;
class UnitExpression;
class LeftHandSide;
class ExpressionVariable;
class ExpressionSymbolList;
class Variable;
class Function;

typedef union _tag_parse_union {
  // note everything is 3 letters to make the .y files look a little nicer
  int tok;
  const char *lit;
  SymbolList *sml;
  SymbolListList *sll;
  Expression *exn;
  ExpressionList *exl;
  ExpressionVariable *var;
  ExpressionSymbolList *esl;
  Variable *sym;
  Equation *eqn;
  UnitExpression *uni;
  LeftHandSide *lhs;
  Function *fnc;
  ExpressionTable *tbl;
  double num;

} ParseUnion;

#ifndef NULL
#define NULL 0
#endif

#endif