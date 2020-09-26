#include "TableFunction.h"

#include <assert.h>

#include <vector>

#include "../Symbol/Expression.h"
#include "../Symbol/Variable.h"
#include "../XMUtil.h"

TableFunction::TableFunction(void) {
}

TableFunction::~TableFunction(void) {
}

double TableFunction::Eval(ExpressionVariable *v, Expression *e, ContextInfo *info) {
  double d = e->Eval(info);
  ExpressionTable *et;

  et = static_cast<ExpressionTable *>(v->GetVariable()->GetEquation(0)->GetExpression());
  assert(et->GetType() == EXPTYPE_Table);
  double *x = et->GetXVals()->data();
  double *y = et->GetYVals()->data();
  int n = et->GetXVals()->size();
  assert(n == et->GetYVals()->size());
  if (d <= x[0])
    return y[0];
  if (d >= x[n - 1])
    return y[n - 1];
  for (int i = 0; i < n; i++) {
    if (d >= x[i]) {
      while (d >= x[i + 1])
        i++;  // in case
      return y[i] + (y[i + 1] - y[i]) * (d - x[i]) / (x[i + 1] - x[i]);
    }
  }
  assert(0);
  return FLT_MAX;
}