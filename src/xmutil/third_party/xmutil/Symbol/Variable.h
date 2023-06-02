#ifndef _XMUTIL_SYMBOL_VARIABLE_H
#define _XMUTIL_SYMBOL_VARIABLE_H
#include <cstddef>
#include <vector>

#include "../Function/Function.h"
#include "../Function/State.h"
#include "../XMUtil.h"
#include "Equation.h"
#include "Symbol.h"
#include "UnitExpression.h"
class View;

/* varibale content - the true stuff for variables without the associated symbol
  the indirect subclassing is necessary because the variable type is not necessarily
  known when it is entered into the symbol table - contrast this with Function which
  is all just a class

  It might also be possible to incorporate State directly into VariableContent, but
  right now it is convenient to completely clear states when analyzing a model

  VariableContent is an abstract class
  */
enum XMILE_Type {
  XMILE_Type_UNKNOWN,
  XMILE_Type_AUX,
  XMILE_Type_DELAYAUX,
  XMILE_Type_STOCK,
  XMILE_Type_FLOW,
  XMILE_Type_ARRAY,
  XMILE_Type_ARRAY_ELM
};
class VariableContent {
public:
  VariableContent(void);
  virtual ~VariableContent(void) = 0;
  friend class Variable;
  friend class VariableContentVar;
  inline double Eval(ContextInfo *info) {
    return pState->GetValue(0);
  }  // todo subscritps
  inline void SetInitialValue(int off, double val) {
    pState->SetInitialValue(off, val);
  }
  inline void SetActiveValue(int off, double val) {
    pState->SetActiveValue(off, val);
  }

  virtual bool CheckComputed(Symbol *parent, ContextInfo *info, bool first) {
    return true;
  }
  virtual void Clear(void);
  virtual void AddEq(Equation *eq) {
  }
  virtual Equation *GetEquation(int pos) {
    return NULL;
  }
  virtual std::vector<Equation *> GetAllEquations() {
    return std::vector<Equation *>();
  }
  virtual void DropEquation(int pos) {
  }
  virtual void SetAllEquations(std::vector<Equation *> set) {
    assert(false);
  }
  virtual std::vector<Variable *> GetInputVars() {
    return std::vector<Variable *>();
  }
  virtual bool AddUnits(UnitExpression *un) {
    return false;
  }
  virtual UnitExpression *Units() {
    return NULL;
  }
  virtual void OutputComputable(ContextInfo *info) {
    assert(0);
  }
  virtual void CheckPlaceholderVars(Model *m) {
  }
  virtual void SetupState(ContextInfo *info) {
  }  // returns number of entries in state vector required (states can also claim thier own storage)
  virtual void SetAlternateName(const std::string &altname) {
  }
  virtual const std::string &GetAlternateName(void) {
    assert(0);
    std::string *s = new std::string;
    return *s;
  }
  virtual int SubscriptCount(std::vector<Variable *> &elmlist) {
    return 0;
  }
  virtual XMILE_Type MarkFlows(SymbolNameSpace *sns, Variable *parent, XMILE_Type intype) {
    return XMILE_Type_UNKNOWN;
  }

protected:
  State *pState;  // different kinds of states, also compute flags are here along with var type
};
class VariableContentSub : public VariableContent {
public:
  VariableContentSub(Variable *v) {
    pFamily = v;
  }
  ~VariableContentSub(void) {
  }

private:
  Variable *pFamily;
  std::vector<Variable *> vElements;  // includes count
};
class VariableContentElm : public VariableContent {
public:
  VariableContentElm(Variable *v) {
    pFamily = v;
  }
  ~VariableContentElm(void) {
  }

private:
  Variable *pFamily;
};
class VariableContentVar : public VariableContent {
public:
  VariableContentVar(void) {
    pUnits = NULL;
  }
  ~VariableContentVar(void) {
  }
  // friend class Variable ;

  bool CheckComputed(Symbol *parent, ContextInfo *info, bool first);
  void Clear(void);
  void AddEq(Equation *eq) {
    vEquations.push_back(eq);
  }
  virtual Equation *GetEquation(int pos) {
    return vEquations[pos];
  }
  virtual std::vector<Equation *> GetAllEquations() {
    return vEquations;
  }
  virtual void DropEquation(int pos) {
    vEquations.erase(vEquations.begin() + pos);
  }
  virtual void SetAllEquations(std::vector<Equation *> set) {
    vEquations = set;
  }
  virtual std::vector<Variable *> GetInputVars();
  bool AddUnits(UnitExpression *un) {
    if (!pUnits) {
      pUnits = un;
      return true;
    }
    return false;
  }
  UnitExpression *Units() {
    return pUnits;
  }
  void OutputComputable(ContextInfo *info) {
    *info << SpaceToUnderBar(sAlternateName);
  }
  void CheckPlaceholderVars(Model *m);
  void SetupState(ContextInfo *info);  // returns number of entries in state vector required (states can also claim
                                       // thier own storage)
  void SetAlternateName(const std::string &altname) {
    sAlternateName = altname;
  }
  const std::string &GetAlternateName(void) {
    return sAlternateName;
  }
  virtual int SubscriptCount(std::vector<Variable *> &elmlist);

protected:
  std::vector<Variable *>
      vSubscripts;  // for regular variables the family's for subscripts the family (possibly self) follwed by elements
  std::vector<Equation *> vEquations;
  std::string Comment;         // arbitrary UTF8 string
  std::string sAlternateName;  // for writing out equations as computer code
  UnitExpression *pUnits;      // units could be attached to equations
};

class Variable : public Symbol {
public:
  Variable(SymbolNameSpace *sns, const std::string &name);
  ~Variable(void);
  // virtual functions
  inline SYMTYPE isType(void) {
    return Symtype_Variable;
  }
  View *GetView() {
    return _view;
  }
  void SetView(View *view) {
    _view = view;
  }
  void SetViewOfCauses();
  void SetViewToCause(int depth);

  void SetComment(const std::string &com) {
    _comment = com;
  }
  bool Unwanted() const {
    return _unwanted;
  }
  void SetUnwanted(bool set) {
    _unwanted = set;
  }
  const std::string &Comment() {
    return _comment;
  }
  // virtuals passed on to content - need to keep pVariableContent populated before calling
  bool CheckComputed(ContextInfo *info, bool first) {
    if (pVariableContent)
      return pVariableContent->CheckComputed(this, info, first);
    return false;
  }
  void CheckPlaceholderVars(Model *m) {
    if (pVariableContent)
      pVariableContent->CheckPlaceholderVars(m);
  }
  void SetupState(ContextInfo *info) {
    if (pVariableContent)
      pVariableContent->SetupState(info);
  }
  int SubscriptCountVars(std::vector<Variable *> &elmlist) {
    return pVariableContent ? pVariableContent->SubscriptCount(elmlist) : 0;
  }
  // passthrough calls - many of these are virtual in VariableContent or passed through to yet another class
  void AddEq(Equation *eq);
  inline Equation *GetEquation(int pos) {
    return pVariableContent->GetEquation(pos);
  }
  std::vector<Equation *> GetAllEquations() {
    return pVariableContent ? pVariableContent->GetAllEquations() : std::vector<Equation *>();
  }
  inline bool AddUnits(UnitExpression *un) {
    return pVariableContent->AddUnits(un);
  }
  UnitExpression *Units() {
    return pVariableContent ? pVariableContent->Units() : NULL;
  }
  void OutputComputable(ContextInfo *info);
  inline double Eval(ContextInfo *info) {
    return pVariableContent->Eval(info);
  }
  inline std::vector<Variable *> GetInputVars() {
    return pVariableContent ? pVariableContent->GetInputVars() : std::vector<Variable *>();
  }
  inline void SetInitialValue(int off, double val) {
    pVariableContent->SetInitialValue(off, val);
  }
  inline void SetActiveValue(int off, double val) {
    pVariableContent->SetActiveValue(off, val);
  }
  inline void SetAlternateName(const std::string &altname) {
    pVariableContent->SetAlternateName(altname);
  }
  std::string GetAlternateName(void);

  void PurgeAFOEq();
  XMILE_Type MarkTypes(SymbolNameSpace *sns);  // mark the variableType of inflows/outflows
  void MarkStockFlows(SymbolNameSpace *sns);   // mark the variableType of inflows/outflows
  XMILE_Type VariableType() {
    return mVariableType;
  }
  void SetVariableType(XMILE_Type t) {
    mVariableType = t;
  }

  void MarkAsFlow() {
    bAsFlow = true;
  }
  bool AsFlow() const {
    return bAsFlow;
  }
  void MarkUsesMemory() {
    bUsesMemory = true;
  }
  bool UsesMemory() const {
    return bUsesMemory;
  }

  size_t Nelm() const {
    return iNelm;
  }
  void SetNelm(size_t set) {
    iNelm = set;
  }

  // for flowing
  bool HasUpstream() const {
    return _hasUpstream;
  }
  void SetHasUpstream(bool set) {
    _hasUpstream = set;
  }
  bool HasDownstream() const {
    return _hasDownstream;
  }
  void SetHasDownstream(bool set) {
    _hasDownstream = set;
  }

  // for other function calles
  inline VariableContent *Content(void) {
    return pVariableContent;
  }
  void SetContent(VariableContent *v) {
    pVariableContent = v;
  }
  std::vector<Variable *> &Inflows() {
    return mInflows;
  }
  std::vector<Variable *> &Outflows() {
    return mOutflows;
  }
  // virtual
private:
  std::string _comment;
  std::vector<Variable *> mInflows;  // only ued for stocks - should push to VariableConent
  std::vector<Variable *> mOutflows;
  VariableContent *pVariableContent;  // dependent on variable type which is not known on instantiation
  XMILE_Type mVariableType;
  size_t iNelm;  // used for subscript owners
  View *_view;   // view defined in
  bool _unwanted;
  bool _hasUpstream;
  bool _hasDownstream;
  bool bAsFlow;
  bool bUsesMemory;
};

#endif
