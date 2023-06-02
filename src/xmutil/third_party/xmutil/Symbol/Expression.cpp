#include "Expression.h"

#include <algorithm>

#include "../Model.h"
#include "../Symbol/Parse.h"
#include "../XMUtil.h"
#include "Equation.h"
#include "ExpressionList.h"
#include "LeftHandSide.h"
#define YYSTYPE ParseUnion
#include "../Vensim/VYacc.tab.hpp"

Expression::Expression(SymbolNameSpace *sns) : SymbolTableBase(sns) {
}

Expression::~Expression(void) {
}

double Expression::Eval(ContextInfo *info) {
  return FLT_MAX;
}

void Expression::OutputComputable(ContextInfo *info) {
}

ExpressionFunction::~ExpressionFunction() {
  if (HasGoodAlloc())
    delete pArgs;
}

void ExpressionFunction::CheckPlaceholderVars(Model *m, bool isfirst) {
  pArgs->CheckPlaceholderVars(m);
}

void ExpressionFunction::GetVarsUsed(std::vector<Variable *> &vars) {
  if (!pArgs) {
    return;
  }

  int n = pArgs->Length();
  for (int i = 0; i < n; i++) {
    pArgs->GetExp(i)->GetVarsUsed(vars);
  }
}

void ExpressionFunctionMemory::CheckPlaceholderVars(Model *m, bool isfirst) {
  if (isfirst || !m) {
    pPlacholderEquation = NULL;  // deletion is handled by Model
  } else {
    pPlacholderEquation = m->AddUnnamedVariable(this);
  }
}

static void is_all_plus_minus(Expression *e, FlowList *fl, bool neg) {
  if (!e)
    fl->SetValid(false);
  else if (e->GetType() == EXPTYPE_Variable) {
    ExpressionVariable *ev = static_cast<ExpressionVariable *>(e);
    Variable *var = ev->GetVariable();
    if (neg) {
      if (var->HasUpstream())
        fl->SetValid(false);
      else {
        if (var->VariableType() == XMILE_Type_STOCK)
          fl->SetValid(false);
        fl->AddOutflow(var);
        var->SetHasUpstream(true);
      }
    } else {
      if (var->HasDownstream())
        fl->SetValid(false);
      else {
        if (var->VariableType() == XMILE_Type_STOCK)
          fl->SetValid(false);
        fl->AddInflow(var);
        var->SetHasDownstream(true);
      }
    }
  } else if (e->GetType() == EXPTYPE_Operator) {
    const char *op = e->GetOperator();
    if (op) {
      if (*op == '\0')  // could be () or unary +/- if - we need to flip neg
      {
        const char *before = e->GetBefore();
        if (before && *before == '-')
          neg = !neg;
        is_all_plus_minus(e->GetArg(0), fl, neg);
      } else if ((*op == '-' || *op == '+') && op[1] == '\0') {
        if (e->GetArg(0) != NULL)  // unary plus or leading - still okay
          is_all_plus_minus(e->GetArg(0), fl, neg);
        if (*op == '-')
          neg = !neg;
        is_all_plus_minus(e->GetArg(1), fl, neg);
      } else
        fl->SetValid(false);
    } else
      fl->SetValid(false);
  } else
    fl->SetValid(false);
}
void ExpressionVariable::GetVarsUsed(std::vector<Variable *> &vars) {
  for (Variable *var : vars) {
    if (var == pVariable)
      return;
  }
  vars.push_back(pVariable);
}  // list of variables used

bool ExpressionFunctionMemory::TestMarkFlows(SymbolNameSpace *sns, FlowList *fl, Equation *eq) {
  if (this->GetFunction()->GetName() != "INTEG" && this->GetFunction()->GetName() != "SINTEG")
    return false;
  // only care about active part here - if it all a+b+c-d-e or similar then we are good to go otherwise
  // we need to make up a new variable and then use that as the equation in place of what was here
  Expression *e = this->GetArgs()->GetExp(0);
  if (eq)  // make a change
  {
    assert(fl);
    // we set the first argument for the variable in the flow list
    ExpressionVariable *ev = new ExpressionVariable(sns, fl->NewVariable(), eq->GetLeft()->GetSubs());
    this->GetArgs()->SetExp(0, ev);

  } else if (fl)  // populat flow list
  {
    is_all_plus_minus(e, fl, false);
    fl->SetActiveExpression(e);
  }
  return true;
}

void FlowList::AddInflow(Variable *v) {
  if (std::find(vInflows.begin(), vInflows.end(), v) != vInflows.end())
    bValid = false;
  else if (std::find(vOutflows.begin(), vOutflows.end(), v) != vOutflows.end())
    bValid = false;
  else
    vInflows.push_back(v);
}

void FlowList::AddOutflow(Variable *v) {
  if (std::find(vInflows.begin(), vInflows.end(), v) != vInflows.end())
    bValid = false;
  else if (std::find(vOutflows.begin(), vOutflows.end(), v) != vOutflows.end())
    bValid = false;
  else
    vOutflows.push_back(v);
}

bool FlowList::operator==(const FlowList &rhs) {
  if (!bValid || !rhs.bValid || vInflows.size() != rhs.vInflows.size() || vOutflows.size() != rhs.vOutflows.size())
    return false;
  for (const Variable *v : rhs.vInflows) {
    if (std::find(vInflows.begin(), vInflows.end(), v) == vInflows.end())
      return false;
  }
  for (const Variable *v : rhs.vOutflows) {
    if (std::find(vOutflows.begin(), vOutflows.end(), v) == vOutflows.end())
      return false;
  }
  return true;
}

void ExpressionLogical::OutputComputable(ContextInfo *info) {
  if (pE1)
    pE1->OutputComputable(info);
  switch (mOper) {
  case VPTT_le:
    *info << " <= ";
    break;
  case VPTT_ge:
    *info << " >= ";
    break;
  case VPTT_ne:
    *info << " <> ";
    break;
  case VPTT_and:
    *info << " and ";
    break;
  case VPTT_or:
    *info << " or ";
    break;
  case VPTT_not:
    *info << " not ";
    break;

  default:
    assert(mOper < 128);
    *info << ' ';
    *info << (char)mOper;
    *info << ' ';
    break;
  }
  if (pE2)
    pE2->OutputComputable(info);
}

void ExpressionTable::TransformLegacy() {
  assert(!(vXVals.size() % 2));
  size_t n = vXVals.size() / 2;
  for (size_t i = 0; i < n; i++)
    vYVals[i] = vXVals[n + i];
  vXVals.resize(n);
  vYVals.resize(n);
}

void ExpressionLookup::OutputComputable(ContextInfo *info) {
  if (pExpressionVariable) {
    *info << "LOOKUP(";
    pExpressionVariable->OutputComputable(info);
    *info << ", ";
    pExpression->OutputComputable(info);
    *info << ")";
  } else {
    pExpression->OutputComputable(info);
  }
}
