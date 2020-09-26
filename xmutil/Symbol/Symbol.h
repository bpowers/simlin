#ifndef _XMUTIL_SYMBOL_H
#define _XMUTIL_SYMBOL_H

#include <string>

#include "../ContextInfo.h"
#include "../Function/State.h"
#include "SymbolTableBase.h"

class Model;  // forward declaration
class Function;
class Variable;

class ModelGroup {
public:
  ModelGroup(const std::string &name, const std::string &owner) : sName(name), sOwner(owner) {
  }
  std::string sName;
  std::string sOwner;
  std::vector<Variable *> vVariables;
};

/* abstract class Symbol - used for model vairblaes, models, units
   and other things that may appear in the symbol table - these things
   share the same search space for lookup but are conceptually distinct */

enum SYMTYPE { Symtype_None, Symtype_Variable, Symtype_Units, Symtype_Model, Symtype_Function };

class Symbol : public SymbolTableBase {
public:
  Symbol(SymbolNameSpace *sns, const std::string &name);
  virtual ~Symbol(void) = 0;
  virtual SYMTYPE isType(void);
  virtual bool CheckComputed(ContextInfo *info, bool first) {
    return true;
  }  // do nothing
  virtual void CheckPlaceholderVars(Model *m) {
  }  // do nothing
  virtual void SetupState(ContextInfo *info) {
  }  // do nothing
  virtual int SubscriptCount(std::vector<Symbol *> &elmlist) {
    return 0;
  }
  const std::string &GetName(void);
  inline void SetName(const std::string &name) {
    sName = name;
  }
  void SetOwner(Symbol *var);
  void AddSubrange(Symbol *sub, Symbol *oldowner);
  std::set<Symbol *> *Subranges() {
    return pSubranges;
  }
  Symbol *Owner() {
    return pOwner ? pOwner : this;
  }

private:
  std::string sName;
  Symbol *pOwner;
  std::set<Symbol *> *pSubranges;  // backward from SetOwber
};

#endif  // once