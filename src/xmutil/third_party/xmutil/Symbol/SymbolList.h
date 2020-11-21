#ifndef _XMUTIL_SYMLIST_H
#define _XMUTIL_SYMLIST_H

#include <vector>

#include "Symbol.h"
#include "SymbolTableBase.h"

class SymbolList : public SymbolTableBase {
public:
  enum EntryType { EntryType_SYMBOL, EntryType_BANG_SYMBOL, EntryType_LIST };
  class SymbolListEntry {
  public:
    SymbolListEntry(Symbol *s, bool bang) {
      u.pSymbol = s;
      eType = bang ? EntryType_BANG_SYMBOL : EntryType_SYMBOL;
    }
    SymbolListEntry(SymbolList *s) {
      u.pSymbolList = s;
      eType = EntryType_LIST;
    }
    union {
      Symbol *pSymbol;
      SymbolList *pSymbolList;
    } u;
    void SetOwner(Variable *var);
    EntryType eType;
  };
  SymbolList(SymbolNameSpace *sns, Symbol *first, bool bang);
  SymbolList(SymbolNameSpace *sns, SymbolList *first);
  ~SymbolList(void);
  SymbolList *Append(Symbol *last, bool bang) {
    vSymbols.push_back(SymbolListEntry(last, bang));
    return this;
  }
  SymbolList *Append(SymbolList *next) {
    vSymbols.push_back(SymbolListEntry(next));
    return this;
  }
  int Length(void) {
    return vSymbols.size();
  }
  const SymbolListEntry &operator[](int pos) const {
    return vSymbols[pos];
  }
  bool IsMapList() {
    return pMapRange != NULL;
  }
  Symbol *MapRange() {
    return pMapRange;
  }
  void SetMapRange(Symbol *range) {
    assert(!pMapRange);
    pMapRange = range;
  }
  void SetOwner(Variable *var);
  virtual void OutputComputable(ContextInfo *info);

private:
  std::vector<SymbolListEntry> vSymbols;
  Symbol *pMapRange;
};

#endif
