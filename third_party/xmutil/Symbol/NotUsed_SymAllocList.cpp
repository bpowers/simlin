#include "NotUsed_SymAllocList.h"

#include <assert.h>

#include "../XMUtil.h"
#include "Symbol.h"

SymAllocList::SymAllocList(SymNameSpace *ns) {
  pSymNameSpace = ns;
}

SymAllocList::~SymAllocList(void) {
  assert(sSymbols.size() == 0);
}

void SymAllocList::Clean(void) {
  std::set<SymbolTableBase *>::iterator i;
  for (i = sSymbols.begin(); i != sSymbols.end(); i++) {
    delete (*i);
  }
  sSymbols.clear();
}

void SymAllocList::Reset(void) {
  std::set<SymbolTableBase *>::iterator i;
  for (i = sSymbols.begin(); i != sSymbols.end(); i++) {
    (*i)->MarkGoodAlloc();
  }
  sSymbols.clear();
}