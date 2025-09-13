#include "DynamoFunction.h"

#include "../Symbol/ExpressionList.h"
#include "../XMUtil.h"

// dynamo uses table y values as a vector and then x,min,max,increment so we output x and then setup the graphical based
// on y vals
/* 				<gf>
                                        <xscale min="1" max="13"/>
                                        <yscale min="0" max="100"/>
                                        <ypts>13.1,46.1,48.5,49.5,51,52.9,53.9,55.3,55.3,55.3,55.3,54.4,0</ypts>
                                </gf>
*/
void DFunctionTable::OutputComputable(ContextInfo *info, ExpressionList *arg) {
  if (arg->Length() == 5) {
    *info << "LOOKUP(";
    const_cast<Expression *>((*arg)[0])->OutputComputable(info);
    *info << ", ";
    const_cast<Expression *>((*arg)[1])->OutputComputable(info);
    *info << ")";
  } else {
    *info << "{error untranslatable table function call}";
    Function::OutputComputable(info, arg);
  }
}

bool DFunctionTable::SetTableXAxis(ExpressionList *args) const {
  if (args->Length() == 5) {
    ExpressionVariable *evar = dynamic_cast<ExpressionVariable *>(const_cast<Expression *>((*args)[0]));
    if (evar) {
      double xmin = 0;
      double xmax = 1;
      double increment = 1;
      Variable *var = evar->GetVariable();
      Expression *exp = const_cast<Expression *>((*args)[2]);
      if (exp->GetType() == EXPTYPE_Number)
        xmin = exp->Eval(NULL);
      exp = const_cast<Expression *>((*args)[3]);
      if (exp->GetType() == EXPTYPE_Number)
        xmax = exp->Eval(NULL);
      exp = const_cast<Expression *>((*args)[4]);
      if (exp->GetType() == EXPTYPE_Number)
        increment = exp->Eval(NULL);
      std::vector<Equation *> equations = var->GetAllEquations();
      size_t n = equations.size();
      // todo arrayed differences????
      bool found = false;
      for (size_t i = 0; i < n; i++) {
        Equation *eq = equations[i];
        exp = eq->GetExpression();
        if (exp->GetType() == EXPTYPE_Table) {
          static_cast<ExpressionTable *>(exp)->SetXAxis(var, xmin, xmax, increment);
          ;
          found = true;
        }
      }
      if (!found)
        log("ERROR - %s is used as a table but not defined that way.\n", var->GetName().c_str());
      return found;
    }
  }
  return false;
}
void DFunctionPulse::OutputComputable(ContextInfo *info, ExpressionList *arg) {
  if (arg->Length() == 2) {
    *info << "( IF TIME >= (";
    const_cast<Expression *>((*arg)[0])->OutputComputable(info);  // OutputComputable should really be const
    *info << ") AND TIME < ((";
    const_cast<Expression *>((*arg)[0])->OutputComputable(info);  // OutputComputable should really be const
    *info << ") + MAX(DT,";
    const_cast<Expression *>((*arg)[1])->OutputComputable(info);  // OutputComputable should really be const
    *info << ")) THEN 1 ELSE 0 )";
    return;
  }
  Function::OutputComputable(info, arg);
}

void DFunctionIfThenElse::OutputComputable(ContextInfo *info, ExpressionList *arg) {
  if (arg->Length() == 3) {
    *info << "( IF ";
    const_cast<Expression *>((*arg)[0])->OutputComputable(info);  // OutputComputable should really be const
    *info << " THEN ";
    const_cast<Expression *>((*arg)[1])->OutputComputable(info);  // OutputComputable should really be const
    *info << " ELSE ";
    const_cast<Expression *>((*arg)[2])->OutputComputable(info);  // OutputComputable should really be const
    *info << " )";
    return;
  }
  Function::OutputComputable(info, arg);
}

void DFunctionDelayN::OutputComputable(ContextInfo *info, ExpressionList *arg) {
  if (arg->Length() == 4) {
    *info << "DELAYN(";
    const_cast<Expression *>((*arg)[0])->OutputComputable(info);
    *info << ",";
    const_cast<Expression *>((*arg)[1])->OutputComputable(info);
    *info << ",";
    const_cast<Expression *>((*arg)[3])->OutputComputable(info);
    *info << ",";
    const_cast<Expression *>((*arg)[2])->OutputComputable(info);
    *info << ")";
  }
}
void DFunctionSmoothN::OutputComputable(ContextInfo *info, ExpressionList *arg) {
  if (arg->Length() == 4) {
    *info << "SMTHN(";
    const_cast<Expression *>((*arg)[0])->OutputComputable(info);
    *info << ",";
    const_cast<Expression *>((*arg)[1])->OutputComputable(info);
    *info << ",";
    const_cast<Expression *>((*arg)[3])->OutputComputable(info);
    *info << ",";
    const_cast<Expression *>((*arg)[2])->OutputComputable(info);
    *info << ")";
  }
}
