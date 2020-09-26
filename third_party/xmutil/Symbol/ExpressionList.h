#ifndef _XMUTIL_EXPRESSIONIST_H
#define _XMUTIL_EXPRESSIONIST_H

#include <vector>

#include "Expression.h"
class Model;

class ExpressionList : public SymbolTableBase {
public:
  ExpressionList(SymbolNameSpace *sns);
  ~ExpressionList(void);
  ExpressionList *Append(Expression *last) {
    vExpressions.push_back(last);
    return this;
  }
  int Length(void) {
    return vExpressions.size();
  }
  const Expression *operator[](int pos) const {
    return vExpressions[pos];
  }
  inline Expression *GetExp(int pos) {
    return vExpressions[pos];
  }
  void SetExp(int pos, Expression *exp) {
    vExpressions[pos] = exp;
  }
  void CheckPlaceholderVars(Model *m);
  bool CheckComputed(ContextInfo *info, unsigned wantargs);
  void OutputComputable(ContextInfo *info, unsigned wantargs);

private:
  std::vector<Expression *> vExpressions;
};

#endif