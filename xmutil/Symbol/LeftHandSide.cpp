#include "LeftHandSide.h"

#include "../XMUtil.h"

LeftHandSide::LeftHandSide(SymbolNameSpace *sns, ExpressionVariable *var, SymbolList *subs, SymbolListList *exceptlist,
                           int interpmode)
    : SymbolTableBase(sns) {
  if (subs)  // replace in var - but make copy of var first - more memory leaks
  {
    pExpressionVariable = new ExpressionVariable(*var);
    pExpressionVariable->SetSubs(subs);
  } else
    pExpressionVariable = var;
  pExceptList = exceptlist;
  iInterpMode = interpmode;
}

LeftHandSide::LeftHandSide(SymbolNameSpace *sns, const LeftHandSide &base, Variable *var) : SymbolTableBase(sns) {
  pExpressionVariable = new ExpressionVariable(sns, var, base.pExpressionVariable->GetSubs());
  if (base.pExceptList)
    pExceptList = new SymbolListList(sns, base.pExceptList);
  else
    pExceptList = NULL;
  iInterpMode = base.iInterpMode;
}

LeftHandSide::~LeftHandSide(void) {
  if (HasGoodAlloc()) {
    if (pExpressionVariable)
      delete pExpressionVariable;
    if (pExceptList)
      delete pExceptList;
  }
}
