#include "DynamoParseFunctions.h"

#include "../Symbol/Parse.h"
#include "../XMUtil.h"
#include "DynamoParse.h"

void dpyy_addfulleq(Equation *eq, int type) {
  return DPObject->AddFullEq(eq, type);
}
Equation *dpyy_addstockeq(LeftHandSide *lhs, ExpressionVariable *v, ExpressionList *exl, int token) {
  return DPObject->AddStockEq(lhs, v, exl, token);
}
Equation *dpyy_addeq(LeftHandSide *lhs, Expression *ex, ExpressionList *exl, int token) {
  return DPObject->AddEq(lhs, ex, exl, token);
}
Equation *dpyy_add_lookup(LeftHandSide *lhs, Expression *ex, ExpressionTable *tvl, int legacy) {
  return DPObject->AddTable(lhs, ex, tvl, legacy != 0);
}
LeftHandSide *dpyy_addexceptinterp(ExpressionVariable *var, SymbolListList *except, int interpmode) {
  return DPObject->AddExceptInterp(var, except, interpmode);
}
SymbolList *dpyy_symlist(SymbolList *in, Variable *add, int bang, Variable *end) {
  return DPObject->SymList(in, add, !!bang, end);
}
SymbolList *dpyy_mapsymlist(SymbolList *in, Variable *maprange, SymbolList *list) {
  return DPObject->MapSymList(in, maprange, list);
}
UnitExpression *dpyy_unitsdiv(UnitExpression *num, UnitExpression *denom) {
  return DPObject->UnitsDiv(num, denom);
}
UnitExpression *dpyy_unitsmult(UnitExpression *f, UnitExpression *s) {
  return DPObject->UnitsMult(f, s);
}
UnitExpression *dpyy_unitsrange(UnitExpression *f, double minval, double maxval, double increment) {
  return DPObject->UnitsRange(f, minval, maxval, increment);
}
SymbolListList *dpyy_chain_sublist(SymbolListList *sll, SymbolList *nsl) {
  return DPObject->ChainSublist(sll, nsl);
}
ExpressionList *dpyy_chain_exprlist(ExpressionList *el, Expression *e) {
  return DPObject->ChainExpressionList(el, e);
}
Expression *dpyy_num_expression(double num) {
  return DPObject->NumExpression(num);
}
Expression *dpyy_literal_expression(const char *lit) {
  return DPObject->LiteralExpression(lit);
}
ExpressionVariable *dpyy_var_expression(Variable *var, SymbolList *subs) {
  return DPObject->VarExpression(var, subs);
}
ExpressionSymbolList *dpyy_symlist_expression(SymbolList *sym, SymbolList *map) {
  return DPObject->SymlistExpression(sym, map);
}
Expression *dpyy_operator_expression(int oper, Expression *exp1, Expression *exp2) {
  return DPObject->OperatorExpression(oper, exp1, exp2);
}
Expression *dpyy_function_expression(Function *func, ExpressionList *eargs) {
  return DPObject->FunctionExpression(func, eargs);
}
Expression *dpyy_lookup_expression(ExpressionVariable *var, ExpressionList *args) {
  return DPObject->LookupExpression(var, args);
}
ExpressionTable *dpyy_tablepair(ExpressionTable *table, double x, double y) {
  return DPObject->TablePairs(table, x, y);
}
ExpressionTable *dpyy_tablevec(ExpressionTable *table, double val) {
  return DPObject->XYTableVec(table, val);
}
ExpressionTable *dpyy_tablerange(ExpressionTable *table, double x1, double y1, double x2, double y2) {
  return DPObject->TableRange(table, x1, y1, x2, y2);
}
void dpyy_macro_start() {
  DPObject->MacroStart();
}
void dpyy_macro_expression(Variable *name, ExpressionList *margs) {
  DPObject->MacroExpression(name, margs);
}
void dpyy_macro_end() {
  DPObject->MacroEnd();
}

/* the default functions called by parser */
int dpyylex(void) {
  return DPObject->yylex();
}
void dpyyerror(const char *str) {
  DPObject->yyerror(str);
}
