#include "SymbolTableBase.h"

#include "../XMUtil.h"

SymbolTableBase::SymbolTableBase(SymbolNameSpace *sns) {
  pSymbolNameSpace = sns;
  sns->AddUnconfirmedAllocation(this);
}

SymbolTableBase::~SymbolTableBase(void) {
  if (!HasGoodAlloc()) {  // remove from this list no longer part of it
    pSymbolNameSpace->RemoveUnconfirmedAllocation(this);
  }
}
