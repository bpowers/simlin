/* Abstract class for expression - the different expression types
   are all tiny extensions and included in the same files */
#ifndef _XMUTIL_SYMBOL_EXPRESSION_H
#define _XMUTIL_SYMBOL_EXPRESSION_H
#include <float.h>
#include <math.h>

#include "../ContextInfo.h"
#include "../Function/Function.h"
#include "../Function/State.h"
#include "../Function/TableFunction.h"
#include "SymbolList.h"
#include "SymbolTableBase.h"
#include "Variable.h"

class ExpressionList;  // forward declaration
class Model;
class FlowList;
class ExpressionTable;

// probably don't need these after all
enum EXPTYPE {
  EXPTYPE_None,
  EXPTYPE_Variable,
  EXPTYPE_Symlist,
  EXPTYPE_Number,
  EXPTYPE_Literal,
  EXPTYPE_NumberTable,
  EXPTYPE_Function,
  EXPTYPE_FunctionMemory,
  EXPTYPE_Lookup,
  EXPTYPE_Table,
  EXPTYPE_Operator
};

class Expression : public SymbolTableBase {
public:
  Expression(SymbolNameSpace *sns);
  virtual ~Expression(void) = 0;
  virtual EXPTYPE GetType(void) {
    return EXPTYPE_None;
  }
  virtual double Eval(ContextInfo *info) = 0;
  virtual void FlipSign(void) {
  }
  virtual Function *GetFunction(void) {
    return NULL;
  }
  virtual const char *GetOperator() {
    return NULL;
  }
  virtual const char *GetBefore() {
    return NULL;
  }
  virtual ExpressionTable *GetTable() {
    return NULL;
  }
  virtual void CheckPlaceholderVars(Model *m, bool isfirst) = 0;  // generally do nothing, but big error to skip
  virtual bool CheckComputed(ContextInfo *info) {
    return true;
  }
  virtual void RemoveFunctionArgs(void) {
  }                                                      // only 1 subclass does anything
  virtual void OutputComputable(ContextInfo *info) = 0;  // again don't skip - todo modify this to make dumping
                                                         // equations easy - possibly returning std::string
  virtual bool IsActiveInit() {
    return false;
  }
  virtual bool TestMarkFlows(SymbolNameSpace *sns, FlowList *fl,
                             Equation *eq) = 0;  // but will also create a flow when the INTEG equation has other stuff
  virtual void GetVarsUsed(std::vector<Variable *> &vars) = 0;  // list of variables used
  virtual void MarkType(XMILE_Type type) = 0;                   // only called with flow after test returns true
  virtual Expression *GetArg(int pos) {
    return NULL;
  }
};

class ExpressionVariable : public Expression {
public:
  ExpressionVariable(SymbolNameSpace *sns, Variable *var, SymbolList *subs) : Expression(sns) {
    pVariable = var;
    pSubList = subs;
  }
  virtual ~ExpressionVariable(void) {
    if (HasGoodAlloc()) {
      if (pSubList)
        delete pSubList; /* leave pVariable alone */
    }
  }
  virtual EXPTYPE GetType(void) {
    return EXPTYPE_Variable;
  }
  virtual Variable *GetVariable(void) {
    return pVariable;
  }
  virtual SymbolList *GetSubs() {
    return pSubList;
  }
  void SetSubs(SymbolList *subs) {
    pSubList = subs;
  }
  virtual void CheckPlaceholderVars(Model *m, bool isfirst) {
  }
  bool CheckComputed(ContextInfo *info) {
    return pVariable->CheckComputed(info, false);
  }
  double Eval(ContextInfo *info) {
    return pVariable->Eval(info);
  }
  virtual void OutputComputable(ContextInfo *info) {
    assert(pVariable);
    pVariable->OutputComputable(info);
    if (pSubList)
      pSubList->OutputComputable(info);
  }
  virtual bool TestMarkFlows(SymbolNameSpace *sns, FlowList *fl, Equation *eq) {
    return false;
  }  // but will also create a flow when the INTEG equation has other stuff
  virtual void GetVarsUsed(std::vector<Variable *> &vars);  // list of variables used
  virtual void MarkType(XMILE_Type type) {
    pVariable->SetVariableType(type);
  }  // only called with flow after test returns true
private:
  Variable *pVariable;   // pointer back to the model variable - not allocated by this object
  SymbolList *pSubList;  // subscripts - allocated by this object
};

class ExpressionSymbolList : public Expression {
public:
  ExpressionSymbolList(SymbolNameSpace *sns, SymbolList *subs, SymbolList *map) : Expression(sns) {
    pSymList = subs;
    pMap = map;
  }
  virtual ~ExpressionSymbolList(void) {
    if (HasGoodAlloc()) {
      if (pSymList)
        delete pSymList;
      if (pMap)
        delete pMap;
    }
  }
  virtual EXPTYPE GetType(void) {
    return EXPTYPE_Symlist;
  }
  virtual void CheckPlaceholderVars(Model *m, bool isfirst) {
  }
  bool CheckComputed(ContextInfo *info) {
    return true;
  }
  double Eval(ContextInfo *info) {
    return -FLT_MAX;
  }
  virtual void OutputComputable(ContextInfo *info) {
    pSymList->OutputComputable(info);
  }
  virtual bool TestMarkFlows(SymbolNameSpace *sns, FlowList *fl, Equation *eq) {
    return false;
  }
  virtual void GetVarsUsed(std::vector<Variable *> &vars) {
  }  // list of variables used
  virtual void MarkType(XMILE_Type type) {
    assert(false);
  }
  SymbolList *SymList() const {
    return pSymList;
  }
  SymbolList *Map() const {
    return pMap;
  }

private:
  SymbolList *pSymList;  // subscript elements - should be deleted by this object
  SymbolList *pMap;      //
};

class ExpressionNumber : public Expression {
public:
  ExpressionNumber(SymbolNameSpace *sns, double num) : Expression(sns) {
    value = num;
  }
  virtual ~ExpressionNumber(void) {
  }
  virtual EXPTYPE GetType(void) {
    return EXPTYPE_Number;
  }
  void FlipSign(void) {
    value = -value;
  }
  virtual double Eval(ContextInfo *info) {
    return value;
  }
  virtual void CheckPlaceholderVars(Model *m, bool isfirst) {
  }
  virtual void OutputComputable(ContextInfo *info) {
    *info << value;
  }
  virtual bool TestMarkFlows(SymbolNameSpace *sns, FlowList *fl, Equation *eq) {
    return false;
  }
  virtual void GetVarsUsed(std::vector<Variable *> &vars) {
  }  // list of variables used
  virtual void MarkType(XMILE_Type type) {
  }

private:
  double value;
};
class ExpressionLiteral : public Expression {
public:
  ExpressionLiteral(SymbolNameSpace *sns, const char *str) : Expression(sns) {
    value = str;
  }
  virtual ~ExpressionLiteral(void) {
  }
  virtual EXPTYPE GetType(void) {
    return EXPTYPE_Literal;
  }
  virtual double Eval(ContextInfo *info) {
    return -1;
  }
  virtual void CheckPlaceholderVars(Model *m, bool isfirst) {
  }
  virtual void OutputComputable(ContextInfo *info) {
    *info << value;
  }
  virtual bool TestMarkFlows(SymbolNameSpace *sns, FlowList *fl, Equation *eq) {
    return false;
  }
  virtual void GetVarsUsed(std::vector<Variable *> &vars) {
  }  // list of variables used
  virtual void MarkType(XMILE_Type type) {
  }

private:
  std::string value;
};
class ExpressionNumberTable : public Expression {
public:
  ExpressionNumberTable(SymbolNameSpace *sns) : Expression(sns) {
  }
  virtual ~ExpressionNumberTable(void) {
  }
  virtual EXPTYPE GetType(void) {
    return EXPTYPE_NumberTable;
  }
  virtual double Eval(ContextInfo *info) {
    return -FLT_MAX;
  }
  virtual void CheckPlaceholderVars(Model *m, bool isfirst) {
  }
  virtual void OutputComputable(ContextInfo *info) {
    *info << " ??? ";
  }
  void AddValue(unsigned row, double num) {
    vVals.push_back(num);
  }  // if (row + 1 > vRow.size()) vRow.resize(row + 1); vRow[row].push_back(num); }
     // int Count(unsigned row) { if(row < vRow.size()) return 0 ; return vRow[row].size() ; }
  const std::vector<double> &GetVals() const {
    return vVals;
  }
  // typedef  std::vector<double> ColVals ;
  virtual bool TestMarkFlows(SymbolNameSpace *sns, FlowList *fl, Equation *eq) {
    return false;
  }
  virtual void GetVarsUsed(std::vector<Variable *> &vars) {
  }  // list of variables used
  virtual void MarkType(XMILE_Type type) {
  }

private:
  std::vector<double> vVals;
};

class ExpressionFunction : public Expression {
public:
  ExpressionFunction(SymbolNameSpace *sns, Function *f, ExpressionList *args) : Expression(sns) {
    pFunction = f;
    pArgs = args;
  }
  ~ExpressionFunction(void);
  virtual EXPTYPE GetType(void) override {
    return EXPTYPE_Function;
  }
  virtual double Eval(ContextInfo *info) override {
    return pFunction->Eval(this, pArgs, info);
  }
  virtual Function *GetFunction(void) override {
    return pFunction;
  }
  virtual ExpressionList *GetArgs(void) {
    return pArgs;
  }
  virtual void CheckPlaceholderVars(Model *m, bool isfirst) override;
  virtual bool CheckComputed(ContextInfo *info) override {
    return pFunction->CheckComputed(info, pArgs);
  }
  virtual void RemoveFunctionArgs(void) override {
    pArgs = NULL;
  }
  virtual void OutputComputable(ContextInfo *info) override {
    pFunction->OutputComputable(info, pArgs);
  }
  virtual bool IsActiveInit() override {
    return pFunction->IsActiveInit();
  }
  virtual bool TestMarkFlows(SymbolNameSpace *sns, FlowList *fl, Equation *eq) override {
    return false;
  }
  virtual void GetVarsUsed(std::vector<Variable *> &vars) override;  // list of variables used
  virtual void MarkType(XMILE_Type type) override {
  }

private:
  Function *pFunction;  // not allocated here
  ExpressionList *pArgs;
};
class ExpressionFunctionMemory : public ExpressionFunction {
public:
  ExpressionFunctionMemory(SymbolNameSpace *sns, Function *f, ExpressionList *args) : ExpressionFunction(sns, f, args) {
    pPlacholderEquation = NULL;
  }
  ~ExpressionFunctionMemory(void) {
  }
  virtual EXPTYPE GetType(void) {
    return EXPTYPE_FunctionMemory;
  }
  virtual double Eval(ContextInfo *info) {
    if (pPlacholderEquation)
      return pPlacholderEquation->GetVariable()->Eval(info);
    return ExpressionFunction::Eval(info);
  }
  void CheckPlaceholderVars(Model *m, bool isfirst);
  bool CheckComputed(ContextInfo *info) {
    if (pPlacholderEquation)
      return pPlacholderEquation->GetVariable()->CheckComputed(info, false);
    return ExpressionFunction::CheckComputed(info);
  }
  virtual void OutputComputable(ContextInfo *info) {
    if (pPlacholderEquation)
      return pPlacholderEquation->GetVariable()->OutputComputable(info);
    return ExpressionFunction::OutputComputable(info);
  }
  virtual bool TestMarkFlows(SymbolNameSpace *sns, FlowList *fl, Equation *eq);
  virtual void MarkType(XMILE_Type type) {
  }

private:
  Equation *pPlacholderEquation;  // used in computation (null if function defines LHS)
};

class ExpressionLookup : public Expression {
public:
  ExpressionLookup(SymbolNameSpace *sns, ExpressionVariable *var, Expression *e) : Expression(sns) {
    pExpressionVariable = var;
    pExpression = e;
    pExpressionTable = NULL;
  }
  ExpressionLookup(SymbolNameSpace *sns, Expression *e, ExpressionTable *tbl) : Expression(sns) {
    pExpressionVariable = NULL;
    pExpression = e;
    pExpressionTable = tbl;
  }
  ~ExpressionLookup(void) {
    if (HasGoodAlloc()) {
      delete pExpressionVariable;
      delete pExpression;
    }
  }
  virtual EXPTYPE GetType(void) {
    return EXPTYPE_Lookup;
  }
  virtual ExpressionTable *GetTable(void) {
    return pExpressionTable;
  }
  void CheckPlaceholderVars(Model *m, bool isfirst) {
    pExpression->CheckPlaceholderVars(m, false);
  }
  bool CheckComputed(ContextInfo *info) {
    return pExpression->CheckComputed(info);
  }
  double Eval(ContextInfo *info) {
    return TableFunction::Eval(pExpressionVariable, pExpression, info);
  }
  virtual void OutputComputable(ContextInfo *info);
  virtual bool TestMarkFlows(SymbolNameSpace *sns, FlowList *fl, Equation *eq) {
    return false;
  }
  virtual void GetVarsUsed(std::vector<Variable *> &vars) {
    if (pExpressionVariable)
      pExpressionVariable->GetVarsUsed(vars);
    if (pExpression)
      pExpression->GetVarsUsed(vars);
  }  // list of variables used
  virtual void MarkType(XMILE_Type type) {
  }

private:
  ExpressionVariable *pExpressionVariable;  // null for with_lookup
  Expression *pExpression;
  ExpressionTable *pExpressionTable;
};

class ExpressionTable : public Expression {
public:
  ExpressionTable(SymbolNameSpace *sns) : Expression(sns) {
    bHasRange = false;
    bExtrapolate = false;
  }
  ~ExpressionTable(void) { /* vector destructors only*/
  }
  virtual EXPTYPE GetType(void) {
    return EXPTYPE_Table;
  }
  virtual ExpressionTable *GetTable(void) {
    return this;
  }
  void AddPair(double x, double y) {
    vXVals.push_back(x);
    vYVals.push_back(y);
  }
  void AddRange(double x1, double y1, double x2, double y2) {
    bHasRange = true;
    dX2 = x2;
    dY1 = y1;
    dX2 = x2;
    dY2 = y2;
  }
  virtual void CheckPlaceholderVars(Model *m, bool isfirst) {
  }
  double Eval(ContextInfo *info) {
    assert(0);
    return FLT_MAX;
  }
  void TransformLegacy();
  std::vector<double> *GetXVals(void) {
    return &vXVals;
  }
  std::vector<double> *GetYVals(void) {
    return &vYVals;
  }
  virtual void OutputComputable(ContextInfo *info) {
    *info << "0+0";
  }  // unattached table give it a 0+0 equation - points dealt with separately
  virtual bool TestMarkFlows(SymbolNameSpace *sns, FlowList *fl, Equation *eq) {
    return false;
  }
  virtual void GetVarsUsed(std::vector<Variable *> &vars) {
  }  // list of variables used
  virtual void MarkType(XMILE_Type type) {
  }
  bool Extrapolate() {
    return bExtrapolate;
  }
  void SetExtrapolate(bool set) {
    bExtrapolate = set;
  }

private:
  std::vector<double> vXVals;
  std::vector<double> vYVals;
  double dY1, dX2, dY2;
  bool bHasRange;
  bool bExtrapolate;
};

class ExpressionOperator2 : public Expression {
public:
  ExpressionOperator2(SymbolNameSpace *sns, Expression *e1, Expression *e2) : Expression(sns) {
    pE1 = e1;
    pE2 = e2;
  }
  ~ExpressionOperator2(void) {
    if (HasGoodAlloc()) {
      if (pE1)
        delete pE1;
      if (pE2)
        delete pE2;
    }
  }
  virtual EXPTYPE GetType(void) {
    return EXPTYPE_Operator;
  }
  void CheckPlaceholderVars(Model *m, bool isfirst) {
    if (pE1)
      pE1->CheckPlaceholderVars(m, false);
    if (pE2)
      pE2->CheckPlaceholderVars(m, false);
  }
  bool CheckComputed(ContextInfo *info) {
    if (pE1 && !pE1->CheckComputed(info))
      return false;
    if (pE2 && !pE2->CheckComputed(info))
      return false;
    return true;
  }
  virtual void OutputComputable(ContextInfo *info) {
  }
  virtual bool TestMarkFlows(SymbolNameSpace *sns, FlowList *fl, Equation *eq) {
    if (pE1 && pE1->TestMarkFlows(sns, fl, eq)) {
      return true;
    }
    if (pE2) {
      return pE2->TestMarkFlows(sns, fl, eq);
    }
    return false;
  }
  virtual void GetVarsUsed(std::vector<Variable *> &vars) {
    if (pE1)
      pE1->GetVarsUsed(vars);
    if (pE2)
      pE2->GetVarsUsed(vars);
  }  // list of variables used
  virtual void MarkType(XMILE_Type type) {
    if (pE1)
      pE1->MarkType(type);
    if (pE2)
      pE2->MarkType(type);
  }
  virtual Expression *GetArg(int pos) {
    return pos == 0 ? pE1 : pos == 1 ? pE2 : NULL;
  }

protected:
  Expression *pE1;
  Expression *pE2;
};

#define EO2SubClassRaw(name, evaleq, before, middle, after)                                         \
  class name : public ExpressionOperator2 {                                                         \
  public:                                                                                           \
    name(SymbolNameSpace *sns, Expression *e1, Expression *e2) : ExpressionOperator2(sns, e1, e2) { \
    }                                                                                               \
    ~name(void) {                                                                                   \
    }                                                                                               \
    virtual double Eval(ContextInfo *info) {                                                        \
      return (evaleq);                                                                              \
    }                                                                                               \
    virtual const char *GetOperator() {                                                             \
      return middle;                                                                                \
    }                                                                                               \
    virtual const char *GetBefore() {                                                               \
      return before;                                                                                \
    }                                                                                               \
    virtual void OutputComputable(ContextInfo *info) {                                              \
      *info << before;                                                                              \
      if (pE1)                                                                                      \
        pE1->OutputComputable(info);                                                                \
      *info << middle;                                                                              \
      if (pE2)                                                                                      \
        pE2->OutputComputable(info);                                                                \
      *info << after;                                                                               \
    }                                                                                               \
  };
#define EO2SubClass(name, evaleq, compsym) EO2SubClassRaw(name, evaleq, "", compsym, "");

EO2SubClass(ExpressionMultiply, pE1->Eval(info) * pE2->Eval(info), "*")
    EO2SubClass(ExpressionDivide, pE1->Eval(info) / pE2->Eval(info), "/")
        EO2SubClass(ExpressionAdd, pE1->Eval(info) + pE2->Eval(info), "+")
            EO2SubClass(ExpressionSubtract, pE1->Eval(info) - pE2->Eval(info), "-")
                EO2SubClass(ExpressionPower, exp(log(pE1->Eval(info)) * pE2->Eval(info)), "^")
                    EO2SubClassRaw(ExpressionParen, pE1->Eval(info), "(", "", ")")
                        EO2SubClassRaw(ExpressionUnaryMinus, (-pE1->Eval(info)), "-", "", "")

                            class ExpressionLogical : public Expression {
public:
  ExpressionLogical(SymbolNameSpace *sns, Expression *exp1, Expression *exp2, int oper) : Expression(sns) {
    pE1 = exp1;
    pE2 = exp2;
    mOper = oper;
  }
  ~ExpressionLogical(void) {
    if (HasGoodAlloc()) {
      delete pE1;
      delete pE2;
    }
  }
  virtual double Eval(ContextInfo *info) {
    return 0;
  }
  void CheckPlaceholderVars(Model *m, bool isfirst) {
    if (pE1)
      pE1->CheckPlaceholderVars(m, false);
    if (pE2)
      pE2->CheckPlaceholderVars(m, false);
  }
  void OutputComputable(ContextInfo *info);
  virtual bool TestMarkFlows(SymbolNameSpace *sns, FlowList *fl, Equation *eq) {
    if (pE1 && pE1->TestMarkFlows(sns, fl, eq)) {
      return true;
    }
    if (pE2) {
      return pE2->TestMarkFlows(sns, fl, eq);
    }
    return false;
  }
  virtual void GetVarsUsed(std::vector<Variable *> &vars) {
    if (pE1)
      pE1->GetVarsUsed(vars);
    if (pE2)
      pE2->GetVarsUsed(vars);
  }  // list of variables used
  virtual void MarkType(XMILE_Type type) {
    if (pE1)
      pE1->MarkType(type);
    if (pE2)
      pE2->MarkType(type);
  }

private:
  Expression *pE1;
  Expression *pE2;
  int mOper;
};

/* helper class for grabbing flows from an INTEG eqution */
class FlowList {
public:
  FlowList() {
    bValid = true;
    pActiveExpression = NULL;
    pNewVariable = NULL;
  }
  bool Valid() {
    return bValid;
  }
  void SetValid(bool set) {
    bValid = set;
  }
  bool Empty() {
    return vInflows.empty() && vOutflows.empty();
  }
  std::vector<Variable *> &Inflows() {
    return vInflows;
  }
  std::vector<Variable *> &Outflows() {
    return vOutflows;
  }
  void AddInflow(Variable *in);
  void AddOutflow(Variable *out);
  void SetActiveExpression(Expression *expression) {
    pActiveExpression = expression;
  }
  Expression *ActiveExpression() {
    return pActiveExpression;
  }
  void SetNewVariable(Variable *var) {
    pNewVariable = var;
  }
  Variable *NewVariable() {
    return pNewVariable;
  }
  bool operator==(const FlowList &rhs);

private:
  std::vector<Variable *> vInflows;
  std::vector<Variable *> vOutflows;
  Expression *pActiveExpression;
  Variable *pNewVariable;
  bool bValid;
};

/*
{

class ExpressionMultiply :
   public Expression
{
public :
   ExpressionMultiply(SymbolNameSpace *sns,Expression *exp1,Expression *exp2) : Expression(sal) {pE1 = exp1;pE2=exp2;}
   ~ExpressionMultiply(void) {if(HasGoodAlloc()){delete pE1;delete pE2;}}
   virtual double Eval(void) { return pE1->Eval() * pE2->Eval() ; }
private :
   Expression *pE1 ;
   Expression *pE2 ;
} ;

*/

#endif
