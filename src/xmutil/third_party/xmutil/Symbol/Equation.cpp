#include "Equation.h"

#include "../Model.h"
#include "../XMUtil.h"
#include "LeftHandSide.h"

Equation::Equation(SymbolNameSpace *sns, LeftHandSide *lhs, Expression *ex, int tok) : SymbolTableBase(sns) {
  pLeftHandSide = lhs;
  pExpression = ex;
  iEqType = tok;
}

Equation::~Equation(void) {
  if (HasGoodAlloc()) {
    delete pLeftHandSide;
    delete pExpression;
  }
}

Variable *Equation::GetVariable(void) {
  return pLeftHandSide->GetVariable();
}

ExpressionTable *Equation::GetTable(void) {
  return pExpression ? pExpression->GetTable() : NULL;
}

void Equation::CheckPlaceholderVars(Model *m) {
  pExpression->CheckPlaceholderVars(m, true);
}

std::string Equation::RHSFormattedXMILE(Variable *lhs, const std::vector<Symbol *> &subs,
                                        const std::vector<Symbol *> &dims, bool init) {
  if (!pExpression)
    return "{empty}";
  ContextInfo info(lhs);
  if (init)
    info.SetInitEqn(true);

  assert(subs.size() == dims.size());
  info.SetLHSElms(&subs, &dims);
  pExpression->OutputComputable(&info);
  return info.str();
}

bool Equation::IsActiveInit() {
  return pExpression && pExpression->IsActiveInit();
}

void Equation::GetSubscriptElements(std::vector<Symbol *> &vals, Symbol *s) {
  assert(s->isType() == Symtype_Variable);
  if (s->isType() != Symtype_Variable)
    return;
  Variable *v = static_cast<Variable *>(s);

  std::vector<Equation *> eqs = v->GetAllEquations();
  if (!eqs.empty())  // recur this is a nested def or equivalence
  {
    assert(eqs.size() == 1);
    Expression *exp = eqs[0]->GetExpression();
    if (exp->GetType() == EXPTYPE_Variable) {
      // equivalent - just make the right hand side the same
      v = static_cast<ExpressionVariable *>(exp)->GetVariable();
      eqs = v->GetAllEquations();
      assert(eqs.size() == 1);
      exp = eqs[0]->GetExpression();
    }
    assert(exp->GetType() == EXPTYPE_Symlist);
    if (exp->GetType() == EXPTYPE_Symlist) {
      SymbolList *symlist = static_cast<ExpressionSymbolList *>(exp)->SymList();
      int n = symlist->Length();
      for (int i = 0; i < n; i++) {
        const SymbolList::SymbolListEntry &elm = (*symlist)[i];
        if (elm.eType == SymbolList::EntryType_SYMBOL)  // only valid type
          GetSubscriptElements(vals, elm.u.pSymbol);
      }
    }
  } else  // otherwise the symbol is it
    vals.push_back(s);
}

// look at the lhs of equation to get element by element expanded subscripts - eg [plant] becomes [p1],[p2],[p3]
bool Equation::SubscriptExpand(std::vector<std::vector<Symbol *>> &elms,
                               std::vector<Symbol *> &orig)  // can be one or many depending on the subs
{
  SymbolList *subs = pLeftHandSide->GetSubs();
  if (!subs)
    return false;
  int n = subs->Length();
  std::vector<std::vector<Symbol *>> elmlist;
  std::vector<int> maxpos;
  std::vector<int> curpos;
  std::vector<Symbol *> cur_elms;
  orig.clear();
  for (int i = 0; i < n; i++) {
    cur_elms.clear();
    const SymbolList::SymbolListEntry &sub = (*subs)[i];
    if (sub.eType == SymbolList::EntryType_SYMBOL)  // only valid type
    {
      // see if it has an equation
      GetSubscriptElements(cur_elms, sub.u.pSymbol);
      orig.push_back(sub.u.pSymbol);
    } else {
      assert(false);
      cur_elms.push_back(NULL);
    }
    assert(!cur_elms.empty());
    elmlist.push_back(cur_elms);
    maxpos.push_back(cur_elms.size());
    curpos.push_back(0);
  }
  // now cycle through elmlist - might be a single entry - we need to do all combinations
  while (curpos[0] < maxpos[0]) {
    cur_elms.clear();
    for (int i = 0; i < n; i++)
      cur_elms.push_back(elmlist[i][curpos[i]]);
    elms.push_back(cur_elms);
    for (int j = n; j-- > 0;) {
      curpos[j]++;
      if (curpos[j] < maxpos[j])
        break;
      if (j)
        curpos[j] = 0;
    }
  }

  // elms.push_back(elmlist);
  return n > 0;
}

void Equation::Execute(ContextInfo *info) {
  // info->LHS(pLeftHandSide) ;
  if (info->GetComputeType() == CF_initial)
    pLeftHandSide->GetVariable()->SetInitialValue(0, pExpression->Eval(info));
  else
    pLeftHandSide->GetVariable()->SetActiveValue(0, pExpression->Eval(info));
}
void Equation::OutputComputable(ContextInfo *info) {
  *info << pLeftHandSide->GetVariable()->GetAlternateName();
  // subscripts
  if (pLeftHandSide->GetSubs())
    pLeftHandSide->GetSubs()->OutputComputable(info);
  *info << "=";
  if (info->GetComputeType() == CF_rate)
    *info << pLeftHandSide->GetVariable()->GetAlternateName() << "+dt*(";
  pExpression->OutputComputable(info);
  if (info->GetComputeType() == CF_rate)
    *info << ")";
  *info << "\n";
}

void Equation::GetVarsUsed(std::vector<Variable *> &vars) {
  pExpression->GetVarsUsed(vars);
}

int Equation::SubscriptCount(std::vector<Variable *> &elmlist) {
  if (iEqType == ':') /* a subscript equation */
    return 0;
  // we just need to look at the LHS variable and count the number of subscripts on it - all usage should be the same
  LeftHandSide *lhs = this->GetLeft();
  if (!lhs)
    return 0;
  SymbolList *subs = lhs->GetSubs();
  if (!subs)
    return 0;
  int n = subs->Length();
  for (int i = 0; i < n; i++) {
    const SymbolList::SymbolListEntry &sub = (*subs)[i];
    if (sub.eType == SymbolList::EntryType_SYMBOL)  // only valid type
      elmlist.push_back(static_cast<Variable *>(sub.u.pSymbol));
  }
  return n;
}
