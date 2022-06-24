#include "SymbolList.h"

#include "../XMUtil.h"
#include "Equation.h"
#include "Variable.h"

void SymbolList::SymbolListEntry::SetOwner(Variable *var) {
  if (eType == EntryType_LIST)
    this->u.pSymbolList->SetOwner(var);
  else
    this->u.pSymbol->SetOwner(var);
}

SymbolList::SymbolList(SymbolNameSpace *sns, Symbol *first, bool bang) : SymbolTableBase(sns) {
  vSymbols.push_back(SymbolListEntry(first, bang));
  pMapRange = NULL;
}

SymbolList::SymbolList(SymbolNameSpace *sns, SymbolList *first) : SymbolTableBase(sns) {
  vSymbols.push_back(SymbolListEntry(first));
  pMapRange = NULL;
}

SymbolList::~SymbolList(void) {
  // do nothing symbols in one hash table or another
}

// set the owner of array - only if we are bigger than other owners
void SymbolList::SetOwner(Variable *var) {
  if (vSymbols.empty())
    return;
  std::vector<Symbol *> expanded;
  for (size_t i = 0; i < vSymbols.size(); i++) {
    if (vSymbols[i].eType == EntryType_SYMBOL) {
      Equation::GetSubscriptElements(expanded, vSymbols[i].u.pSymbol);
    }
  }
  var->SetNelm(expanded.size());
  // the two that follow might end up doing the same thing depending on the content of the defining list
  for (size_t i = 0; i < vSymbols.size(); i++) {
    if (vSymbols[i].eType == EntryType_SYMBOL) {
      vSymbols[i].u.pSymbol->SetOwner(var);
    }
  }
  for (Symbol *s : expanded) {
    s->SetOwner(var);
  }
  if (expanded[0]->Owner() != var)
    var->SetOwner(expanded[0]->Owner());
}

void SymbolList::OutputComputable(ContextInfo *info) {
  if (vSymbols.empty())
    return;
  info->SetInSubList(true);
  *info << "[";
  for (size_t i = 0; i < vSymbols.size(); i++) {
    if (i)
      *info << ", ";
    if (vSymbols[i].eType == EntryType_SYMBOL) {
      // try to find the symbol in the lhs generic list - if there substitue specific otherwise
      // use the original symbol
      Symbol *s = info->GetLHSSpecific(vSymbols[i].u.pSymbol);
      *info << SpaceToUnderBar(s->GetName());
    } else if (vSymbols[i].eType == EntryType_BANG_SYMBOL) {
      Symbol *s = vSymbols[i].u.pSymbol;
      if (s->Owner() != s) {
        *info << "*:" << SpaceToUnderBar(s->GetName());  // new convention for XMILE to allow subrange use

        //// if this is a contiguous subrange we can use a:b notation - otherwise can't do it
        // std::vector<Symbol*> elms;
        // Equation::GetSubscriptElements(elms, s);
        // if(elms.size() == 1)
        //	*info << SpaceToUnderBar(elms[0]->GetName());
        // else
        //{
        //	std::vector<Symbol*> pelms;
        //	Symbol* owner = s->Owner();
        //	while (owner != owner->Owner())
        //		owner = owner->Owner(); // this is a bug elsewhere that does not properly reassign owners
        //	Equation::GetSubscriptElements(pelms, owner);
        //	int n = elms.size();
        //	int m = pelms.size();
        //	int i = 0;
        //	int j = 0;
        //	for (; j < m; j++)
        //	{
        //		if (pelms[j] == elms[i])
        //			break;
        //	}
        //	for (i = 1; i < n; i++)
        //	{
        //		j++; // this is next
        //		if (j >= m || pelms[j] != elms[i])
        //			break;
        //	}
        //	if (i == n)
        //	{
        //		*info << SpaceToUnderBar(elms.front()->GetName()) << ":" <<
        //SpaceToUnderBar(elms.back()->GetName());
        //	}
        //	else
        //		*info << "*" << SpaceToUnderBar(s->GetName());
        // }
      } else
        *info << "*";  // normally this is all
    }
  }
  *info << "]";
  info->SetInSubList(false);
}
