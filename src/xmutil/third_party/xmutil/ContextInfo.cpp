#include "ContextInfo.h"

#include "Symbol/Expression.h"
#include "Symbol/Symbol.h"
#include "Symbol/Variable.h"

Symbol *ContextInfo::GetLHSSpecific(Symbol *dim) {
  if (!pLHSElmsGeneric || dim->isType() != Symtype_Variable)
    return dim;
  size_t n = pLHSElmsGeneric->size();
  for (size_t i = 0; i < n; i++) {
    if ((*pLHSElmsGeneric)[i] == dim)
      return (*pLHSElmsSpecific)[i];
  }
  // no match
  // see if dim has anything it maps to - if so look for that on the LHS - then take the corresponding specific element
  Variable *v = static_cast<Variable *>(dim);
  std::vector<Equation *> eqs = v->GetAllEquations();
  if (!eqs.empty()) {
    Expression *exp = eqs[0]->GetExpression();
    assert(exp->GetType() == EXPTYPE_Symlist);
    if (exp->GetType() == EXPTYPE_Symlist) {
      ExpressionSymbolList *esl = static_cast<ExpressionSymbolList *>(exp);
      SymbolList *map = esl->Map();
      if (map) {
        // I believe we are missing the case where the mapping is explicitly laid out in a list, but I don't remember
        // the syntax for this walk through the things mapped to
        size_t count = map->Length();
        for (size_t j = 0; j < count; j++) {
          const SymbolList::SymbolListEntry &sle = (*map)[j];
          if (sle.eType == SymbolList::EntryType_SYMBOL)  // only type know how to deal with
          {
            Symbol *owner = sle.u.pSymbol;  // shoulw be  asubscript range
            for (size_t i = 0; i < n; i++) {
              if ((*pLHSElmsGeneric)[i] == owner) {
                // look for pLHSElmsSpecific in the entries for owner as subscript definition - use that position from
                // v above
                // Variable* mv = static_cast<Variable*>(owner);

                std::vector<Symbol *> list;
                Equation::GetSubscriptElements(list, owner);
                for (size_t k = 0; k < list.size(); k++) {
                  if (list[k] == (*pLHSElmsSpecific)[i]) {
                    std::vector<Symbol *> ours;
                    Equation::GetSubscriptElements(ours, v);
                    if (ours.size() == list.size())
                      return ours[k];
                  }
                }
              }
            }
            break;
          }
        }
      }
    }
  }

  return dim;
}
