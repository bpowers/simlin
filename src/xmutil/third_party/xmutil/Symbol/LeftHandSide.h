#ifndef _XMUTIL_SYMBOL_LEFTHANDSIDE_H
#define _XMUTIL_SYMBOL_LEFTHANDSIDE_H

/* left hand side - mostly just ExpressionVariable but could include some extra
   stuff
   */

#include "Expression.h"
#include "SymbolListList.h"
#include "SymbolTableBase.h"

class LeftHandSide : public SymbolTableBase {
public:
  LeftHandSide(SymbolNameSpace *sns, ExpressionVariable *var, SymbolList *subs, SymbolListList *exceptlist,
               int interpmode);
  LeftHandSide(SymbolNameSpace *sns, const LeftHandSide &base, Variable *newvar);  // copy but put new variable in place
  ~LeftHandSide(void);
  ExpressionVariable *GetExpressionVariable() {
    return pExpressionVariable;
  }
  Variable *GetVariable(void) {
    return pExpressionVariable->GetVariable();
  }
  SymbolList *GetSubs() {
    return pExpressionVariable->GetSubs();
  }

private:
  LeftHandSide(const LeftHandSide &base);
  ExpressionVariable *pExpressionVariable;
  SymbolListList *pExceptList;
  int iInterpMode;  // nonstandard - Vensim for data equations what else?
};

#endif