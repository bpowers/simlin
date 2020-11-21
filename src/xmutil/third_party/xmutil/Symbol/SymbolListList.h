#ifndef _XMUTIL_SYMBOLLISTLIST_H
#define _XMUTIL_SYMBOLLISTLIST_H

#include <vector>

#include "SymbolList.h"
#include "SymbolTableBase.h"

class SymbolListList : public SymbolTableBase {
public:
  SymbolListList(SymbolNameSpace *sns, SymbolList *first);
  SymbolListList(SymbolNameSpace *sns, SymbolListList *orig);
  SymbolListList *Append(SymbolList *last) {
    vSymbolLists.push_back(last);
    return this;
  }
  int Length(void) {
    return vSymbolLists.size();
  }
  const SymbolList *operator[](int pos) const {
    return vSymbolLists[pos];
  }
  ~SymbolListList(void);

private:
  std::vector<SymbolList *> vSymbolLists;
  bool bNoDelete;
};

#endif