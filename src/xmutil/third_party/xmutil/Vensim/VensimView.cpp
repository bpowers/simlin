#include "VensimView.h"

#include "../Symbol/Variable.h"
#include "VensimParse.h"

VensimVariableElement::VensimVariableElement(VensimView *view, char *curpos, char *buf, VensimParse *parser) {
  std::string name;
  curpos = parser->GetString(curpos, name);  // this might be an index number

  curpos = parser->GetInt(curpos, _x);
  curpos = parser->GetInt(curpos, _y);
  curpos = parser->GetInt(curpos, _width);
  curpos = parser->GetInt(curpos, _height);

  int shape, bits;
  curpos = parser->GetInt(curpos, shape);
  if (shape & (1 << 5))
    _attached = true;
  else
    _attached = false;
  curpos = parser->GetInt(curpos, bits);
  if ((bits & 1))
    _ghost = false;
  else
    _ghost = true;
  _cross_level = false;

#ifndef NDEBUG
  if (name == "P100")
    curpos = curpos;
#endif

  // try to find the variable
  _variable = parser->FindVariable(name);
  if (_variable) {
    if (_variable->GetView())
      _ghost = true;  // only allow 1 definition
    else if (!_ghost) {
      _variable->SetView(view);
      if (_attached)
        _variable->MarkAsFlow();
    }
  } else {
    std::string *nname = SymbolNameSpace::ToLowerSpace(name);
    if (*nname != "time")  // any others?
      log("Can't find - %s\n", name.c_str());
    delete nname;
  }
}
VensimVariableElement::VensimVariableElement(VensimView *view, Variable *var, int x, int y) {
  _x = x;
  _y = y;
  _width = _height = 0;
  _ghost = var->GetView() != NULL;
  _cross_level = false;
  _variable = var;
  _variable->SetView(view);
#ifndef NDEBUG
  if (var->GetName() == "P100")
    x = x;
#endif
}

VensimCommentElement::VensimCommentElement(char *curpos, char *buf, VensimParse *parser) {
  std::string name;
  curpos = parser->GetString(curpos, name);  // this might be an index number

  curpos = parser->GetInt(curpos, _x);
  curpos = parser->GetInt(curpos, _y);
  curpos = parser->GetInt(curpos, _width);
  curpos = parser->GetInt(curpos, _height);

  int shape, bits;
  curpos = parser->GetInt(curpos, shape);
  curpos = parser->GetInt(curpos, bits);

  if (bits & (1 << 2))  // scratch name - it is the next line
  {
    parser->Lexer().ReadLine(buf, BUFLEN);
    name = buf;
  }
}

VensimValveElement::VensimValveElement(char *curpos, char *buf, VensimParse *parser) {
  std::string name;
  curpos = parser->GetString(curpos, name);  // this might be an index number

  curpos = parser->GetInt(curpos, _x);
  curpos = parser->GetInt(curpos, _y);
  curpos = parser->GetInt(curpos, _width);
  curpos = parser->GetInt(curpos, _height);
  int shape;
  curpos = parser->GetInt(curpos, shape);
  if (shape & (1 << 5))
    _attached = true;
  else
    _attached = false;
}

bool VensimVariableElement::Ghost(std::set<Variable *> *adds) {
  if (_ghost && !_cross_level) {
    if (adds) {
      std::set<Variable *>::iterator it = adds->find(this->GetVariable());
      if (it != adds->end()) {
        adds->erase(it);
        _cross_level = true;  // so it will continue to return false
        return false;
      }
    }
    return true;
  }
  return false;
}

VensimConnectorElement::VensimConnectorElement(char *curpos, char *buf, VensimParse *parser) {
  curpos = parser->GetInt(curpos, _from);
  curpos = parser->GetInt(curpos, _to);
  std::string ignore;
  curpos = parser->GetString(curpos, ignore);
  curpos = parser->GetString(curpos, ignore);
  int polarity_ascii;
  curpos = parser->GetInt(curpos, polarity_ascii);
  if (polarity_ascii == 'S' || polarity_ascii == 's') {
    parser->SetLetterPolarity(true);
    _polarity = '+';
  } else if (polarity_ascii == 'O' || polarity_ascii == '0') {
    parser->SetLetterPolarity(true);
    _polarity = '-';
  } else
    _polarity = polarity_ascii;  // might be invalid
  curpos = parser->GetString(curpos, ignore);
  curpos = parser->GetString(curpos, ignore);
  curpos = parser->GetString(curpos, ignore);
  curpos = parser->GetString(curpos, ignore);
  curpos = parser->GetString(curpos, ignore);
  curpos = parser->GetString(curpos, ignore);

  int npoints;
  sscanf(curpos, "%d|(%d,%d)", &npoints, &_x, &_y);
  _npoints = 1;  // todo get all of them

  std::string name;
  curpos = parser->GetString(curpos, name);  // this might be an index number
}

VensimConnectorElement::VensimConnectorElement(int from, int to, int x, int y) {
  _from = from;
  _to = to;
  _npoints = 1;
  _x = x;
  _y = y;
}

bool VensimConnectorElement::ScalePoints(double xs, double ys, int xo, int yo) {
  if (_x != 0 || _y != 0)  // invalide leave it alone
  {
    _x = _x * xs + xo;
    _y = _y * ys + yo;
  }
  return true;
}

void VensimView::ReadView(VensimParse *parser, char *buf) {
  VensimLex &lexer = parser->Lexer();
  while (true) {
    lexer.ReadLine(buf, BUFLEN);  // version line
    if (buf[0] < '0' || buf[0] > '9')
      break;
    int len = 0;
    int type = -1;
    int uid = -1;
    char *curpos = parser->GetInt(buf, type);
    curpos = parser->GetInt(curpos, uid);
    if (type >= 0 && uid >= 0)  // otherwise ignore
    {
      if (uid > len) {
        len = uid + 25;
        vElements.resize(len + 1, NULL);
      }
      assert(vElements[uid] == NULL);
      switch (type) {
      case 10:  // a variable
        vElements[uid] = new VensimVariableElement(this, curpos, buf, parser);
        break;
      case 11:  // a valve if connected to a variable always just after it in the lest (??)
        vElements[uid] = new VensimValveElement(curpos, buf, parser);
        break;
      case 12:  // a comment including clouds
        vElements[uid] = new VensimCommentElement(curpos, buf, parser);
        break;
      case 1:  // a connector
        vElements[uid] = new VensimConnectorElement(curpos, buf, parser);
        break;
      case 30:  // a ??????
        break;
      default:
        assert(false);
        break;
      }
    }
  }
}

int VensimView::GetNextUID() {
  for (size_t i = vElements.size(); --i > 0;) {
    if (!vElements[i])
      return i;
  }
  vElements.resize(vElements.size() + 25, NULL);
  return GetNextUID();
}

int VensimView::SetViewStart(int startx, int starty, double xratio, double yratio, int uid_start) {
  _uid_offset = uid_start;
  if (this->vElements.empty())
    return _uid_offset;
  int min_x = INT32_MAX;
  int min_y = INT32_MAX;
  for (VensimViewElement *ele : vElements) {
    if (ele) {
      if (ele->X() < min_x)
        min_x = ele->X();
      if (ele->Y() < min_y)
        min_y = ele->Y();
    }
  }
  int off_x = std::round(startx - min_x * xratio);
  int off_y = std::round(starty - min_y * yratio);
  for (VensimViewElement *ele : vElements) {
    if (ele) {
      if (!ele->ScalePoints(xratio, yratio, off_x, off_y)) {
        ele->SetX(std::round(ele->X() * xratio + off_x));
        ele->SetY(std::round(ele->Y() * yratio + off_y));
        ele->SetWidth(std::round(ele->Width() * xratio));
        ele->SetHeight(std::round(ele->Height() * yratio));
      }
    }
  }
  return _uid_offset + vElements.size();
}

int VensimView::GetViewMaxX(int defval) {
  if (this->vElements.empty())
    return defval;
  int max_x = -INT32_MAX;
  for (VensimViewElement *ele : vElements) {
    if (ele) {
      if (ele->X() > max_x)
        max_x = ele->X();
    }
  }
  return max_x;
}
int VensimView::GetViewMaxY(int defval) {
  if (this->vElements.empty())
    return defval;
  int max_y = -INT32_MAX;
  for (VensimViewElement *ele : vElements) {
    if (ele) {
      if (ele->Y() > max_y)
        max_y = ele->Y();
    }
  }
  return max_y;
}

bool VensimView::UpgradeGhost(Variable *var) {
  for (VensimViewElement *ele : vElements) {
    if (ele && ele->Type() == VensimViewElement::ElementTypeVARIABLE) {
      VensimVariableElement *vele = static_cast<VensimVariableElement *>(ele);
      if (vele->GetVariable() == var) {
        assert(vele->Ghost(NULL));
        vele->SetGhost(false);
        var->SetView(this);  // now done
        return true;
      }
    }
  }
  return false;
}

bool VensimView::AddFlowDefinition(Variable *var, Variable *upstream, Variable *downstream) {
  // for flows we are looking for any stocks that use the flow
  // for
  int xstart, ystart, xend, yend;
  xstart = ystart = xend = yend = 0;
  bool startfound = false;
  bool endfound = false;
  for (VensimViewElement *ele : vElements) {
    if (ele && ele->Type() == VensimViewElement::ElementTypeVARIABLE) {
      VensimVariableElement *vele = static_cast<VensimVariableElement *>(ele);
      if (vele->GetVariable() == upstream) {
        xstart = vele->X();
        ystart = vele->Y();
        startfound = true;
        if (endfound)
          break;
      } else if (vele->GetVariable() == downstream) {
        xend = vele->X();
        yend = vele->Y();
        endfound = true;
        if (startfound)
          break;
      }
    }
  }
  if (!startfound && !endfound)
    return false;              // can't find anything - should not happen
  if (startfound && endfound)  // put in the middle
  {
    xstart = (xstart + xend) / 2;
    ystart = (ystart + yend) / 2;
  } else if (startfound) {
    xstart += 60;
  } else if (endfound) {
    xstart = xend - 60;
    ystart = yend;
  }
  // add the var to this view
  int uid = this->GetNextUID();
  vElements[uid] = new VensimVariableElement(this, var, xstart, ystart);
  return true;
}

bool VensimView::AddVarDefinition(Variable *var, int x, int y) {
  // add the var to this view
  int uid = this->GetNextUID();
  vElements[uid] = new VensimVariableElement(this, var, x, y);
  return true;
}

// add if msising - if extra just ignore
void VensimView::CheckGhostOwners() {
  int uid;
  int n = vElements.size();
  for (uid = 0; uid < n; uid++) {
    VensimViewElement *ele = vElements[uid];
    if (ele && ele->Type() == VensimViewElement::ElementTypeVARIABLE) {
      VensimVariableElement *vele = static_cast<VensimVariableElement *>(ele);
      Variable *var = vele->GetVariable();
      if (var && var->GetView() == NULL) {
        var->SetView(this);
        vele->SetGhost(false);
      }
    }
  }
}

// add if msising - if extra just ignore
void VensimView::CheckLinksIn() {
  int uid;
  int n = vElements.size();
  for (uid = 0; uid < n; uid++) {
    VensimViewElement *ele = vElements[uid];
    if (ele && ele->Type() == VensimViewElement::ElementTypeVARIABLE) {
      VensimVariableElement *vele = static_cast<VensimVariableElement *>(ele);
      Variable *var = vele->GetVariable();
      if (var && var->VariableType() != XMILE_Type_STOCK && !vele->Ghost(NULL)) {
        std::vector<Variable *> ins = var->GetInputVars();
        for (Variable *in : ins) {
          if (!this->FindInArrow(in, uid) && in->VariableType() != XMILE_Type_ARRAY &&
              in->VariableType() != XMILE_Type_ARRAY_ELM && in->VariableType() != XMILE_Type_UNKNOWN) {
            int fromuid = this->FindVariable(in, vele->X(), vele->Y() + 30);
            int x = (vElements[fromuid]->X() + vele->X()) / 2;
            int y = (vElements[fromuid]->Y() + vele->Y()) / 2;
            int nuid = this->GetNextUID();
            vElements[nuid] = new VensimConnectorElement(fromuid, uid, x, y);
          }
        }
        this->RemoveExtraArrowsIn(ins, uid);  // sometimes there are anomolous arrows that show up
      }
    }
  }
}

bool VensimView::FindInArrow(Variable *in, int target) {
  for (VensimViewElement *ele : this->vElements) {
    if (ele && ele->Type() == VensimViewElement::ElementTypeCONNECTOR) {
      VensimConnectorElement *cele = static_cast<VensimConnectorElement *>(ele);
      int to = cele->To();
      VensimValveElement *tele = static_cast<VensimValveElement *>(vElements[to]);
      if (tele && tele->Type() == VensimViewElement::ElementTypeVALVE && tele->Attached())
        to++;
      if (to == target) {
        VensimVariableElement *from = static_cast<VensimVariableElement *>(vElements[cele->From()]);
        if (from && from->Type() == VensimViewElement::ElementTypeVALVE && tele->Attached() &&
            static_cast<VensimValveElement *>(vElements[cele->From()])->Attached())
          from = static_cast<VensimVariableElement *>(vElements[cele->From() + 1]);
        if (from->Type() == VensimViewElement::ElementTypeVARIABLE && from->GetVariable() == in)
          return true;
      }
    }
  }
  return false;
}

void VensimView::RemoveExtraArrowsIn(std::vector<Variable *> ins, int target) {
  for (VensimViewElement *ele : this->vElements) {
    if (ele && ele->Type() == VensimViewElement::ElementTypeCONNECTOR) {
      VensimConnectorElement *cele = static_cast<VensimConnectorElement *>(ele);
      int to = cele->To();
      VensimValveElement *tele = static_cast<VensimValveElement *>(vElements[to]);
      if (tele && tele->Type() == VensimViewElement::ElementTypeVALVE && tele->Attached())
        to++;
      if (to == target) {
        VensimVariableElement *from = static_cast<VensimVariableElement *>(vElements[cele->From()]);
        if (from && from->Type() == VensimViewElement::ElementTypeVALVE && tele->Attached() &&
            static_cast<VensimValveElement *>(vElements[cele->From()])->Attached())
          from = static_cast<VensimVariableElement *>(vElements[cele->From() + 1]);
        bool found = false;
        for (Variable *in : ins) {
          if (from->GetVariable() == in) {
            found = true;
            break;
          }
        }
        if (!found)
          cele->Invalidate();
      }
    }
  }
}

int VensimView::FindVariable(Variable *in, int x, int y) {
  int uid = 0;
  for (VensimViewElement *ele : this->vElements) {
    if (ele && ele->Type() == VensimViewElement::ElementTypeVARIABLE) {
      VensimVariableElement *vele = static_cast<VensimVariableElement *>(ele);
      if (vele->GetVariable() == in)
        return uid;
    }
    uid++;
  }
  uid = GetNextUID();
  vElements[uid] = new VensimVariableElement(this, in, x, y);
  return uid;
}
