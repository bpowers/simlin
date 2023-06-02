#ifndef _XMUTIL_MODEL_H
#define _XMUTIL_MODEL_H
#include <vector>

#include "Symbol/Expression.h"
#include "Symbol/Variable.h"

enum Integration_Type { Integration_Type_EULER, Integration_Type_RK2, Integration_Type_RK4 };
class View {
public:
  virtual bool UpgradeGhost(Variable *var) = 0;
  virtual bool AddFlowDefinition(Variable *var, Variable *in, Variable *out) = 0;
  virtual bool AddVarDefinition(Variable *var, int x, int y) = 0;
  virtual void CheckLinksIn() = 0;
  virtual void CheckGhostOwners() = 0;
  // just a placeholder to derive from
};
class Model {
public:
  Model(void);
  ~Model(void);
  bool UnitsCheck(void) {
    return false;
  }
  bool AnalyzeEquations(void);
  bool Simulate(void);
  SymbolNameSpace *GetNameSpace(void) {
    return &mSymbolNameSpace;
  }
  Equation *AddUnnamedVariable(ExpressionFunctionMemory *e);
  bool RenameVariable(Variable *v, const std::string &newname);
  void GenerateCanonicalNames(void);
  void GenerateShortNames(void);
  bool OutputComputable(bool wantshort);
  bool MarkVariableTypes(SymbolNameSpace *ns);
  void CheckGhostOwners();
  void AttachStragglers();  // try to get diagramatic stuff right
  void MakeViewNamesUnique();
  std::string PrintXMILE(bool isCompact, std::vector<std::string> &errs, double xscale, double yscale);

  double GetConstanValue(const char *var, double defval);
  UnitExpression *GetUnits(const char *var);
  std::vector<std::string> &UnitEquivs() {
    return vUnitEquivs;
  }
  void SetUnwanted(const char *var, const char *nametouse);
  std::vector<Variable *> GetVariables(SymbolNameSpace *ns = NULL);
  void AddView(View *view) {
    vViews.push_back(view);
  }
  std::vector<View *> &Views() {
    return vViews;
  }

  std::vector<MacroFunction *> &MacroFunctions() {
    return mMacroFunctions;
  }
  void SetMacroFunctions(std::vector<MacroFunction *> set) {
    mMacroFunctions = set;
  }
  void SetIntegrationType(Integration_Type type) {
    iIntegrationType = type;
  }
  Integration_Type IntegrationType() {
    return iIntegrationType;
  }
  std::vector<ModelGroup> &Groups() {
    return vGroups;
  }

  void SetAsSectors(bool set) {
    bAsSectors = set;
  }
  bool AsSectors() const {
    return bAsSectors;
  }

  void SetLetterPolarity(bool set) {
    bLetterPolarity = set;
  }
  bool LetterPolarity() const {
    return bLetterPolarity;
  }

private:
  bool OrderEquations(ContextInfo *info, bool tonly);
  bool SetupVariableStates(int pass);
  bool ValidatePlaceholderVars(void);
  bool OrganizeSubscripts(void);
  void ClearCompEquations(void);

  SymbolNameSpace mSymbolNameSpace;
  std::vector<ModelGroup> vGroups;
  std::vector<View *> vViews;
  std::vector<Variable *> vUnamedVars;
  // std::vector<Equation *>vConstantComps ; // actually just assignment
  std::vector<Equation *> vInitialTimeComps;
  std::vector<Equation *> vInitialComps;
  std::vector<Equation *> vUnchangingComps;
  std::vector<Equation *> vActiveComps;
  std::vector<Equation *> vRateComps;
  std::vector<MacroFunction *> mMacroFunctions;
  std::vector<std::string> vUnitEquivs;
  /* the last could be part of active but it is helpful to split
     out when creating equations for a computer language */
  int iNLevel;
  int iNAux;
  Integration_Type iIntegrationType;
  double *dLevel;
  double *dRate;
  double *dAux;
  bool bAsSectors;
  bool bLetterPolarity;
};

#endif
