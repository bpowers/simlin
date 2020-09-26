#include "SymbolListList.h"

#include "../XMUtil.h"

SymbolListList::SymbolListList(SymbolNameSpace *sns, SymbolList *first) : SymbolTableBase(sns) {
  if (first)
    vSymbolLists.push_back(first);
  bNoDelete = false;
}

SymbolListList::SymbolListList(SymbolNameSpace *sns, SymbolListList *orig) : SymbolTableBase(sns) {
  vSymbolLists = orig->vSymbolLists;
  bNoDelete = true;
}

SymbolListList::~SymbolListList(void) {
  if (bNoDelete)
    return;
  int n = vSymbolLists.size();
  for (int i = 0; i < n; i++)
    delete vSymbolLists[i];
}
