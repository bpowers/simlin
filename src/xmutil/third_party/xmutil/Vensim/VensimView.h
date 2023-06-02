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
  virtual bool ScalePoints(double xratio, double yratio, int offx, int offy) {
    return false;
  }
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
  int Width() const {
    return _width;
  }
  void SetWidth(int w) {
    _width = w;
  }
  int Height() const {
    return _height;
  }
  void SetHeight(int h) {
    _height = h;
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
  bool Ghost(std::set<Variable *> *adds);
  bool CrossLevel() {
    return _cross_level;
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
  bool _cross_level;
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
  ElementType Type() override {
    return ElementTypeCONNECTOR;
  }
  VensimConnectorElement(char *curpos, char *buf, VensimParse *parser);
  VensimConnectorElement(int from, int to, int x, int y);
  virtual bool ScalePoints(double xratio, double yratio, int offx, int offy) override;
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
  char Polarity() const {
    return _polarity;
  }

private:
  int _from;
  int _to;
  int _npoints;
  char _polarity;
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

  virtual bool UpgradeGhost(Variable *var) override;
  virtual bool AddFlowDefinition(Variable *var, Variable *upstream, Variable *downstream) override;
  virtual bool AddVarDefinition(Variable *var, int x, int y) override;
  virtual void CheckGhostOwners() override;
  virtual void CheckLinksIn() override;
  bool FindInArrow(Variable *source, int target);
  void RemoveExtraArrowsIn(std::vector<Variable *> ins, int target);
  int FindVariable(Variable *in, int x, int y);  // add if necessary - returns UID

  int SetViewStart(int x, int y, double xratio, double yratio, int uid);  // returns last uid val + 1
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
