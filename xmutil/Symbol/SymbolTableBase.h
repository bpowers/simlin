#ifndef _XMUTIL_SYMBOLTABLEBASE_H
#define _XMUTIL_SYMBOLTABLEBASE_H

// an abstract base class for symbol table control - it is intended to
// help with putting equations out into different formats and also
// has a virtual cleanup function which lets it take care of freeing
// memory before it is destroyed - used to cleanup during exception
// handling when the various objects may not yet be properly connected
//
// another idea is to abstract out some of the serialization
//
//
//

#include "SymbolNameSpace.h"

// forward class declarations - used by the concrete classes
class Equation;
class LeftHandSide;
class Expression;
class Variable;

class SymbolTableBase {
public:
  SymbolTableBase(SymbolNameSpace *sns);
  inline void MarkGoodAlloc(void) {
    pSymbolNameSpace = NULL;
  }
  inline bool HasGoodAlloc(void) {
    return pSymbolNameSpace == NULL;
  }
  inline SymbolNameSpace *GetSymbolNameSpace(void) {
    return pSymbolNameSpace;
  }
  virtual ~SymbolTableBase(void) = 0;
  // SymNameSpace *GetNameSpace() { return pSymbolNameSpace ; }
private:
  SymbolNameSpace *pSymbolNameSpace;
};

#endif
