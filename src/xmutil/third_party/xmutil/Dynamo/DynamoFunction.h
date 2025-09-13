#ifndef _XMUTIL_DYNAMO_FUNCTION_H
#define _XMUTIL_DYNAMO_FUNCTION_H
/* function declarations for dynamo - should also probably split off the Vensim ones*/
#include "../Function/Function.h"

class DFunctionTable : public Function {
public:
  DFunctionTable(SymbolNameSpace *sns) : Function(sns, "TABLE", 5) {
  }
  ~DFunctionTable(void) {
  }
  virtual void OutputComputable(ContextInfo *info, ExpressionList *arg);
  virtual bool IsTableCall() const override {
    return true;
  }
  bool SetTableXAxis(ExpressionList *args) const;

private:
};

FSubclassMemoryStart(DFunctionInteg, "INTEGRATE", 2, 0b10, 0b01, "integ_active",
                     "integ_init") virtual bool IsIntegrator() const override {
  return true;
}
}
;

FSubclass(DFunctionAbs, "ABS", 1, "ABS");
FSubclass(DFunctionExp, "EXP", 1, "EXP");
FSubclass(DFunctionSqrt, "SQRT", 1, "SQRT");

FSubclass(DFunctionCosine, "COS", 1, "COS");
FSubclass(DFunctionTangent, "TAN", 1, "TAN");
FSubclass(DFunctionSine, "SIN", 1, "SIN");
FSubclass(DFunctionArcCosine, "ARCCOS", 1, "ARCCOS");
FSubclass(DFunctionArcSine, "ARCSIN", 1, "ARCSIN");
FSubclass(DFunctionArcTangent, "ARCTAN", 1, "ARCTAN");
FSubclass(DFunctionInterger, "INTEGER", 1, "INT");

FSubclass(DFunctionMax, "MAX", 2, "MAX");
FSubclass(DFunctionMin, "MIN", 2, "MIN");
FSubclass(DFunctionZidz, "FIFZE", 2, "SAFEDIV");

FSubclass(DFunctionXidz, "XIDZ", 3, "SAFEDIV");

FSubclass(DFunctionSum, "SUM", 1, "SUM");
FSubclass(DFunctionProd, "PROD", 1, "PROD");
FSubclass(DFunctionVMax, "VMAX", 1, "MAX");
FSubclass(DFunctionVMin, "VMIN", 1, "MIN");

FSubclass(DFunctionRandom01, "RANDOM 0 1", 0, "UNIFORM(0,1)");
FSubclass(DFunctionRandomUniform, "RANDOM UNIFORM", 3, "UNIFORM");
FSubclass(DFunctionRandomPink, "RANDOM PINK NOISE", 4, "NORMALPINK");

// actually memory but no init - or init - does not matter for translation
DFSubclass(DFunctionSmooth, "SMOOTH", 2, "SMTH1");
DFSubclass(DFunctionSmoothI, "SMOOTHI", 3, "SMTH1");
DFSubclass(DFunctionSmooth3, "SMOOTH3", 2, "SMTH3");
DFSubclass(DFunctionSmooth3I, "SMOOTH3I", 3, "SMTH3");
DFSubclass(DFunctionTrend, "TREND", 3, "TREND");
DFSubclass(DFunctionFrcst, "FORECAST", 3, "FORCST");
DFSubclass(DFunctionDelay1, "DELAY1", 2, "DELAY1");
DFSubclass(DFunctionDelay1I, "DELAY1I", 3, "DELAY1");
DFSubclass(DFunctionDelay3, "DELAY3", 2, "DELAY3");
DFSubclass(DFunctionDelay3I, "DELAY3I", 3, "DELAY3");
DFSubclass(DFunctionDelay, "DELAY FIXED", 3, "DELAY");
DFSubclass(DFunctionNPV, "NPV", 4, "NPV");

FSubclass(DFunctionModulo, "MODULO", 2, "MODULO");

FSubclassStart(DFunctionDelayN, "DELAY N", 4, "DELAYN") public
    : virtual void OutputComputable(ContextInfo *info, ExpressionList *arg);
}
;
FSubclassStart(DFunctionSmoothN, "SMOOTH N", 4, "SMTHN") public
    : virtual void OutputComputable(ContextInfo *info, ExpressionList *arg);
}
;

FSubclassMemoryStart(DFunctionActiveInitial, "ACTIVE INITIAL", 2, 0b10, 0b01, "ai_active",
                     "ai_init") virtual bool IsActiveInit() override {
  return true;
}
}
;
FSubclass(DFunctionInitial, "INITIAL", 1, "INIT");
FSubclass(DFunctionReInitial, "REINITIAL", 1, "INIT");

FSubclassTime(DFunctionRamp, "RAMP", 3, "RAMP");
FSubclass(DFunctionLn, "LN", 1, "LN");
FSubclassTime(DFunctionStep, "STEP", 2, "step");

FSubclassKeyword(DFunctionTabbedArray, "TABBED ARRAY", 1);

// functions that will never translate - but easier to catch error on other side
FSubclass(DFunctionGetDirectData, "GET DIRECT DATA", 4, "GET_DIRECT_DATA");
FSubclass(DFunctionGetDataMean, "GET DATA MEAN", 3, "GET_DATA_MEAN");

class DFunctionPulse : public Function {
public:
  DFunctionPulse(SymbolNameSpace *sns) : Function(sns, "PULSE", 2) {
  }
  ~DFunctionPulse(void) {
  }
  std::string ComputableName(void) {
    return "Pulse";
  }
  virtual void OutputComputable(ContextInfo *info, ExpressionList *arg);

private:
};

class DFunctionIfThenElse : public Function {
public:
  DFunctionIfThenElse(SymbolNameSpace *sns) : Function(sns, "IF THEN ELSE", 3) {
    ;
  }
  ~DFunctionIfThenElse(void) {
  }
  std::string ComputableName(void) {
    return "IF";
  }
  virtual void OutputComputable(ContextInfo *info, ExpressionList *arg);

private:
};

#endif
