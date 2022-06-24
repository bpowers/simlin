#include "Symbol.h"

#include "../XMUtil.h"
#include "SymbolNameSpace.h"
#include "Variable.h"

// Abstract Symbol. Parent to Variable, Function, Token...
// Has a changeable name and returns a type
// does not manage self needs to be part of a collection

Symbol::Symbol(SymbolNameSpace *sns, const std::string &name) : SymbolTableBase(sns) {
  sName = name;
  pOwner = NULL;
  pSubranges = NULL;
  // insert into the name space sns if it has a name - empty get special treatment
  if (!name.empty())
    sns->Insert(this);
}

Symbol::~Symbol(void) {
  if (!HasGoodAlloc() && !sName.empty()) {  // remove from the lookup table
    GetSymbolNameSpace()->Remove(this);
  }
}

const std::string &Symbol::GetName(void) {
  return sName;
}

void Symbol::SetOwner(Symbol *var) {
  // static FILE *f; if (!f) f = fopen("c:\\temp\\temp.txt", "w");
  // if(f) fprintf(f, "%s <-- %s", var->GetName().c_str(), this->GetName().c_str());
  if (!pOwner || static_cast<Variable *>(pOwner)->Nelm() < static_cast<Variable *>(var)->Nelm()) {
    // if(f)fprintf(f,"  yes\n");
    var->AddSubrange(this, pOwner);
    if (pOwner)
      pOwner->SetOwner(var);
    pOwner = var;
  }
  // else if (pOwner == var)
  //{
  //	if (f)fprintf(f, "   already\n");
  // }
  // else
  //{
  //	if(f)fprintf(f,"   skipped\n");
  // }
}

void Symbol::AddSubrange(Symbol *sub, Symbol *oldowner) {
  if (oldowner) {
    std::set<Symbol *> *osr = oldowner->Subranges();
    if (osr) {
      if (pSubranges)
        pSubranges->insert(osr->begin(), osr->end());
      else
        pSubranges = osr;
    }
  }
  std::set<Symbol *> *osr = sub->Subranges();
  if (osr) {
    if (pSubranges)
      pSubranges->insert(osr->begin(), osr->end());
    else
      pSubranges = osr;
  }
  if (pSubranges == NULL)
    pSubranges = new std::set<Symbol *>();
  pSubranges->insert(sub);
}

SYMTYPE Symbol::isType(void) {
  return Symtype_None;
}
