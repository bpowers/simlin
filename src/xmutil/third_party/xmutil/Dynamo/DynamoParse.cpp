// DynamoParse.cpp : Read an mdl file into an XModel object
// we use an in-memory string to simplify look ahead/back
// we include the tokenizer here because it is as easy as setting
// up regular expressions for Flex and more easily understood

#include "DynamoParse.h"

#include <cstring>

#include "../Symbol/ExpressionList.h"
#include "../Symbol/LeftHandSide.h"
#include "../Symbol/Variable.h"
#include "DynamoFunction.h"
#define YYSTYPE DynamoParse
#include "../Model.h"
#include "../XMUtil.h"
#include "DYacc.tab.hpp"

DynamoParse *DPObject = NULL;
DynamoParse::DynamoParse(Model *model) {
  assert(!DPObject);
  DPObject = this;
  _model = model;
  _model->set_from_dynamo(true);
  pSymbolNameSpace = model->GetNameSpace();
  bLongName = true;
  bInMacro = false;
  ReadyFunctions();
}
DynamoParse::~DynamoParse(void) {
  DPObject = NULL;
}

void DynamoParse::ReadyFunctions() {
  // initialize functions - use only D functions
  try {
    new DFunctionTable(pSymbolNameSpace);

    new DFunctionMin(pSymbolNameSpace);
    new DFunctionMax(pSymbolNameSpace);
    new DFunctionInteg(pSymbolNameSpace);
    new DFunctionActiveInitial(pSymbolNameSpace);
    new DFunctionInitial(pSymbolNameSpace);
    new DFunctionReInitial(pSymbolNameSpace);
    new DFunctionPulse(pSymbolNameSpace);
    new DFunctionIfThenElse(pSymbolNameSpace);
    new DFunctionZidz(pSymbolNameSpace);
    new DFunctionXidz(pSymbolNameSpace);
    new DFunctionStep(pSymbolNameSpace);
    new DFunctionTabbedArray(pSymbolNameSpace);
    new DFunctionRamp(pSymbolNameSpace);
    new DFunctionLn(pSymbolNameSpace);
    new DFunctionSmooth(pSymbolNameSpace);
    new DFunctionSmoothI(pSymbolNameSpace);
    new DFunctionSmooth3(pSymbolNameSpace);
    new DFunctionSmooth3I(pSymbolNameSpace);
    new DFunctionTrend(pSymbolNameSpace);
    new DFunctionFrcst(pSymbolNameSpace);
    new DFunctionDelay1(pSymbolNameSpace);
    new DFunctionDelay1I(pSymbolNameSpace);
    new DFunctionDelay3(pSymbolNameSpace);
    new DFunctionDelay3I(pSymbolNameSpace);
    new DFunctionDelay(pSymbolNameSpace);
    new DFunctionDelayN(pSymbolNameSpace);
    new DFunctionSmoothN(pSymbolNameSpace);
    new DFunctionModulo(pSymbolNameSpace);
    new DFunctionNPV(pSymbolNameSpace);
    new DFunctionSum(pSymbolNameSpace);
    new DFunctionProd(pSymbolNameSpace);
    new DFunctionVMax(pSymbolNameSpace);
    new DFunctionVMin(pSymbolNameSpace);
    new DFunctionRandom01(pSymbolNameSpace);
    new DFunctionRandomUniform(pSymbolNameSpace);
    new DFunctionRandomPink(pSymbolNameSpace);
    new DFunctionAbs(pSymbolNameSpace);
    new DFunctionExp(pSymbolNameSpace);
    new DFunctionSqrt(pSymbolNameSpace);

    new DFunctionCosine(pSymbolNameSpace);
    new DFunctionSine(pSymbolNameSpace);
    new DFunctionTangent(pSymbolNameSpace);
    new DFunctionArcCosine(pSymbolNameSpace);
    new DFunctionArcSine(pSymbolNameSpace);
    new DFunctionArcTangent(pSymbolNameSpace);
    new DFunctionInterger(pSymbolNameSpace);

    new DFunctionGetDirectData(pSymbolNameSpace);
    new DFunctionGetDataMean(pSymbolNameSpace);

    pSymbolNameSpace->ConfirmAllAllocations();
  } catch (...) {
    log("Failed to initialize symbol table");
  }
}
Equation *DynamoParse::AddEq(LeftHandSide *lhs, Expression *ex, ExpressionList *exl, int tok) {
  if (exl) {
    if (exl->Length() == 1) {
      ex = exl->GetExp(0);
      delete exl;
    } else { /* only a list of numbers is valid here - throw exception for anything else */
      ExpressionNumberTable *ent = new ExpressionNumberTable(pSymbolNameSpace);
      int n = exl->Length();
      int i;
      for (i = 0; i < n; i++) {
        ex = exl->GetExp(i);
        if (ex->GetType() == EXPTYPE_Operator && ex->GetArg(0) == NULL && *ex->GetOperator() == '-') {
          // alloe unary minus here
          ent->AddValue(0, -ex->GetArg(1)->Eval(NULL));  // note eval does not need context for number
        } else if (ex->GetType() != EXPTYPE_Number) {
          has_error_ = true;
          last_error_ = "Expecting only comma delimited numbers ";
          delete ex;  // Delete the current expression that caused the error
          delete ent;
          delete exl;
          return nullptr;
        } else
          ent->AddValue(0, ex->Eval(NULL));  // note eval does not need context for number
        delete ex;
      }
      delete exl;
      ex = ent;
    }
  }

  return new Equation(pSymbolNameSpace, lhs, ex, tok);
}
Equation *DynamoParse::AddStockEq(LeftHandSide *lhs, ExpressionVariable *stock, ExpressionList *exl, int tok) {
  if (stock) {
    // needs to match lhs or we throu an exception
    if (lhs->GetVariable() != stock->GetVariable()) {
      has_error_ = true;
      last_error_ = "Level equations must be stock=stock+flow in form ";
      return nullptr;
    }
  }
  Expression *ex = NULL;
  if (exl) {
    if (exl->Length() == 1) {
      ex = exl->GetExp(0);
      delete exl;
    }
  }
  if (ex == NULL) {
    has_error_ = true;
    last_error_ = "Bad level equation ";
    return nullptr;
  }
  // wrap the expression/DT in INTEGRATE - the DT will probably just cancel another DT - so could be reduced
  ex = DPObject->OperatorExpression('(', ex, NULL);
  Variable *var = this->InsertVariable("DT");
  if (!var) {
    delete ex;
    has_error_ = true;
    last_error_ = "Failed to insert DT variable";
    return nullptr;
  }
  Expression *ex2 = DPObject->VarExpression(var, NULL);
  ex = DPObject->OperatorExpression('/', ex, ex2);
  ExpressionList *args = DPObject->ChainExpressionList(NULL, ex);
  var = DPObject->InsertVariable("INTEGRATE");
  if (!var) {
    delete args;
    has_error_ = true;
    last_error_ = "Failed to insert INTEGRATE variable";
    return nullptr;
  }
  ex = DPObject->FunctionExpression(static_cast<Function *>(static_cast<Symbol *>(var)), args);

  return new Equation(pSymbolNameSpace, lhs, ex, DPTT_dt_to_one);
}

Equation *DynamoParse::AddTable(LeftHandSide *lhs, Expression *ex, ExpressionTable *tbl, bool legacy) {
  if (!tbl) {
    // this is an exogenous data entry - treat as a table on time and give the table value 1 at all times if we have
    // that info
    tbl = new ExpressionTable(this->pSymbolNameSpace);
    tbl->AddPair(0, 1);
    tbl->AddPair(1, 1);
    Variable *var = this->FindVariable("TIME");
    if (!var)
      var = new Variable(this->pSymbolNameSpace, "TIME");
    ex = new ExpressionVariable(this->pSymbolNameSpace, var, NULL);
  }
  if (legacy)
    tbl->TransformLegacy();
  if (!ex)
    return new Equation(pSymbolNameSpace, lhs, tbl, '(');
  // with lookup is just a table embedded in a variable - actually the norm for XMILE
  // Function* wl = static_cast<Function*>(pSymbolNameSpace->Find("WITH LOOKUP"));
  Expression *rhs = new ExpressionLookup(pSymbolNameSpace, ex, tbl);
  return new Equation(pSymbolNameSpace, lhs, rhs, '=');
}

/* a full eq cleans up the temporary memory space and
  also assigns the equation to the Variable  - not that inside
  of macros there is a separate symbol space this is going
  against */
void DynamoParse::AddFullEq(Equation *eq, int type) {
  pSymbolNameSpace->ConfirmAllAllocations();  // now independently allocated
  pActiveVar = eq->GetVariable();
  if (!_model->Groups().empty() && pActiveVar->GetAllEquations().empty() && !bInMacro) {
    _model->Groups().back()->vVariables.push_back(pActiveVar);
    pActiveVar->SetGroup(_model->Groups().back());
  }
  if (type == DPTT_level) {
    pActiveVar->AddEq(eq, false);
  } else if (type == DPTT_init)
    pActiveVar->AddEq(eq, true);
  else
    pActiveVar->AddEq(eq, false);
}

int DynamoParse::yyerror(const char *str) {
  // Set error state instead of throwing exception
  has_error_ = true;
  last_error_ = str;
  // Return 1 to signal error to the parser
  return 1;
}

static std::string compress_whitespace(const std::string &s) {
  std::string rval;
  const char *tv = s.c_str();
  for (; *tv; tv++) {
    if (*tv != ' ' && *tv != '\t' && *tv != '\n' && *tv != '\r' && !isdigit(*tv))
      break;
  }
  for (; *tv; tv++) {
    if (*tv == ' ' || *tv == '\t' || *tv == '\n' || *tv == '\r') {
      rval.push_back('_');
      for (; tv[1]; tv++) {
        if (tv[1] != ' ' && tv[1] != '\t' && tv[1] != '\n' && tv[1] != '\r')
          break;
      }
    } else if ((*tv >= 'A' && *tv <= 'Z') || (*tv >= 'a' && *tv <= 'z') || isdigit(*tv))
      rval.push_back(*tv);  // otherwise ignore
    else
      break;  // start skipping at any special character
  }
  while (rval.back() == '_')
    rval.pop_back();
  return rval;
}

bool DynamoParse::ProcessFile(const std::string &filename, const char *contents, size_t contentsLen) {
  sFilename = filename;

  // Clear any previous error state
  ClearError();

  if (true) {
    bool noerr = true;
    mDynamoLex.Initialize(contents, contentsLen);
    // now we call the bison built parser which will call back to DynamoLex
    // for the tokenizing -
    int rval;
    while (true) {
      rval = 0;
      try {
        mDynamoLex.GetReady();
        rval = dpyyparse();

        // Check if an error occurred during parsing
        if (has_error_) {
          log("%s\n", last_error_.c_str());
          log("Error at line %d position %d in file %s\n", mDynamoLex.LineNumber(), mDynamoLex.Position(),
              sFilename.c_str());
          log(".... skipping the associated variable and looking for the next usable content.\n");
          pSymbolNameSpace->DeleteAllUnconfirmedAllocations();
          noerr = false;
          ClearError();  // Clear for next iteration
          if (!FindNextEq(false))
            break;
          continue;
        }
        if (rval == DPTT_eoq) {  // some combination of comment and units probably follows
          if (!FindNextEq(true))
            break;
        } else if (rval == DPTT_groupstar) {
          size_t depth = 0;
          std::string *cur = mDynamoLex.CurToken();
          while (depth < cur->size() && cur->at(depth) == '*')
            depth++;
          std::string name = cur->substr(depth);
          ModelGroup *group_owner = NULL;
          for (std::vector<ModelGroup *>::const_iterator it = _model->Groups().end();
               it-- != _model->Groups().begin();) {
            if ((*it)->iDepth < static_cast<int>(depth)) {
              group_owner = (*it);
              break;
            }
          }
          _model->Groups().push_back(new ModelGroup(name, group_owner, depth));
        } else if (rval == DPTT_specs) {
          ParseSpecs();
        } else if (rval == DPTT_save) {
          ParseSave();
        } else {
          log("Unknown terminal token %d\n", rval);
          if (!FindNextEq(false))
            break;
        }

      } catch (DynamoParseSyntaxError &e) {
        log("%s\n", e.str.c_str());
        log("Error at line %d position %d in file %s\n", mDynamoLex.LineNumber(), mDynamoLex.Position(),
            sFilename.c_str());
        log(".... skipping the associated variable and looking for the next usable content.\n");
        pSymbolNameSpace->DeleteAllUnconfirmedAllocations();
        noerr = false;
        if (!FindNextEq(false))
          break;

      } catch (...) {
        // Log unknown exceptions to aid diagnosis and ensure visibility via C API
        log("Unknown exception while parsing (Dynamo) at line %d position %d in file %s\n", mDynamoLex.LineNumber(),
            mDynamoLex.Position(), sFilename.c_str());
        log(".... skipping the associated variable and looking for the next usable content.\n");
        pSymbolNameSpace->DeleteAllUnconfirmedAllocations();
        noerr = false;
        if (!FindNextEq(false))
          break;
      }
    }
    _model->SetMacroFunctions(mMacroFunctions);

    if (bLongName) {
      // try to replace variable names with long names from the documentaion
      std::vector<Variable *> vars = _model->GetVariables(NULL);  // all symbols that are variables
      for (Variable *var : vars) {
        std::string alt = compress_whitespace(var->Comment());
        if (!alt.empty() && alt.size() < 80 && pSymbolNameSpace->Rename(var, alt)) {
          var->SetAlternateName(alt);
        }
      }
    }
    if (!noerr) {
      log("warning: writing output file, but we had errors. check the result carefully.\n");
    }
    return true;  // got something - try to put something out
  } else
    return false;
}

char *DynamoParse::GetIntChar(char *s, int &val, char c) {
  char *tv;
  for (tv = s; *tv; tv++) {
    if (*tv == c) {
      *tv++ = '\0';
      break;
    }
  }
  val = atoi(s);
  return tv;
}
char *DynamoParse::GetInt(char *s, int &val) {
  char *tv;
  for (tv = s; *tv; tv++) {
    if (*tv == ',') {
      *tv++ = '\0';
      break;
    }
  }
  val = atoi(s);
  return tv;
}
char *DynamoParse::GetString(char *s, std::string &name) {
  char *tv = s;
  if (*tv == '\"') {
    for (tv++; *tv; tv++) {
      if (*tv == '\"') {
        tv++;
        assert(*tv == ',');
        *tv++ = '\0';
        break;
      } else if (*tv == '\\' && tv[1] == '\"')
        tv++;
    }
  } else {
    for (tv = s; *tv; tv++) {
      if (*tv == ',') {
        *tv++ = '\0';
        break;
      }
    }
  }
  name = s;
  return tv;
}

Variable *DynamoParse::FindVariable(const std::string &name) {
  Variable *var = static_cast<Variable *>(pSymbolNameSpace->Find(name));
  if (var && var->isType() == Symtype_Variable)
    return var;
  return NULL;
}

Variable *DynamoParse::InsertVariable(const std::string &name) {
  Variable *var = static_cast<Variable *>(pSymbolNameSpace->Find(name));
  if (var && var->isType() != Symtype_Variable && var->isType() != Symtype_Function) {
    has_error_ = true;
    last_error_ = "Type meaning mismatch for " + name;
    return nullptr;
  }
  if (!var) {
    var = new Variable(pSymbolNameSpace, name);
    // this will insert it into the name space for hash lookup as well
  }
  return var;
}
Units *DynamoParse::InsertUnits(const std::string &name) {
  std::string uname = ">" + name;  // an illegal variable name since we allow the same names to be used for vars and
                                   // units - could use a separate namespace
  Units *u = static_cast<Units *>(pSymbolNameSpace->Find(uname));
  if (u && u->isType() != Symtype_Units) {
    has_error_ = true;
    last_error_ = "Type meaning mismatch for " + name;
    return nullptr;
  }
  if (!u) {
    u = new Units(pSymbolNameSpace, uname);
  }
  return u;
}

UnitExpression *DynamoParse::InsertUnitExpression(Units *u) {
  UnitExpression *uni = new UnitExpression(pSymbolNameSpace, u);
  return uni;
}

// find the beginning of the next equation - for error recovery
bool DynamoParse::FindNextEq(bool want_comment) {
  if (want_comment && this->pActiveVar) {
    std::string units;
    std::string comment = mDynamoLex.GetComment(units);
    if (!comment.empty())  // multile appearances okay - take last non empty
      this->pActiveVar->SetComment(comment);
    if (!units.empty())
      this->pActiveVar->SetUnitsString(units);
  } else {
    // consume the rest of the current line then try again
    mDynamoLex.ConsumeCurrentLine();
  }
  // just zip through to the first | then whatever follows is it
  return mDynamoLex.FindStartToken();
}

LeftHandSide *DynamoParse::AddExceptInterp(ExpressionVariable *var, SymbolListList *except, int interpmode) {
  return new LeftHandSide(pSymbolNameSpace, var, NULL, except, interpmode);
}
SymbolList *DynamoParse::SymList(SymbolList *in, Variable *add, bool bang, Variable *end) {
  SymbolList *sl;
  if (in)
    sl = in->Append(add, bang);
  else
    sl = new SymbolList(pSymbolNameSpace, add, bang);
  if (end) { /* actually shortcut for (axx-ayy) */
    int i, j;
    Variable *v;
    std::string start = add->GetName();
    std::string finish = end->GetName();
    for (i = start.length(); --i > 0;) {
      if (start[i - 1] < '0' || start[i - 1] > '9')
        break;
    }
    for (j = finish.length(); --j > 0;) {
      if (finish[j - 1] < '0' || finish[j - 1] > '9')
        break;
    }
    int low = atoi(start.c_str() + i);
    int high = atoi(finish.c_str() + j);
    if (i != j || start.compare(0, j, finish, 0, j) || low >= high) {
      has_error_ = true;
      last_error_ = "Bad subscript range specification";
      delete sl;
      return nullptr;
    }
    start.erase(i, std::string::npos);
    for (i = low + 1; i < high; i++) {
      finish = start + std::to_string(i);
      v = static_cast<Variable *>(pSymbolNameSpace->Find(finish));
      if (!v)
        v = new Variable(pSymbolNameSpace, finish);
      sl->Append(v, bang);
    }
    sl->Append(end, bang);
  }
  return sl;
}

SymbolList *DynamoParse::MapSymList(SymbolList *in, Variable *range, SymbolList *list) {
  list->SetMapRange(range);
  if (in) {
    in->Append(list);
    return in;
  }
  return list;
}
UnitExpression *DynamoParse::UnitsDiv(UnitExpression *num, UnitExpression *denom) {
  return num->Divide(denom);
}
UnitExpression *DynamoParse::UnitsMult(UnitExpression *f, UnitExpression *s) {
  return f->Multiply(s);
}
UnitExpression *DynamoParse::UnitsRange(UnitExpression *e, double minval, double maxval, double increment) {
  if (e == NULL) {
    Units *units = DPObject->InsertUnits("1");
    if (units) {
      e = DPObject->InsertUnitExpression(units);
    }
  }
  e->SetRange(minval, maxval, increment);
  return e;
}

SymbolListList *DynamoParse::ChainSublist(SymbolListList *sll, SymbolList *nsl) {
  if (!sll)
    sll = new SymbolListList(pSymbolNameSpace, nsl);
  else
    sll->Append(nsl);
  return sll;
}
ExpressionList *DynamoParse::ChainExpressionList(ExpressionList *el, Expression *e) {
  if (!el)
    el = new ExpressionList(pSymbolNameSpace);
  return el->Append(e);
}
Expression *DynamoParse::NumExpression(double num) {
  return new ExpressionNumber(pSymbolNameSpace, num);
}
Expression *DynamoParse::LiteralExpression(const char *lit) {
  return new ExpressionLiteral(pSymbolNameSpace, lit);
}
ExpressionVariable *DynamoParse::VarExpression(Variable *var, SymbolList *subs) {
  return new ExpressionVariable(pSymbolNameSpace, var, subs);
}
ExpressionSymbolList *DynamoParse::SymlistExpression(SymbolList *subs, SymbolList *map) {
  return new ExpressionSymbolList(pSymbolNameSpace, subs, map);
}
Expression *DynamoParse::OperatorExpression(int oper, Expression *exp1, Expression *exp2) {
  switch (oper) {
  case '*':
    return new ExpressionMultiply(pSymbolNameSpace, exp1, exp2);
  case '/':
    return new ExpressionDivide(pSymbolNameSpace, exp1, exp2);
  case '+':
    if (!exp1 && exp2 && exp2->GetType() == EXPTYPE_Number)
      return exp2; /* unary plus just ignore */
    return new ExpressionAdd(pSymbolNameSpace, exp1, exp2);
  case '-':
    if (!exp1) {
      if (exp2 && exp2->GetType() == EXPTYPE_Number) {
        exp2->FlipSign();
        return exp2;
      }
      return new ExpressionUnaryMinus(pSymbolNameSpace, exp2, NULL);
    }
    return new ExpressionSubtract(pSymbolNameSpace, exp1, exp2);
  case '^':
    return new ExpressionPower(pSymbolNameSpace, exp1, exp2);
  case '(':
    assert(!exp2);
    return new ExpressionParen(pSymbolNameSpace, exp1, NULL);
  case '<':
  case '>':
  case DPTT_le:
  case DPTT_ge:
  case DPTT_ne:
  case DPTT_and:
  case DPTT_or:
  case '=':
    return new ExpressionLogical(pSymbolNameSpace, exp1, exp2, oper);
  case DPTT_not:
    assert(exp2 == NULL);
    return new ExpressionLogical(pSymbolNameSpace, NULL, exp1, oper);
  default:
    has_error_ = true;
    last_error_ = "Unknown operator internal error ";
    return nullptr;
  }
}
Expression *DynamoParse::FunctionExpression(Function *func, ExpressionList *eargs) {
  if (func->IsIntegrator()) {
    if (eargs->Length() != 1) {
      has_error_ = true;
      last_error_ = "Invalid Level Equation internal error";
      return nullptr;
    }
  } else if (func->NumberArgs() >= 0 &&
             ((!eargs && func->NumberArgs() > 0) || (eargs && func->NumberArgs() != eargs->Length()))) {
    has_error_ = true;
    last_error_ = "Argument count mismatch for ";
    last_error_.append(func->GetName());
    return nullptr;
  }
  if (func->IsMemoryless() && !func->IsTimeDependent())
    return new ExpressionFunction(pSymbolNameSpace, func, eargs);
  return new ExpressionFunctionMemory(pSymbolNameSpace, func, eargs);
}
Expression *DynamoParse::LookupExpression(ExpressionVariable *var, ExpressionList *args) {
  if (args->Length() == 1)
    return new ExpressionLookup(pSymbolNameSpace, var, args->GetExp(0));
  // really an error so we use uknown function
  const std::string &name = var->GetVariable()->GetName();
  Function *f = new UnknownFunction(new SymbolNameSpace(), name, args->Length());
  return new ExpressionFunction(pSymbolNameSpace, f, args);
}

ExpressionTable *DynamoParse::TablePairs(ExpressionTable *table, double x, double y) {
  if (!table)
    table = new ExpressionTable(pSymbolNameSpace);
  table->AddPair(x, y);
  return table;
}

ExpressionTable *DynamoParse::XYTableVec(ExpressionTable *table, double val) {
  if (!table)
    table = new ExpressionTable(pSymbolNameSpace);
  table->AddYVal(val);  // fix these after reducing
  return table;
}

ExpressionTable *DynamoParse::TableRange(ExpressionTable *table, double x1, double y1, double x2, double y2) {
  table->AddRange(x1, y1, x2, y2);
  return table;
}

void DynamoParse::MacroStart() {
  bInMacro = true;
  pMainSymbolNameSpace = pSymbolNameSpace;
  pSymbolNameSpace = new SymbolNameSpace();  // local name space for macro variables and macro name - macro name will go
                                             // into the main name space on close
  ReadyFunctions();                          // against this new name space - somewhat duplicative
}

void DynamoParse::MacroExpression(Variable *name, ExpressionList *margs) {
  // the macro functiongoes against the main name space - everything else is local
  mMacroFunctions.push_back(new MacroFunction(pMainSymbolNameSpace, pSymbolNameSpace, name->GetName(), margs));
}
void DynamoParse::MacroEnd() {
  pSymbolNameSpace = pMainSymbolNameSpace;
  bInMacro = false;
}

bool DynamoParse::LetterPolarity() const {
  return _model->LetterPolarity();
}
void DynamoParse::SetLetterPolarity(bool set) {
  _model->SetLetterPolarity(set);
}

void DynamoParse::ParseSpecs() {
  mDynamoLex.ParseSpecs(_model);
}

void DynamoParse::ParseSave() {
  mDynamoLex.ParseSpecs(NULL);
}
