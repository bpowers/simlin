#include "VensimParseFunctions.h"

#include "../Symbol/Parse.h"
#include "../XMUtil.h"
#include "VensimParse.h"

void vpyy_addfulleq(Equation *eq, UnitExpression *un) {
  return VPObject->AddFullEq(eq, un);
}
Equation *vpyy_addeq(LeftHandSide *lhs, Expression *ex, ExpressionList *exl, int token) {
  return VPObject->AddEq(lhs, ex, exl, token);
}
Equation *vpyy_add_lookup(LeftHandSide *lhs, Expression *ex, ExpressionTable *tvl, int legacy) {
  return VPObject->AddTable(lhs, ex, tvl, legacy != 0);
}
LeftHandSide *vpyy_addexceptinterp(ExpressionVariable *var, SymbolListList *except, int interpmode) {
  return VPObject->AddExceptInterp(var, except, interpmode);
}
SymbolList *vpyy_symlist(SymbolList *in, Variable *add, int bang, Variable *end) {
  return VPObject->SymList(in, add, !!bang, end);
}
SymbolList *vpyy_mapsymlist(SymbolList *in, Variable *maprange, SymbolList *list) {
  return VPObject->MapSymList(in, maprange, list);
}
UnitExpression *vpyy_unitsdiv(UnitExpression *num, UnitExpression *denom) {
  return VPObject->UnitsDiv(num, denom);
}
UnitExpression *vpyy_unitsmult(UnitExpression *f, UnitExpression *s) {
  return VPObject->UnitsMult(f, s);
}
UnitExpression *vpyy_unitsrange(UnitExpression *f, double minval, double maxval, double increment) {
  return VPObject->UnitsRange(f, minval, maxval, increment);
}
SymbolListList *vpyy_chain_sublist(SymbolListList *sll, SymbolList *nsl) {
  return VPObject->ChainSublist(sll, nsl);
}
ExpressionList *vpyy_chain_exprlist(ExpressionList *el, Expression *e) {
  return VPObject->ChainExpressionList(el, e);
}
Expression *vpyy_num_expression(double num) {
  return VPObject->NumExpression(num);
}
Expression *vpyy_literal_expression(const char *lit) {
  return VPObject->LiteralExpression(lit);
}
ExpressionVariable *vpyy_var_expression(Variable *var, SymbolList *subs) {
  return VPObject->VarExpression(var, subs);
}
ExpressionSymbolList *vpyy_symlist_expression(SymbolList *sym, SymbolList *map) {
  return VPObject->SymlistExpression(sym, map);
}
Expression *vpyy_operator_expression(int oper, Expression *exp1, Expression *exp2) {
  return VPObject->OperatorExpression(oper, exp1, exp2);
}
Expression *vpyy_function_expression(Function *func, ExpressionList *eargs) {
  return VPObject->FunctionExpression(func, eargs);
}
Expression *vpyy_lookup_expression(ExpressionVariable *var, ExpressionList *args) {
  return VPObject->LookupExpression(var, args);
}
ExpressionTable *vpyy_tablepair(ExpressionTable *table, double x, double y) {
  return VPObject->TablePairs(table, x, y);
}
ExpressionTable *vpyy_tablevec(ExpressionTable *table, double val) {
  return VPObject->XYTableVec(table, val);
}
ExpressionTable *vpyy_tablerange(ExpressionTable *table, double x1, double y1, double x2, double y2) {
  return VPObject->TableRange(table, x1, y1, x2, y2);
}
void vpyy_macro_start() {
  VPObject->MacroStart();
}
void vpyy_macro_expression(Variable *name, ExpressionList *margs) {
  VPObject->MacroExpression(name, margs);
}
void vpyy_macro_end() {
  VPObject->MacroEnd();
}

/* the default functions called by parser */
int vpyylex(void) {
  return VPObject->yylex();
}
void vpyyerror(const char *str) {
  VPObject->yyerror(str);
}
