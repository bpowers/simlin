#include "Variable.h"

#include <assert.h>

#include <iostream>

#include "../Symbol/Expression.h"
#include "../Symbol/LeftHandSide.h"
#include "../XMUtil.h"

// model Variable - this has subscript (families) units
// and the comment attached to it - inside of expressions
// we use an ExpressionVariable which has a pointer back
// to this

Variable::Variable(SymbolNameSpace *sns, const std::string &name) : Symbol(sns, name), _view(NULL) {
  pVariableContent = NULL;
  mVariableType = XMILE_Type_UNKNOWN;  // till typed
  iNelm = 0;
  _unwanted = false;
  _hasUpstream = _hasDownstream = false;
  bAsFlow = false;
  bUsesMemory = false;
}

Variable::~Variable(void) {
  if (pVariableContent) {
    if (HasGoodAlloc())
      pVariableContent->Clear();
    delete pVariableContent;
    pVariableContent = NULL;
  }
}

void Variable::SetViewOfCauses() {
  if (_view == NULL || pVariableContent == NULL || _unwanted)
    return;  // nothing useful to do with this
  std::vector<Equation *> eqns = pVariableContent->GetAllEquations();
  for (Equation *eqn : eqns) {
    std::vector<Variable *> vars;
    eqn->GetVarsUsed(vars);
    for (Variable *var : vars) {
      if (var->_view == NULL) {
        var->_view = _view;
        var->SetViewOfCauses();  // recur
      }
    }
  }
}

void Variable::SetViewToCause(int depth) {
  if (_view != NULL || pVariableContent == NULL || _unwanted || depth == 0)
    return;  // nothing useful to do with this
  depth--;
  std::vector<Equation *> eqns = pVariableContent->GetAllEquations();
  for (Equation *eqn : eqns) {
    std::vector<Variable *> vars;
    eqn->GetVarsUsed(vars);
    for (Variable *var : vars) {
      var->SetViewToCause(depth);  // recur
      if (var->_view != NULL) {
        _view = var->_view;
        return;
      }
    }
  }
}

std::string Variable::GetAlternateName(void) {
  std::string name = pVariableContent ? pVariableContent->GetAlternateName() : GetName();
  // strip out surrounding quotes if they exist - we want to deliver the name without them
  if (name.size() > 2 && name[0] == '\"' && name.back() == '\"' && name.find('.') == std::string::npos)
    name = name.substr(1, name.size() - 2);
  return name;
}

void Variable::PurgeAFOEq() {
  if (!pVariableContent)
    return;
  std::vector<Equation *> equations = pVariableContent->GetAllEquations();
  if (equations.size() <= 1)
    return;
  // if the first equation is an A FUNCTION OF equation then delete it
  Equation *eq = equations[0];
  Expression *exp = eq->GetExpression();
  if (exp->GetType() == EXPTYPE_Function) {
    Function *f = exp->GetFunction();
    if (f->GetName() == "A FUNCTION OF")
      pVariableContent->DropEquation(0);
  }
}

XMILE_Type Variable::MarkTypes(SymbolNameSpace *sns) {
  if (!pVariableContent)
    return mVariableType;

  std::vector<Equation *> equations = pVariableContent->GetAllEquations();

  if (equations.empty()) {
    // todo data variables have empty equations  - could fill something in here???
    return mVariableType;
  }

  /* if the equations are INTEG this is a stock and we need to validate flows - if we need to make
     up flows it has to be done here so that all the equations get the same net flow name
     we make up flows if the active part of INTEG uses something other then +/- of flows or
     if there are multiple equations that don't match (even if they all use +/- of flows)

     */
  // first pass - just figure out if there is anything to do -
  bool gotone = false;
  size_t n = equations.size();
  for (size_t i = 0; i < n; i++) {
    Equation *eq = equations[i];
    Expression *exp = eq->GetExpression();
    if (exp->GetType() == EXPTYPE_Symlist) {
      // the array should be a list of elements - we need to point those back to the array so that we can propery
      // dimension variables
      SymbolList *symlist = static_cast<ExpressionSymbolList *>(exp)->SymList();
      std::string name = this->GetName();
      symlist->SetOwner(this);  // this can recur
      mVariableType = XMILE_Type_ARRAY;
      // anything untyped in the symlist should be marked an elm - may contain other arrays
      int n = symlist->Length();
      for (int i = 0; i < n; i++) {
        const SymbolList::SymbolListEntry &sle = (*symlist)[i];
        if (sle.eType == SymbolList::EntryType_SYMBOL) {
          const Variable *elm = static_cast<const Variable *>(sle.u.pSymbol);
          if (elm->mVariableType == XMILE_Type_UNKNOWN)
            const_cast<Variable *>(elm)->mVariableType = XMILE_Type_ARRAY_ELM;
        }
      }
      return mVariableType;
    }
    if (exp->GetType() == EXPTYPE_NumberTable) {
      // need to blow this out to separate equations for allentries - only implemented for the single equation version
      // right now
      std::vector<std::vector<Symbol *>> elms;
      std::vector<Symbol *> subs;
      eq->SubscriptExpand(elms, subs);
      if (!elms.empty())  // can do something - otherwise just put out ???
      {
        ExpressionNumberTable *t = static_cast<ExpressionNumberTable *>(exp);
        const std::vector<double> &vals = t->GetVals();
        if (vals.size() != elms.size()) {
          log("Error the number of entries does not match array size for \"%s\"\n", this->GetName().c_str());
        } else {
          equations.erase(equations.begin() + i);
          size_t n2 = vals.size();
          for (size_t j = 0; j < n2; j++) {
            SymbolList *entry = new SymbolList(sns, elms[j][0], SymbolList::EntryType_SYMBOL);
            for (size_t k = 1; k < elms[j].size(); k++)
              entry->Append(elms[j][k], false);
            LeftHandSide *lhs = new LeftHandSide(sns, eq->GetLeft()->GetExpressionVariable(), entry, NULL, 0);
            ExpressionNumber *expnum = new ExpressionNumber(sns, vals[j]);
            Equation *neq = new Equation(sns, lhs, expnum, '=');
            equations.push_back(neq);
          }
          // now reenter with new equations
          pVariableContent->SetAllEquations(equations);
          return MarkTypes(sns);
        }
      }
    }
    if (exp->GetType() == EXPTYPE_Function) {
      Function *function = static_cast<ExpressionFunction *>(exp)->GetFunction();
      bool mrl = function->IsMemoryless();
      if (function->IsDelay())
        this->MarkUsesMemory();
      std::string name = function->GetName();
      if (name == "LOOKUP EXTRAPOLATE") {
        // if we get a LOOKUP_EXTRAPOLATE then try to mark the associated lookup - assume all will extrapolate
        std::vector<Variable *> vars;
        exp->GetVarsUsed(vars);
        // the first should be a graphical
        std::vector<Equation *> eqs = vars[0]->GetAllEquations();
        for (Equation *eq : eqs) {
          Expression *exp = eq->GetExpression();
          if (exp->GetType() == EXPTYPE_Table)
            static_cast<ExpressionTable *>(exp)->SetExtrapolate(true);
        }
      }
    }
    if (exp->TestMarkFlows(sns, NULL, NULL)) {
      gotone = true;
      break;  // one is all that is needed
    }
  }
  if (!gotone) {
    if (mVariableType == XMILE_Type_UNKNOWN) {
      // check to see if this is decorated as a flow
      if (this->AsFlow())
        mVariableType = XMILE_Type_FLOW;
      else if (this->UsesMemory())
        mVariableType = XMILE_Type_DELAYAUX;
      else
        mVariableType = XMILE_Type_AUX;
    }
    return mVariableType;
  }
  mVariableType = XMILE_Type_STOCK;
  return mVariableType;
}

void Variable::MarkStockFlows(SymbolNameSpace *sns) {
  // second pass, get the flow lists for everyone -- NOTE there is a bug in this code
  // because we don't check subscripts on the flows list so they may match even though
  // they shouldn't eg STOCK[A]=INTEG(FLOW[B],0) STOCK[B]=INTEG(FLOW[A],0)

  if (mVariableType != XMILE_Type_STOCK)
    return;
  std::vector<Equation *> equations = pVariableContent->GetAllEquations();
  if (equations.empty())
    return;

  std::vector<FlowList> flow_lists;
  flow_lists.resize(equations.size());
  size_t i = 0;
  bool match = true;
  for (Equation *eq : equations) {
    Expression *exp = eq->GetExpression();
    if (!exp->TestMarkFlows(sns, &flow_lists[i], NULL) || !flow_lists[i].Valid())
      match = false;
    else if (i > 0 && !(flow_lists[i] == flow_lists[i - 1]))
      match = false;  // all must be the same
    i++;
  }
  if (match) {
    for (Variable *v : flow_lists[0].Inflows()) {
      v->SetVariableType(XMILE_Type_FLOW);
      mInflows.push_back(v);
    }
    for (Variable *v : flow_lists[0].Outflows()) {
      v->SetVariableType(XMILE_Type_FLOW);
      mOutflows.push_back(v);
    }
    return;  // done
  }

  // if no active causes we don't need flows
  if (flow_lists.size() == 1 && flow_lists[0].Empty())
    return;

  // mismatched for invalid flow equations - create a flow variable and add it to the model
  std::string basename = this->GetName() + " net flow";
  std::string name = basename;
  i = 0;
  while (sns->Find(name)) {
    ++i;
    name = basename + "_" + std::to_string(i);
  }
  Variable *v = new Variable(sns, name);
  v->SetVariableType(XMILE_Type_FLOW);
  v->SetView(this->GetView());
  mInflows.push_back(v);

  // now we swap the active part of the INTEG equation for v and set v's equation to
  // the active part - this is equation by equation
  i = 0;
  for (Equation *eq : equations) {
    // left hand side for this variable
    LeftHandSide *lhs = new LeftHandSide(sns, *eq->GetLeft(), v);  // replace var in lhs equation
    Equation *neweq = new Equation(sns, lhs, flow_lists[i].ActiveExpression(), '=');
    v->AddEq(neweq);
    flow_lists[i].SetNewVariable(v);
    eq->GetExpression()->TestMarkFlows(sns, &flow_lists[i], eq);
    i++;
  }
  // don't do this - we get some memory leakage but risk a crash otherwise v->MarkGoodAlloc();
}

void VariableContent::Clear(void) {
  if (pState) {
    delete pState;
    pState = 0;
  }
}

void VariableContentVar::Clear(void) {
  // clear memory
  VariableContent::Clear();
  if (pUnits)
    delete pUnits;
  int i;
  int n = vSubscripts.size();
  for (i = 0; i < n; i++)
    delete vSubscripts[i];
  n = vEquations.size();
  for (i = 0; i < n; i++)
    delete vEquations[i];
  // comment takes care of itself
}
std::vector<Variable *> VariableContentVar::GetInputVars() {
  std::vector<Variable *> vars;
  for (Equation *eq : this->vEquations)
    eq->GetVarsUsed(vars);
  return vars;
}

void Variable::AddEq(Equation *eq) {
  if (!pVariableContent) {
    try {
      pVariableContent = new VariableContentVar;
      pVariableContent->SetAlternateName(this->GetName());  // until overidden
    } catch (...) {
      throw "Memory failure adding equations";
    }
  }
  pVariableContent->AddEq(eq);
}

void Variable::OutputComputable(ContextInfo *info) {
  if (info->LHS() == this) {
    if (info->SelfIsPrevious())
      *info << "PREVIOUS(SELF, NAN)";
    else
      *info << "SELF";
    return;
  }
  if (this->mVariableType == XMILE_Type_ARRAY) {
    Symbol *s = info->GetLHSSpecific(this);
    if (s && s != this) {
      *info << SpaceToUnderBar(this->GetName()) << ".";
      *info << SpaceToUnderBar(s->GetName());
      return;
    }
  }
  if (pVariableContent)
    pVariableContent->OutputComputable(info);
  else {
    if (mVariableType == XMILE_Type_ARRAY_ELM && !info->InSubList())
      *info << SpaceToUnderBar(this->Owner()->GetName()) << ".";
    *info << SpaceToUnderBar(GetName());
  }
}

VariableContent::VariableContent(void) {
  pState = NULL;
}

VariableContent::~VariableContent(void) {
}

void VariableContentVar::CheckPlaceholderVars(Model *m) {
  for (Equation *eq : vEquations) {
    eq->CheckPlaceholderVars(m);
  }
}

bool VariableContentVar::CheckComputed(Symbol *parent, ContextInfo *info, bool first) {
  // log("Checking out %s\n",GetName().c_str()) ;
  if (!pState) {
    // log("   - not computable ignoring\n") ;
    return true;
  }
  if (pState->cComputeFlag & info->GetComputeType()) {
    if (info->GetComputeType() == CF_active) {
      if (pState->HasMemory())
        info->AddDDF(DDF_level);  // not recorded in cDynamicDependencyFlag
      else
        info->AddDDF(pState->cDynamicDependencyFlag);
    }
    return true;  // done
  }
  int intype = info->GetComputeType() << 1;
  if (pState->cComputeFlag & intype) {
    if (info->GetComputeType() == CF_initial)
      log("Simultaneous initial equations found \n");
    else {
      if (pState->HasMemory()) {  // first call was for rates - now for level
        assert(!first);
        info->AddDDF(DDF_level);
        return true;
      }
      log("Simultaneous active equations found \n");
    }
    log("     %s\n", parent->GetName().c_str());
    pState->cComputeFlag &= ~intype;
    return false;
  } else if (!first && (info->GetComputeType() != CF_initial) && pState->HasMemory()) {
    // log("Not tracing further for level  %s\n",GetName().c_str()) ;
    info->AddDDF(
        DDF_level);  // this is the level - even if the rate is a constant (only a 0 rate would really be unchanging)
    return true;
  } else if (first && info->GetComputeType() == CF_initial && !pState->HasMemory()) {
    return true; /* don't need to do anything, if var is needed will be called not first */
  } else {       // really need to check
    unsigned char ddf = info->GetDDF();
    if (info->GetComputeType() == CF_active) {
      info->ClearDDF();
      if (!pState->UpdateOnPartialStep())
        info->AddDDF(DDF_time_varying);
    }
    pState->cComputeFlag |= intype;
    for (Equation *e : vEquations) {
      if (!e->GetExpression()->CheckComputed(info)) {
        log("     %s\n", parent->GetName().c_str());
        pState->cComputeFlag &= ~intype;
        pState->cComputeFlag |= info->GetComputeType();  // don't reenter
        return false;
      }
    }
    if (info->GetComputeType() == CF_active) {
      pState->cDynamicDependencyFlag = info->GetDDF();
      info->AddDDF(ddf);
    }
  }

  pState->cComputeFlag = (pState->cComputeFlag | info->GetComputeType()) & (~intype);

  /* add the equations - some can be ignored */
  if (info->GetComputeType() == CF_active) {
    if (pState->HasMemory() || !(pState->cDynamicDependencyFlag & (DDF_level | DDF_data | DDF_time_varying)))
      return true;
  } else if (info->GetComputeType() == CF_unchanging) {
    if (pState->HasMemory() || pState->cDynamicDependencyFlag & (DDF_level | DDF_data | DDF_time_varying))
      return true;
  } else if (info->GetComputeType() == CF_rate) {
    if (!pState->HasMemory())
      return true;
  }
  log("Outputting equations for  %s\n", parent->GetName().c_str());
  for (Equation *e : vEquations) {
    info->PushEquation(e);
  }
  return true;
}

void VariableContentVar::SetupState(ContextInfo *info) {
  bool hasmemory = false;
  bool timedependent = false;
  if (!info) {
    if (pState)
      delete pState;
    pState = NULL;
    return;
  }
  if (!pState) { /* create it - actually this is called twice with value assignment on second pass */
    if (info->GetComputeType())
      return;  // do nothing no state allocated on the previous try
    // find out what defines this to determine type
    bool haseq = false;
    for (Equation *e : vEquations) {
      haseq = true;
      if (e->GetExpression()->GetType() == EXPTYPE_Table) {
        return;  // for now no state assigned
      }
      Function *f = e->GetExpression()->GetFunction();
      if (f) {
        if (!f->IsMemoryless())
          hasmemory = true;
        if (f->IsTimeDependent())
          timedependent = true;
      }
    }
    // empty equation causes what???
    if (!haseq)
      timedependent = true;  // consistent with exog variables
    if (hasmemory)
      pState = new StateLevel(info->GetSymbolNameSpace());
    else if (timedependent)
      pState = new StateTime(info->GetSymbolNameSpace());
    else
      pState = new State(info->GetSymbolNameSpace());
    pState->iNVals = 1;  // todo arrays
  }
  // the following addresses will be nonsense on the first pass we just
  // use them to set the requires size for the integration state vector
  if (pState->HasMemory()) {
    pState->pVals = info->GetLevelP(pState->iNVals);
    pState->SetRateP(info->GetRateP(pState->iNVals));
  } else
    pState->pVals = info->GetAuxP(pState->iNVals);
}

int VariableContentVar::SubscriptCount(std::vector<Variable *> &elmlist) {
  if (vEquations.empty())
    return 0;
  int count = vEquations[0]->SubscriptCount(elmlist);
  if (count > 0) {
    if (vEquations.size() != 1) {
      for (size_t i = 1; i < vEquations.size(); i++) {
        std::vector<Variable *> other;
        if (vEquations[0]->SubscriptCount(other) != count)
          throw "Bad subscript equations";
      }
    }
    // we need to get to the array not the elements for elmlist - not map to parent only if multiple equations
    // for (int i = 0; i < count; i++)
    //{
    // Variable* sym = elmlist[i];
    // //Symbol* var = sym->Owner();
    // if (sym)
    //  elmlist[i] = sym;
    //}

    return count;
  }
  return 0;
}
