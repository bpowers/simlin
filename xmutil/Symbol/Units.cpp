#include "Units.h"

#include "../XMUtil.h"

Units::Units(SymbolNameSpace *sns, const std::string &name) : Symbol(sns, name) {
  pUnitExpression = NULL;
}

Units::~Units(void) {
  if (pUnitExpression)
    delete pUnitExpression;
}
