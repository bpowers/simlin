#ifndef _XMUTIL_FUNCTION_STATE_H
#define _XMUTIL_FUNCTION_STATE_H

#include <assert.h>

#include <vector>

#include "../Symbol/SymbolTableBase.h"

class Variable;

/* state - contains information about the value of either explicit
   or implicit variables such as those associated with functions with
   memory - the base, and most common, state is simply a vector of
   numbers */

class State : public SymbolTableBase  // just to handle memory cleanup when an exception is thrown
{
public:
  State(SymbolNameSpace *sns);
  ~State(void);
  friend class VariableContentVar;
  virtual bool HasMemory(void) {
    return false;
  }  // but true for levels
  virtual bool UpdateOnPartialStep() {
    return true;
  }  // but false for STEP...
  inline int SetStateVec(double *p);
  inline void ClearComputeFlag(void) {
    cComputeFlag = 0;
  }
  inline double GetValue(int off) {
    return pVals[off];
  }
  inline void SetInitialValue(int off, double val) {
    pVals[off] = val;
  }
  virtual void SetActiveValue(int off, double val) {
    pVals[off] = val;
  }
  virtual void SetRateP(double *p) {
    assert(0);
  }
  /* don't use these - levells override SetActiveValue which is for rates
     during normal compuation
  inline double & operator [](int off) { return pVals[off] ; }
  inline double & Value(int off) { return pVals[off] ; }
  */
private:
  double *pVals;  // this will be allocated from a global pool to facilitate variable step size integration
  int iNVals;     // actual number in pVals - might be sparse for an array
  unsigned short cComputeFlag;           // used in equation ordering for unnamed state variables
  unsigned char cDynamicDependencyFlag;  // see context info
  unsigned char cSparse;                 // special value lookup required
};

class StateLevel : public State {
public:
  StateLevel(SymbolNameSpace *sns) : State(sns) {
  }
  ~StateLevel(void) {
  }
  bool HasMemory(void) {
    return true;
  }
  void SetActiveValue(int off, double val) {
    pRates[off] = val;
  }
  void SetRateP(double *p) {
    pRates = p;
  }

private:
  double *pRates;
};

class StateTime : public State {
public:
  StateTime(SymbolNameSpace *sns) : State(sns) {
  }
  ~StateTime(void) {
  }
  bool UpdateOnPartialStep() {
    return false;
  }  // but false for STEP...
private:
};

class StateSubscriptRange : public State {
  StateSubscriptRange(SymbolNameSpace *sns) : State(sns) {
  }
  ~StateSubscriptRange(void) {
  }

private:
};

#endif
