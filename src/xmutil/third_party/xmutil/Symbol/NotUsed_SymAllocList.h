#ifndef _XMUTIL_SYMBOL_SYMALLOCLIST_H
#define _XMUTIL_SYMBOL_SYMALLOCLIST_H

#include <set>
class SymNameSpace;  // forward declaration

class SymbolTableBase;  // forward declaration

class SymAllocList {
public:
  SymAllocList(SymNameSpace *ns);
  ~SymAllocList(void);
  void Clean(void);
  void Reset(void);
  void Remove(SymbolTableBase *s) {
    sSymbols.erase(s);
  }
  inline void Add(SymbolTableBase *s) {
    sSymbols.insert(s);
  }
  SymNameSpace *GetNameSpace(void) {
    return pSymNameSpace;
  }

private:
  std::set<SymbolTableBase *> sSymbols;
  SymNameSpace *pSymNameSpace;
};

#endif