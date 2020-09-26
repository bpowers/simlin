#ifndef _XMUTIL_VENSIM_VENSIMVIEW_H
#define _XMUTIL_VENSIM_VENSIMVIEW_H
#include <string>

#include "../Model.h"
#include "../Symbol/Parse.h"
#include "../Symbol/Symbol.h"
#include "VensimLex.h"
class VensimParse;
class VensimView;
class Variable;

class VensimViewElement {
public:
  enum ElementType { ElementTypeVARIABLE, ElementTypeVALVE, ElementTypeCOMMENT, ElementTypeCONNECTOR };
  virtual ElementType Type() = 0;
  int X() {
    return _x;
  }
  void SetX(int x) {
    _x = x;
  }
  int Y() {
    return _y;
  }
  void SetY(int y) {
    _y = y;
  }

protected:
  int _x;
  int _y;
  int _width;
  int _height;
};
typedef std::vector<VensimViewElement *> VensimViewElements;
class VensimVariableElement : public VensimViewElement {
public:
  VensimVariableElement(VensimView *view, char *curpos, char *buf, VensimParse *parser);
  VensimVariableElement(VensimView *view, Variable *var, int x, int y);
  ElementType Type() {
    return ElementTypeVARIABLE;
  }
  Variable *GetVariable() {
    return _variable;
  }
  bool Ghost() {
    return _ghost;
  }
  void SetGhost(bool set) {
    _ghost = set;
  }
  bool Attached() {
    return _attached;
  }

protected:
  Variable *_variable;
  bool _ghost;
  bool _attached;  // to a valve for flows
};
class VensimValveElement : public VensimViewElement {
public:
  ElementType Type() {
    return ElementTypeVALVE;
  }
  VensimValveElement(char *curpos, char *buf, VensimParse *parser);
  bool Attached() {
    return _attached;
  }

private:
  bool _attached;
};
class VensimCommentElement : public VensimViewElement {
public:
  ElementType Type() {
    return ElementTypeCOMMENT;
  }
  VensimCommentElement(char *curpos, char *buf, VensimParse *parser);
};
class VensimConnectorElement : public VensimViewElement {
public:
  ElementType Type() {
    return ElementTypeCONNECTOR;
  }
  VensimConnectorElement(char *curpos, char *buf, VensimParse *parser);
  VensimConnectorElement(int from, int to, int x, int y);
  int From() {
    return _from;
  }
  int To() {
    return _to;
  }
  void Invalidate() {
    _to = _from = 0;
  }
  bool FromAsAlias();  // in this case From will send back a number
private:
  int _from;
  int _to;
  int _npoints;
};

class VensimView : public View {
public:
  const std::string &Title() {
    return sTitle;
  }
  void SetTitle(const std::string &title) {
    sTitle = title;
  }
  void ReadView(VensimParse *parser, char *buf);
  int GetNextUID();
  VensimViewElements &Elements() {
    return vElements;
  }

  bool UpgradeGhost(Variable *var);
  bool AddFlowDefinition(Variable *var, Variable *upstream, Variable *downstream);
  bool AddVarDefinition(Variable *var, int x, int y);
  void CheckLinksIn();
  bool FindInArrow(Variable *source, int target);
  void RemoveExtraArrowsIn(std::vector<Variable *> ins, int target);
  int FindVariable(Variable *in, int x, int y);  // add if necessary - returns UID

  int SetViewStart(int x, int y, int uid);  // returns last uid val + 1
  int GetViewMaxX(int defval);
  int GetViewMaxY(int defval);
  int UIDOffset() {
    return _uid_offset;
  }

private:
  VensimViewElements vElements;
  std::string sTitle;
  int _uid_offset;
};

#endif