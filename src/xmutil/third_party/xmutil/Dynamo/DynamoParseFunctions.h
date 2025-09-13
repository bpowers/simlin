#ifndef _XMUTIL_DYNAMO_DYNAMOPARSEFUNCTIONS_H
#define _XMUTIL_DYNAMO_DYNAMOPARSEFUNCTIONS_H
#include "../Symbol/Parse.h"

void dpyy_addfulleq(Equation *eq, int type);
Equation *dpyy_addeq(LeftHandSide *lhs, Expression *ex, ExpressionList *exl, int token);
Equation *dpyy_addstockeq(LeftHandSide *lhs, ExpressionVariable *stock, ExpressionList *exl, int token);
Equation *dpyy_add_lookup(LeftHandSide *lhs, Expression *ex, ExpressionTable *tvl, int legacy);
LeftHandSide *dpyy_addexceptinterp(ExpressionVariable *var, SymbolListList *except, int interpmode);
SymbolList *dpyy_symlist(SymbolList *in, Variable *add, int bang, Variable *end);
SymbolList *dpyy_mapsymlist(SymbolList *in, Variable *maprange, SymbolList *list);
UnitExpression *dpyy_unitsdiv(UnitExpression *num, UnitExpression *denom);
UnitExpression *dpyy_unitsmult(UnitExpression *f, UnitExpression *s);
UnitExpression *dpyy_unitsrange(UnitExpression *f, double minval, double maxval, double increment);
SymbolListList *dpyy_chain_sublist(SymbolListList *sll, SymbolList *nsl);
ExpressionList *dpyy_chain_exprlist(ExpressionList *el, Expression *e);
Expression *dpyy_num_expression(double num);
Expression *dpyy_literal_expression(const char *tok);
ExpressionVariable *dpyy_var_expression(Variable *var, SymbolList *subs);
ExpressionSymbolList *dpyy_symlist_expression(SymbolList *subs, SymbolList *map);
Expression *dpyy_operator_expression(int oper, Expression *exp1, Expression *exp2);
Expression *dpyy_function_expression(Function *func, ExpressionList *args);
Expression *dpyy_lookup_expression(ExpressionVariable *var, ExpressionList *args);
ExpressionTable *dpyy_tablepair(ExpressionTable *table, double x, double y);
ExpressionTable *dpyy_tablevec(ExpressionTable *table, double val);
ExpressionTable *dpyy_tablerange(ExpressionTable *table, double x1, double y1, double x2, double y2);
void dpyy_macro_start();
void dpyy_macro_expression(Variable *name, ExpressionList *margs);
void dpyy_macro_end();

#endif
