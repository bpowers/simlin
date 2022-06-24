#ifndef _XMUTIL_VENSIM_VENSIMPARSEFUNCTIONS_H
#define _XMUTIL_VENSIM_VENSIMPARSEFUNCTIONS_H
#include "../Symbol/Parse.h"

void vpyy_addfulleq(Equation *eq, UnitExpression *un);
Equation *vpyy_addeq(LeftHandSide *lhs, Expression *ex, ExpressionList *exl, int token);
Equation *vpyy_add_lookup(LeftHandSide *lhs, Expression *ex, ExpressionTable *tvl, int legacy);
LeftHandSide *vpyy_addexceptinterp(ExpressionVariable *var, SymbolListList *except, int interpmode);
SymbolList *vpyy_symlist(SymbolList *in, Variable *add, int bang, Variable *end);
SymbolList *vpyy_mapsymlist(SymbolList *in, Variable *maprange, SymbolList *list);
UnitExpression *vpyy_unitsdiv(UnitExpression *num, UnitExpression *denom);
UnitExpression *vpyy_unitsmult(UnitExpression *f, UnitExpression *s);
UnitExpression *vpyy_unitsrange(UnitExpression *f, double minval, double maxval, double increment);
SymbolListList *vpyy_chain_sublist(SymbolListList *sll, SymbolList *nsl);
ExpressionList *vpyy_chain_exprlist(ExpressionList *el, Expression *e);
Expression *vpyy_num_expression(double num);
Expression *vpyy_literal_expression(const char *tok);
ExpressionVariable *vpyy_var_expression(Variable *var, SymbolList *subs);
ExpressionSymbolList *vpyy_symlist_expression(SymbolList *subs, SymbolList *map);
Expression *vpyy_operator_expression(int oper, Expression *exp1, Expression *exp2);
Expression *vpyy_function_expression(Function *func, ExpressionList *args);
Expression *vpyy_lookup_expression(ExpressionVariable *var, ExpressionList *args);
ExpressionTable *vpyy_tablepair(ExpressionTable *table, double x, double y);
ExpressionTable *vpyy_tablevec(ExpressionTable *table, double val);
ExpressionTable *vpyy_tablerange(ExpressionTable *table, double x1, double y1, double x2, double y2);
void vpyy_macro_start();
void vpyy_macro_expression(Variable *name, ExpressionList *margs);
void vpyy_macro_end();

#endif
