#ifndef _XMUTIL_CONTEXTINFO_H
#define _XMUTIL_CONTEXTINFO_H
#include <cassert>
#include <sstream>
#include <vector>
/* a utility class helpfu in sorting and evaluating equations
 */

/* computing flags - note that the incomp version must be the
  flag << 1 */
#define CF_xmile_output 0
#define CF_active 1  // int testing most the work is done here
#define CF_active_incomp 2
#define CF_unchanging 4  // subset of active not changing over time
#define CF_unchanging_incomp 8
#define CF_rate 16  // another subset of active
#define CF_rate_incomp 32
#define CF_initial 64
#define CF_initial_incomp 128  // that is it for a unsigned char

/* dynamic dependency flags - allows optimization of what is computed and what skipped */
#define DDF_constant 1
#define DDF_initial 2
#define DDF_time_varying 4
#define DDF_data 8
#define DDF_level 16

class Model;
class SymbolNameSpace;
class Equation;  // forward
class Symbol;
class Variable;

class ContextInfo : public std::ostringstream {
public:
  ContextInfo(Variable *lhs) {
    pLHS = lhs;
    iComputeType = 0;
    bInitEqn = false;
    bInSubList = false;
    bSelfIsPrevious = false;
    pEquations = NULL;
  }
  ~ContextInfo(void) {
  }
  friend class Model;
  // ContextInfo& operator << (const char *s) { std::cout << s ; return *this; }
  // ContextInfo& operator << (const std::string& s) {std::cout << s.c_str(); return *this; }
  // ContextInfo& operator << (const double num) { std::cout << num ; return *this ;}
  inline int GetComputeType(void) {
    return iComputeType;
  }
  inline void SetComputType(int type) {
    iComputeType = type;
  }
  inline bool InitEqn() {
    return bInitEqn;
  }
  inline void SetInitEqn(bool set) {
    bInitEqn = set;
  }
  inline double *GetLevelP(int count) {
    double *r = pCurLevel;
    pCurLevel += count;
    return r;
  }
  inline double *GetRateP(int count) {
    double *r = pCurRate;
    pCurRate += count;
    return r;
  }
  inline double *GetAuxP(int count) {
    double *r = pCurAux;
    pCurAux += count;
    return r;
  }
  inline void PushEquation(Equation *e) {
    if (pEquations)
      pEquations->push_back(e);
  }
  inline SymbolNameSpace *GetSymbolNameSpace(void) {
    return pSymbolNameSpace;
  }
  inline unsigned char GetDDF(void) {
    return cDynamicDependencyFlag;
  }
  inline void ClearDDF(void) {
    cDynamicDependencyFlag = 0;
  }
  inline void AddDDF(unsigned char flag) {
    cDynamicDependencyFlag |= flag;
  }
  inline double GetTime(void) {
    return dTime;
  }
  inline double GetDT(void) {
    return dDT;
  }
  inline void SetLHSElms(const std::vector<Symbol *> *generic, const std::vector<Symbol *> *specific) {
    assert(specific->size() == generic->size());
    pLHSElmsGeneric = generic;
    pLHSElmsSpecific = specific;
  }
  Symbol *GetLHSSpecific(Symbol *generic);
  bool InSubList() const {
    return bInSubList;
  }
  void SetInSubList(bool set) {
    bInSubList = set;
  }
  bool SelfIsPrevious() const {
    return bSelfIsPrevious;
  }
  void SetSelfIsPrevious(bool set) {
    bSelfIsPrevious = set;
  }
  Variable *LHS() {
    return pLHS;
  }

private:
  double dTime, dDT;
  double *pBaseLevel, *pCurLevel;
  double *pBaseRate, *pCurRate;
  double *pBaseAux, *pCurAux;
  SymbolNameSpace *pSymbolNameSpace;
  const std::vector<Symbol *> *pLHSElmsGeneric;   // left hand side current settings of subscripts
  const std::vector<Symbol *> *pLHSElmsSpecific;  // left hand side current settings of subscripts
  std::vector<Equation *> *pEquations;            /* passed from model - active or initial or... */
  Variable *pLHS;
  int iComputeType;                      // CF_... as above
  unsigned char cDynamicDependencyFlag;  // DDF_... as above
  bool bInitEqn;                         // for xmile
  bool bInSubList;
  bool bSelfIsPrevious;
};

#endif
