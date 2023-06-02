#ifndef _XMUTIL_SYMBOL_FUNCTION_H
#define _XMUTIL_SYMBOL_FUNCTION_H
#include "../Symbol/Symbol.h"
#include "State.h"

class Expression;      /* forward declaration */
class ExpressionList;  // forward
class UnitExpression;

/* abstract class - every function has its own subclass
   defined here with bodies in Function.cpp or in their own
   files depending on complexity */

class Function : public Symbol {
public:
  Function(SymbolNameSpace *sns, const std::string &name, int narg);
  virtual ~Function(void) = 0;
  SYMTYPE isType(void) {
    return Symtype_Function;
  }
  virtual bool AsKeyword(void) {
    return false;
  }  // for the parser - treats name as keyword not function
  virtual bool IsMemoryless(void) {
    return true;
  }
  virtual bool IsDelay(void) {
    return false;
  }
  virtual bool IsTimeDependent(void) {
    return false;
  }
  virtual double Eval(Expression *ex, ExpressionList *arg, ContextInfo *info) {
    return 0;
  }  // make this pure virtual if finishing engine is required
  virtual bool CheckComputedList(ContextInfo *info, ExpressionList *arg);
  virtual void OutputComputable(ContextInfo *info, ExpressionList *arg);
  virtual std::string ComputableName(void) {
    return "";
  }
  virtual std::string ComputableNameInit(void) {
    return "";
  }
  virtual bool IsActiveInit() {
    return false;
  }
  int NumberArgs(void) {
    return iNumberArgs;
  }

protected:
  int iNumberArgs;
};

class DFunction : public Function {
public:
  DFunction(SymbolNameSpace *sns, const std::string &name, int narg) : Function(sns, name, narg) {
  }
  virtual bool IsDelay(void) override {
    return true;
  }
};

class UnknownFunction : public Function {
public:
  UnknownFunction(SymbolNameSpace *sns, const std::string &name, int narg) : Function(sns, name, narg), sName(name) {
  }
  ~UnknownFunction() {
  }
  virtual void OutputComputable(ContextInfo *info, ExpressionList *arg);

private:
  std::string sName;
};

class FunctionMemoryBase : public Function {
public:
  FunctionMemoryBase(SymbolNameSpace *sns, const std::string &name, int narg, unsigned actarg, unsigned iniarg)
      : Function(sns, name, narg) {
    iActiveArgMark = BitFlip(actarg);
    iInitArgMark = BitFlip(iniarg);
  }
  ~FunctionMemoryBase(void) {
  }
  unsigned BitFlip(unsigned bits);
  virtual bool IsMemoryless(void) override {
    return false;
  }
  virtual bool IsDelay(void) override {
    return true;
  }
  virtual bool CheckComputedList(ContextInfo *info, ExpressionList *arg) override;
  virtual void OutputComputable(ContextInfo *info, ExpressionList *arg) override;

private:
  unsigned iInitArgMark;
  unsigned iActiveArgMark;
};

class MacroFunction : public Function {
public:
  class EqUnitPair {
  public:
    EqUnitPair(Equation *eq, UnitExpression *un) : equation(eq), units(un) {
    }
    Equation *equation;
    UnitExpression *units;
  };
  SymbolNameSpace *NameSpace() {
    return pSymbolNameSpace;
  }
  MacroFunction(SymbolNameSpace *sns, SymbolNameSpace *local, const std::string &name, ExpressionList *margs);
  ~MacroFunction() {
    delete pSymbolNameSpace;
  }
  void AddEq(Equation *equation, UnitExpression *units) {
    mEquations.push_back(EqUnitPair(equation, units));
  }
  ExpressionList *Args() {
    return mArgs;
  }
  virtual std::string ComputableName(void);

private:
  SymbolNameSpace *pSymbolNameSpace;  // local
  MacroFunction(const MacroFunction &other);
  ExpressionList *mArgs;
  std::vector<EqUnitPair> mEquations;
};

#define FSubclassKeyword(name, xname, narg)                   \
  class name : public Function {                              \
  public:                                                     \
    name(SymbolNameSpace *sns) : Function(sns, xname, narg) { \
      ;                                                       \
    }                                                         \
    ~name(void) {                                             \
    }                                                         \
    std::string ComputableName(void) {                        \
      return " ?? ";                                          \
    }                                                         \
    bool AsKeyword(void) {                                    \
      return true;                                            \
    }                                                         \
  }

#define FSubclassStart(name, xname, narg, cname)              \
  class name : public Function {                              \
  public:                                                     \
    name(SymbolNameSpace *sns) : Function(sns, xname, narg) { \
      ;                                                       \
    }                                                         \
    ~name(void) {                                             \
    }                                                         \
    std::string ComputableName(void) {                        \
      return cname;                                           \
    }                                                         \
                                                              \
  private:

#define FSubclass(name, xname, narg, cname) \
  FSubclassStart(name, xname, narg, cname)  \
  }

#define DFSubclassStart(name, xname, narg, cname)              \
  class name : public DFunction {                              \
  public:                                                      \
    name(SymbolNameSpace *sns) : DFunction(sns, xname, narg) { \
      ;                                                        \
    }                                                          \
    ~name(void) {                                              \
    }                                                          \
    std::string ComputableName(void) {                         \
      return cname;                                            \
    }                                                          \
                                                               \
  private:

#define DFSubclass(name, xname, narg, cname) \
  DFSubclassStart(name, xname, narg, cname)  \
  }

#define FSubclassMemoryStart(name, xname, narg, actarg, iniarg, cnamea, cnamei)         \
  class name : public FunctionMemoryBase {                                              \
  public:                                                                               \
    name(SymbolNameSpace *sns) : FunctionMemoryBase(sns, xname, narg, actarg, iniarg) { \
    }                                                                                   \
    ~name(void) {                                                                       \
    }                                                                                   \
    virtual std::string ComputableName(void) override {                                 \
      return cnamea;                                                                    \
    }                                                                                   \
    virtual std::string ComputableNameInit(void) override {                             \
      return cnamei;                                                                    \
    }                                                                                   \
                                                                                        \
  private:

#define FSubclassMemory(name, xname, narg, actarg, iniarg, cnamea, cnamei) \
  FSubclassMemoryStart(name, xname, narg, actarg, iniarg, cnamea, cnamei)  \
  }                                                                        \
  ;

#define FSubclassTimeStart(name, xname, narg, cname)          \
  class name : public Function {                              \
  public:                                                     \
    name(SymbolNameSpace *sns) : Function(sns, xname, narg) { \
      ;                                                       \
    }                                                         \
    ~name(void) {                                             \
    }                                                         \
    bool IsTimeDependent(void) {                              \
      return true;                                            \
    }                                                         \
    std::string ComputableName(void) {                        \
      return cname;                                           \
    }                                                         \
                                                              \
  private:

#define FSubclassTime(name, xname, narg, cname) \
  FSubclassTimeStart(name, xname, narg, cname)  \
  }                                             \
  ;

FSubclass(FunctionAbs, "ABS", 1, "ABS");
FSubclass(FunctionExp, "EXP", 1, "EXP");
FSubclass(FunctionSqrt, "SQRT", 1, "SQRT");

FSubclass(FunctionCosine, "COS", 1, "COS");
FSubclass(FunctionTangent, "TAN", 1, "TAN");
FSubclass(FunctionSine, "SIN", 1, "SIN");
FSubclass(FunctionArcCosine, "ARCCOS", 1, "ARCCOS");
FSubclass(FunctionArcSine, "ARCSIN", 1, "ARCSIN");
FSubclass(FunctionArcTangent, "ARCTAN", 1, "ARCTAN");
FSubclass(FunctionInterger, "INTEGER", 1, "INT");

FSubclass(FunctionMax, "MAX", 2, "MAX");
FSubclass(FunctionMin, "MIN", 2, "MIN");
FSubclass(FunctionZidz, "ZIDZ", 2, "SAFEDIV");
FSubclass(FunctionXidz, "XIDZ", 3, "SAFEDIV");
FSubclass(FunctionLookupInv, "LOOKUP INVERT", 2, "LOOKUPINV");
FSubclass(FunctionWithLookup, "WITH LOOKUP", 3, "WITH_LOOKUP");
FSubclass(FunctionSum, "SUM", 1, "SUM");
FSubclass(FunctionProd, "PROD", 1, "PROD");
FSubclass(FunctionVMax, "VMAX", 1, "MAX");
FSubclass(FunctionVMin, "VMIN", 1, "MIN");
FSubclass(FunctionVectorSelect, "VECTOR SELECT", 5, "VECTOR SELECT");
FSubclass(FunctionVectorElmMap, "VECTOR ELM MAP", 2, "VECTOR ELM MAP");
FSubclass(FunctionVectorSortOrder, "VECTOR SORT ORDER", 2, "VECTOR SORT ORDER");
FSubclass(FunctionGame, "GAME", 1, "");  // don't need this
FSubclass(FunctionRandom01, "RANDOM 0 1", 0, "UNIFORM(0,1)");
FSubclass(FunctionRandomUniform, "RANDOM UNIFORM", 3, "UNIFORM");
FSubclass(FunctionRandomPink, "RANDOM PINK NOISE", 4, "NORMALPINK");

FSubclass(FunctionNAN, "A FUNCTION OF", -1, "NAN");

// actually memory but no init - or init - does not matter for translation
DFSubclass(FunctionSmooth, "SMOOTH", 2, "SMTH1");
DFSubclass(FunctionSmoothI, "SMOOTHI", 3, "SMTH1");
DFSubclass(FunctionSmooth3, "SMOOTH3", 2, "SMTH3");
DFSubclass(FunctionSmooth3I, "SMOOTH3I", 3, "SMTH3");
DFSubclass(FunctionTrend, "TREND", 3, "TREND");
DFSubclass(FunctionFrcst, "FORECAST", 3, "FORCST");
DFSubclass(FunctionDelay1, "DELAY1", 2, "DELAY1");
DFSubclass(FunctionDelay1I, "DELAY1I", 3, "DELAY1");
DFSubclass(FunctionDelay3, "DELAY3", 2, "DELAY3");
DFSubclass(FunctionDelay3I, "DELAY3I", 3, "DELAY3");
DFSubclass(FunctionDelay, "DELAY FIXED", 3, "DELAY");
DFSubclass(FunctionNPV, "NPV", 4, "NPV");

// done as macros
FSubclass(FunctionDelayConveyor, "DELAY CONVEYOR", 6, "DELAY_CONVEYOR");
// - this one is fake - return NaN
FSubclass(FunctionVectorReorder, "VECTOR REORDER", 2, "VECTOR_REORDER");
class FunctionVectorLookup : public Function {
public:
  FunctionVectorLookup(SymbolNameSpace *sns) : Function(sns, "VECTOR LOOKUP", 5) {
  }
  ~FunctionVectorLookup(void) {
  }
  std::string ComputableName(void) {
    return "VECTOR LOOKUP";
  }
  virtual void OutputComputable(ContextInfo *info, ExpressionList *arg);

private:
};
FSubclass(FunctionElmCount, "ELMCOUNT", 1, "SIZE");
FSubclass(FunctionModulo, "MODULO", 2, "MODULO");
FSubclass(FunctionGetDataAtTime, "GET DATA AT TIME", 2, "GET_DATA_AT_TIME");
FSubclass(FunctionGetDataLastTime, "GET DATA LAST TIME", 1, "GET_DATA_LAST_TIME");
FSubclass(FunctionLookupArea, "LOOKUP AREA", 3, "LOOKUP_AREA");
FSubclass(FunctionLookupExtrapolate, "LOOKUP EXTRAPOLATE", 2, "LOOKUP");  // changes the graphical
FSubclassStart(FunctionTimeBase, "TIME BASE", 2, "TIME_BASE");
virtual void OutputComputable(ContextInfo *info, ExpressionList *arg);
}
;

FSubclassStart(FunctionDelayN, "DELAY N", 4, "DELAYN");

public:
virtual void OutputComputable(ContextInfo *info, ExpressionList *arg);
}
;
FSubclassStart(FunctionSmoothN, "SMOOTH N", 4, "SMTHN");

public:
virtual void OutputComputable(ContextInfo *info, ExpressionList *arg);
}
;

FSubclassMemory(FunctionInteg, "INTEG", 2, 0b10, 0b01, "integ_active", "integ_init")
    FSubclassMemoryStart(FunctionActiveInitial, "ACTIVE INITIAL", 2, 0b10, 0b01, "ai_active",
                         "ai_init") virtual bool IsActiveInit() override {
  return true;
}
}
;
FSubclass(FunctionInitial, "INITIAL", 1, "INIT");
FSubclass(FunctionReInitial, "REINITIAL", 1, "INIT");

FSubclassTime(FunctionRamp, "RAMP", 3, "RAMP");
FSubclass(FunctionLn, "LN", 1, "LN");
FSubclassTime(FunctionStep, "STEP", 2, "step");

FSubclassKeyword(FunctionTabbedArray, "TABBED ARRAY", 1);

// functions that will never translate - but easier to catch error on other side
FSubclass(FunctionGetDirectData, "GET DIRECT DATA", 4, "GET_DIRECT_DATA");
FSubclass(FunctionGetDataMean, "GET DATA MEAN", 3, "GET_DATA_MEAN");

/*
class FunctionMin :
   public Function
{
public :
   FunctionMin(SymbolNameSpace *sns) : Function(sal,"MIN",2) { ; }
   ~FunctionMin(void) { }
   inline double Eval(Expression *,ExpressionList *arg) ;
} ;

*/

class FunctionSampleIfTrue : public Function {
public:
  FunctionSampleIfTrue(SymbolNameSpace *sns) : Function(sns, "SAMPLE IF TRUE", 3) {
  }
  ~FunctionSampleIfTrue(void) {
  }
  std::string ComputableName(void) {
    return "SampleIfTrue";
  }
  virtual void OutputComputable(ContextInfo *info, ExpressionList *arg);

private:
};

class FunctionPulse : public Function {
public:
  FunctionPulse(SymbolNameSpace *sns) : Function(sns, "PULSE", 2) {
  }
  ~FunctionPulse(void) {
  }
  std::string ComputableName(void) {
    return "Pulse";
  }
  virtual void OutputComputable(ContextInfo *info, ExpressionList *arg);

private:
};
class FunctionPulseTrain : public Function {
public:
  FunctionPulseTrain(SymbolNameSpace *sns) : Function(sns, "PULSE TRAIN", 4) {
  }
  ~FunctionPulseTrain(void) {
  }
  std::string ComputableName(void) {
    return "PulseTrain";
  }
  virtual void OutputComputable(ContextInfo *info, ExpressionList *arg);

private:
};
class FunctionQuantum : public Function {
public:
  FunctionQuantum(SymbolNameSpace *sns) : Function(sns, "QUANTUM", 2) {
  }
  ~FunctionQuantum(void) {
  }
  std::string ComputableName(void) {
    return "Quantum";
  }
  virtual void OutputComputable(ContextInfo *info, ExpressionList *arg);

private:
};
class FunctionRandomNormal : public Function {
public:
  FunctionRandomNormal(SymbolNameSpace *sns) : Function(sns, "RANDOM NORMAL", 5) {
  }
  ~FunctionRandomNormal(void) {
  }
  std::string ComputableName(void) {
    return "RandomNormal";
  }
  virtual void OutputComputable(ContextInfo *info, ExpressionList *arg);

private:
};

class FunctionRandomPoisson : public Function {
public:
  FunctionRandomPoisson(SymbolNameSpace *sns) : Function(sns, "RANDOM POISSON", 6) {
  }
  ~FunctionRandomPoisson(void) {
  }
  std::string ComputableName(void) {
    return "RandomPoisson";
  }
  virtual void OutputComputable(ContextInfo *info, ExpressionList *arg);

private:
};

class FunctionIfThenElse : public Function {
public:
  FunctionIfThenElse(SymbolNameSpace *sns) : Function(sns, "IF THEN ELSE", 3) {
    ;
  }
  ~FunctionIfThenElse(void) {
  }
  std::string ComputableName(void) {
    return "IF";
  }
  virtual void OutputComputable(ContextInfo *info, ExpressionList *arg);

private:
};

class FunctionLog : public Function {
public:
  FunctionLog(SymbolNameSpace *sns) : Function(sns, "LOG", 2) {
    ;
  }
  ~FunctionLog(void) {
  }
  std::string ComputableName(void) {
    return "LOG10";
  }
  virtual void OutputComputable(ContextInfo *info, ExpressionList *arg);

private:
};
#endif
