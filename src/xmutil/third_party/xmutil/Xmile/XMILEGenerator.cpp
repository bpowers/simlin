#include "XMILEGenerator.h"

#include <algorithm>

#include "../Model.h"
#include "../Symbol/ExpressionList.h"
#include "../Vensim/VensimView.h"
#include "../XMUtil.h"

XMILEGenerator::XMILEGenerator(Model *model, double xratio, double yratio) {
  _model = model;
  _xratio = xratio;
  _yratio = yratio;
}

std::string XMILEGenerator::Print(bool is_compact, std::vector<std::string> &errs, bool as_sectors) {
  tinyxml2::XMLDocument doc;

  tinyxml2::XMLElement *root = doc.NewElement("xmile");
  root->SetName("xmile");
  root->SetAttribute("xmlns", "http://docs.oasis-open.org/xmile/ns/XMILE/v1.0");
  root->SetAttribute("xmlns:isee", "http://iseesystems.com/XMILE");
  root->SetAttribute("version", "1.0");
  doc.InsertFirstChild(root);

  tinyxml2::XMLElement *prefs = doc.NewElement("isee:prefs");
  root->InsertEndChild(prefs);
  prefs->SetAttribute("show_module_prefix", "true");
  prefs->SetAttribute("layer", "model");

  tinyxml2::XMLElement *header = doc.NewElement("header");
  this->generateHeader(header, errs);
  root->InsertEndChild(header);

  tinyxml2::XMLElement *specs = doc.NewElement("sim_specs");
  this->generateSimSpecs(specs, errs);
  root->InsertEndChild(specs);

  tinyxml2::XMLElement *model_units = doc.NewElement("model_units");
  this->generateModelUnits(model_units, errs);
  root->InsertEndChild(model_units);

  tinyxml2::XMLElement *dimensions = doc.NewElement("dimensions");
  this->generateDimensions(dimensions, errs);
  root->InsertEndChild(dimensions);
  _model->MakeViewNamesUnique();
  if (as_sectors) {
    tinyxml2::XMLElement *model = doc.NewElement("model");
    this->generateModelAsSectors(model, errs, NULL, true);
    root->InsertEndChild(model);
  } else
    this->generateModelAsModules(root, errs, NULL);

  // macros are presented as separate models
  for (MacroFunction *mf : _model->MacroFunctions()) {
    tinyxml2::XMLElement *macro = doc.NewElement("macro");
    macro->SetAttribute("name", mf->GetName().c_str());

    // in vensim the equation is always just the name fo the macro
    tinyxml2::XMLElement *xeqn = doc.NewElement("eqn");
    macro->InsertEndChild(xeqn);
    xeqn->SetText(mf->GetName().c_str());
    // the parms are all of the entries in the macro description
    ExpressionList *args = mf->Args();
    int n = args->Length();
    for (int i = 0; i < n; i++) {
      tinyxml2::XMLElement *xparm = doc.NewElement("parm");
      macro->InsertEndChild(xparm);
      Expression *pexp = args->GetExp(i);
      ContextInfo info(NULL);
      pexp->OutputComputable(&info);
      xparm->SetText(info.str().c_str());
    }

    this->generateModelAsSectors(macro, errs, mf->NameSpace(), false);  // not really a secotr - only a root module here
    root->InsertEndChild(macro);
  }

  tinyxml2::XMLPrinter printer{nullptr, is_compact};
  if (!doc.Accept(&printer)) {
    if (doc.ErrorStr()) {
      errs.push_back("TinyXML2 Error: " + std::string(doc.ErrorStr()));
    }
    return "";
  }

  std::string xmile = printer.CStr();

  return xmile;
}

void XMILEGenerator::generateHeader(tinyxml2::XMLElement *element, std::vector<std::string> &errs) {
  tinyxml2::XMLDocument *doc = element->GetDocument();

  tinyxml2::XMLElement *options = doc->NewElement("options");
  options->SetAttribute("namespace", "std");
  element->InsertEndChild(options);

  tinyxml2::XMLElement *vendor = doc->NewElement("vendor");
  vendor->SetText("Ventana Systems, xmutil");
  element->InsertEndChild(vendor);

  tinyxml2::XMLElement *product = doc->NewElement("product");
  product->SetAttribute("lang", "en");
  product->SetText("Vensim, xmutil");
  element->InsertEndChild(product);
}

void XMILEGenerator::generateSimSpecs(tinyxml2::XMLElement *element, std::vector<std::string> &errs) {
  /*
  <sim_specs method = "Euler" time_units = "Months">
  <start>0< / start>
  <stop>100< / stop>
  <dt>0.125< / dt>
  < / sim_specs>
  */

  tinyxml2::XMLDocument *doc = element->GetDocument();

  if (_model->IntegrationType() == Integration_Type_RK4)
    element->SetAttribute("method", "RK4");
  else if (_model->IntegrationType() == Integration_Type_RK2)
    element->SetAttribute("method", "RK2");
  else
    element->SetAttribute("method", "Euler");

  UnitExpression *uexpr = _model->GetUnits("TIME STEP");
  if (!uexpr)
    uexpr = _model->GetUnits("FINAL TIME");
  if (!uexpr)
    uexpr = _model->GetUnits("INITIAL TIME");
  if (uexpr)
    element->SetAttribute("time_units", uexpr->GetEquationString().c_str());
  else
    element->SetAttribute("time_units", "Months");

  double start = _model->GetConstanValue("INITIAL TIME", -1);  // default to 0 if INITIAL TIME is missing or an equation
  double stop = _model->GetConstanValue("FINAL TIME", 100);
  double dt = _model->GetConstanValue("TIME STEP", 1);
  double saveper = _model->GetConstanValue("SAVEPER", dt);
  double speed = _model->GetConstanValue("SIMULATION PAUSE", 0);

  if (start == -1) {
    if (stop > 200)  // this happens to work for national model - but hey
      start = stop - 200;
    else
      start = 0;
  }
  if (stop <= start)
    stop = start + 10 * dt;

  if (speed > 0) {
    double duration = (stop - start) / saveper * speed;
    char dur[32] = {0};
    snprintf(dur, 32, "%g", duration);
    element->SetAttribute("isee:sim_duration", dur);
  } else
    element->SetAttribute("isee:sim_duration", "0");

  tinyxml2::XMLElement *startEle = doc->NewElement("start");
  startEle->SetText(StringFromDouble(start).c_str());
  element->InsertEndChild(startEle);

  tinyxml2::XMLElement *stopEle = doc->NewElement("stop");
  stopEle->SetText(StringFromDouble(stop).c_str());
  element->InsertEndChild(stopEle);

  tinyxml2::XMLElement *dtEle = doc->NewElement("dt");
  dtEle->SetText(StringFromDouble(dt).c_str());
  element->InsertEndChild(dtEle);

  if (saveper > dt) {
    element->SetAttribute("isee:save_interval", std::to_string(saveper).c_str());
  }

  _model->SetUnwanted("INITIAL TIME", "STARTTIME");
  _model->SetUnwanted("FINAL TIME", "STOPTIME");
  _model->SetUnwanted("TIME STEP", "DT");
  _model->SetUnwanted("SAVEPER", "SAVEPER");
}

void XMILEGenerator::generateModelUnits(tinyxml2::XMLElement *element, std::vector<std::string> &errs) {
  /*
          <model_units>
          <unit name="Dollar">
<eqn>$</eqn>/>
                  <alias>Dollars</alias>
                  <alias>$s</alias>
</unit>
  </model_units>

  */

  tinyxml2::XMLDocument *doc = element->GetDocument();

  std::vector<std::string> &equivs = _model->UnitEquivs();

  for (std::string &equiv : equivs) {
    std::string name;
    std::string eqn;
    std::vector<std::string> aliases;
    const char *cur = equiv.c_str();
    while (*cur) {
      const char *tv = cur;
      while (true) {
        if (*tv == ',' || !*tv) {
          std::string cur_e(cur, tv - cur);
          if (cur_e == "$")
            eqn = cur_e;
          else if (name.empty())
            name = cur_e;
          else
            aliases.push_back(cur_e);
          if (*tv)
            tv++;
          break;
        }
        tv++;
      }
      cur = tv;
    }
    tinyxml2::XMLElement *xunit = doc->NewElement("unit");
    xunit->SetAttribute("name", name.c_str());
    if (!eqn.empty()) {
      tinyxml2::XMLElement *xeqn = doc->NewElement("eqn");
      xeqn->SetText(eqn.c_str());
      xunit->InsertEndChild(xeqn);
    }
    for (std::string &alias : aliases) {
      tinyxml2::XMLElement *xalias = doc->NewElement("alias");
      xalias->SetText(alias.c_str());
      xunit->InsertEndChild(xalias);
    }
    element->InsertEndChild(xunit);
  }
}

void XMILEGenerator::generateDimensions(tinyxml2::XMLElement *element, std::vector<std::string> &errs) {
  tinyxml2::XMLDocument *doc = element->GetDocument();
  std::vector<Variable *> vars = _model->GetVariables();  // all symbols that are variables
  for (Variable *var : vars) {
    if (var->VariableType() == XMILE_Type_ARRAY) {
      // simple minded - defining equation -
      Equation *eq = var->GetEquation(0);
      if (eq) {
        Expression *exp = eq->GetExpression();
        if (exp && exp->GetType() == EXPTYPE_Symlist) {
          SymbolList *symlist = static_cast<ExpressionSymbolList *>(exp)->SymList();
          std::vector<Symbol *> expanded;
          int n = symlist->Length();
          for (int i = 0; i < n; i++) {
            const SymbolList::SymbolListEntry &elm = (*symlist)[i];
            if (elm.eType == SymbolList::EntryType_SYMBOL) {
              Equation::GetSubscriptElements(expanded, elm.u.pSymbol);
            }
          }
          // we define subranges as if they were arrays themselves - because of the unique namespace in XMILE this
          // is proper - and it will make any model with partial definitions more or less okay -
          if (!expanded.empty() /* && expanded[0]->Owner() == var*/) {
            tinyxml2::XMLElement *xsub = doc->NewElement("dim");
            xsub->SetAttribute("name", var->GetName().c_str());
            for (Symbol *s : expanded) {
              tinyxml2::XMLElement *xelm = doc->NewElement("elem");
              xelm->SetAttribute("name", s->GetName().c_str());
              xsub->InsertEndChild(xelm);
            }
            element->InsertEndChild(xsub);
          }
        }
      }
    }
  }
}

// first pass if flat - we probably want to do this differently when we break up into modules
void XMILEGenerator::generateModelAsSectors(tinyxml2::XMLElement *element, std::vector<std::string> &errs,
                                            SymbolNameSpace *ns, bool want_diagram) {
  tinyxml2::XMLDocument *doc = element->GetDocument();
  tinyxml2::XMLElement *variables = doc->NewElement("variables");
  element->InsertEndChild(variables);

  std::vector<Variable *> vars = _model->GetVariables(ns);  // all symbols that are variables
  for (Variable *var : vars) {
    if (var->Unwanted())
      continue;
    XMILE_Type type = var->VariableType();
    std::string tag;
    switch (type) {
    case XMILE_Type_DELAYAUX:
    case XMILE_Type_AUX:
      tag = "aux";
      break;
    case XMILE_Type_STOCK:
      tag = "stock";
      break;
    case XMILE_Type_FLOW:
      tag = "flow";
      break;
    case XMILE_Type_ARRAY:
      continue;
    case XMILE_Type_ARRAY_ELM:
      continue;
    default:
      continue;
      break;
    }
    tinyxml2::XMLElement *xvar = doc->NewElement(tag.c_str());

    variables->InsertEndChild(xvar);
    xvar->SetAttribute("name", var->GetAlternateName().c_str());

    if (type == XMILE_Type_DELAYAUX) {
      tinyxml2::XMLElement *xcomment = doc->NewElement("isee:delay_aux");
      xvar->InsertEndChild(xcomment);
    }

    std::vector<Equation *> eqns = var->GetAllEquations();
    size_t eq_count = eqns.size();

    // dimensions
    std::vector<Variable *> elmlist;
    int dim_count = var->SubscriptCountVars(elmlist);

    std::string comment = var->Comment();
    if (!comment.empty()) {
      tinyxml2::XMLElement *xcomment = doc->NewElement("doc");
      xvar->InsertEndChild(xcomment);
      xcomment->SetText(comment.c_str());
    }
    if (type == XMILE_Type_STOCK) {
      for (Variable *in : var->Inflows()) {
        tinyxml2::XMLElement *inflow = doc->NewElement("inflow");
        xvar->InsertEndChild(inflow);
        inflow->SetText(SpaceToUnderBar(in->GetAlternateName()).c_str());
      }
      for (Variable *out : var->Outflows()) {
        tinyxml2::XMLElement *outflow = doc->NewElement("outflow");
        xvar->InsertEndChild(outflow);
        outflow->SetText(SpaceToUnderBar(out->GetAlternateName()).c_str());
      }
    }

    tinyxml2::XMLElement *xelement = xvar;  // usually these are the same - but for non a2a we have element entries
    size_t eq_ind = 0;
    size_t eq_pos = 0;
    std::vector<Symbol *> subs;               // [ship,location]
    std::vector<std::vector<Symbol *>> elms;  // [s1,l1]
    std::vector<std::set<Symbol *>> entries;
    std::vector<Symbol *> dims;
    while (eq_ind < eq_count) {
      Equation *eqn = eqns[eq_ind];
      if (eq_count > 1) {
        if (entries.empty())
          entries.resize(dim_count);
        // we will blow up everything to single elements
        if (elms.empty()) {
          eq_pos = 0;
          elms.clear();
          eqn->SubscriptExpand(elms, subs);
          if (!elms.empty()) {
            for (std::vector<Symbol *> elm : elms) {
              for (int i = 0; i < dim_count; i++) {
                entries[i].insert(elm[i]);
              }
            }
          }
        }
        if (!elms.empty()) {
          dims = elms[eq_pos];
          std::string s;
          int dim_count = dims.size();
          for (int j = 0; j < dim_count; j++) {
            if (j)
              s += ", ";
            s += dims[j]->GetName();
          }
          xelement = doc->NewElement("element");
          xelement->SetAttribute("subscript", s.c_str());
          xvar->InsertEndChild(xelement);
        }
      }
      // skip it altogether if it is an A FUNCTION OF equation
      std::string rhs = eqn->RHSFormattedXMILE(var, subs, dims, false);
      if (eq_count <= 1 || rhs.size() < 42 || rhs.substr(28, 13) != "A FUNCTION OF") {
        tinyxml2::XMLElement *xeqn = doc->NewElement("eqn");
        xelement->InsertEndChild(xeqn);
        xeqn->SetText(rhs.c_str());

        // it it is active init we need to store that separately
        if (eqn->IsActiveInit()) {
          tinyxml2::XMLElement *xieqn = doc->NewElement("init_eqn");
          xelement->InsertEndChild(xieqn);
          xieqn->SetText(eqn->RHSFormattedXMILE(var, subs, dims, true).c_str());
        }

        // if it has a lookup we need to store that separately
        ExpressionTable *et = eqn->GetTable();
        if (et) {
          assert(type == XMILE_Type_AUX || type == XMILE_Type_FLOW);
          std::vector<double> *xvals = et->GetXVals();
          std::vector<double> *yvals = et->GetYVals();
          tinyxml2::XMLElement *gf = doc->NewElement("gf");
          if (et->Extrapolate())
            gf->SetAttribute("type", "extrapolate");
          xelement->InsertEndChild(gf);
          tinyxml2::XMLElement *yscale = doc->NewElement("yscale");
          gf->InsertEndChild(yscale);
          tinyxml2::XMLElement *xpts = doc->NewElement("xpts");
          gf->InsertEndChild(xpts);
          tinyxml2::XMLElement *ypts = doc->NewElement("ypts");
          gf->InsertEndChild(ypts);

          std::string xstr;
          for (size_t i = 0; i < xvals->size(); i++) {
            if (i)
              xstr += ",";
            xstr += StringFromDouble((*xvals)[i]);
          }
          xpts->SetText(xstr.c_str());

          std::string ystr;
          double ymin = 0;
          double ymax = 0;
          for (size_t i = 0; i < yvals->size(); i++) {
            if (i) {
              ystr += ",";
              if ((*yvals)[i] < ymin)
                ymin = (*yvals)[i];
              else if ((*yvals)[i] > ymax)
                ymax = (*yvals)[i];
            } else
              ymin = ymax = (*yvals)[i];
            ystr += StringFromDouble((*yvals)[i]);
          }
          ypts->SetText(ystr.c_str());

          if (ymin == ymax)
            ymax = ymin + 1;
          yscale->SetAttribute("min", StringFromDouble(ymin).c_str());
          yscale->SetAttribute("max", StringFromDouble(ymax).c_str());
        }
      }
      if (eq_count > 1) {
        eq_pos++;
        if (eq_pos >= elms.size()) {
          elms.clear();
          eq_ind++;
        }
      } else
        eq_ind++;
    }

    // use entries to try to figure out the appropriate dimensions
    if (dim_count) {
      // Vensim allowed partial definition sets - XMILE uses subranges as separate dimensions so we
      // try to find the most compact set of dimensions possible that inlcude all the equations include
      std::vector<Variable *> dimensions;

      tinyxml2::XMLElement *xdims = doc->NewElement("dimensions");
      for (int i = 0; i < dim_count; i++) {
        tinyxml2::XMLElement *xdim = doc->NewElement("dim");
        if (entries.empty()) {
          // we might get a subrange in elmlist so need to get parent - but only if there is more than 1 equation
          if (eq_count > 1 || elmlist[i]->GetAllEquations().empty())
            xdim->SetAttribute("name", elmlist[i]->Owner()->GetName().c_str());
          else
            xdim->SetAttribute("name", elmlist[i]->GetName().c_str());
        } else {
          std::set<Symbol *> &entry = entries[i];
          Symbol *parent = (*entry.begin())->Owner();
          Symbol *best = parent;
          if (parent->Subranges() != NULL && static_cast<Variable *>(parent)->Nelm() > entry.size()) {
            for (Symbol *subrange : *parent->Subranges()) {
              if (static_cast<Variable *>(subrange)->Nelm() >= entry.size() &&
                  static_cast<Variable *>(subrange)->Nelm() < static_cast<Variable *>(best)->Nelm()) {
                // does it have them all
                bool complete = true;
                std::vector<Symbol *> telms;
                Equation::GetSubscriptElements(telms, subrange);
                for (Symbol *elm : entries[i]) {
                  if (std::find(telms.begin(), telms.end(), elm) == telms.end()) {
                    complete = false;
                    break;
                  }
                }
                if (complete)
                  best = subrange;
              }
            }
          }
          xdim->SetAttribute("name", best->GetName().c_str());
        }
        xdims->InsertEndChild(xdim);
      }
      xvar->InsertEndChild(xdims);
    }

    UnitExpression *un = var->Units();
    if (un) {
      tinyxml2::XMLElement *units = doc->NewElement("units");
      xvar->InsertEndChild(units);
      units->SetText(un->GetEquationString().c_str());
    }
  }
  if (want_diagram) {
    tinyxml2::XMLElement *views = doc->NewElement("views");
    this->generateSectorViews(views, variables, errs, ns == NULL);
    element->InsertEndChild(views);
  }
}

void XMILEGenerator::generateEquations(std::set<Variable *> &included, tinyxml2::XMLDocument *doc,
                                       tinyxml2::XMLElement *variables) {
  for (Variable *var : included) {
    XMILE_Type type = var->VariableType();
    std::string tag;
    switch (type) {
    case XMILE_Type_DELAYAUX:
    case XMILE_Type_AUX:
      tag = "aux";
      break;
    case XMILE_Type_STOCK:
      tag = "stock";
      break;
    case XMILE_Type_FLOW:
      tag = "flow";
      break;
    case XMILE_Type_ARRAY:
      continue;
    case XMILE_Type_ARRAY_ELM:
      continue;
    default:
      continue;
      break;
    }
    tinyxml2::XMLElement *xvar = doc->NewElement(tag.c_str());

    variables->InsertEndChild(xvar);
    xvar->SetAttribute("name", var->GetAlternateName().c_str());

    if (type == XMILE_Type_DELAYAUX) {
      tinyxml2::XMLElement *xcomment = doc->NewElement("isee:delay_aux");
      xvar->InsertEndChild(xcomment);
    }

    std::vector<Equation *> eqns = var->GetAllEquations();
    size_t eq_count = eqns.size();

    // dimensions
    std::vector<Variable *> elmlist;
    int dim_count = var->SubscriptCountVars(elmlist);

    std::string comment = var->Comment();
    if (!comment.empty()) {
      tinyxml2::XMLElement *xcomment = doc->NewElement("doc");
      xvar->InsertEndChild(xcomment);
      xcomment->SetText(comment.c_str());
    }
    if (type == XMILE_Type_STOCK) {
      for (Variable *in : var->Inflows()) {
        tinyxml2::XMLElement *inflow = doc->NewElement("inflow");
        xvar->InsertEndChild(inflow);
        inflow->SetText(SpaceToUnderBar(in->GetAlternateName()).c_str());
      }
      for (Variable *out : var->Outflows()) {
        tinyxml2::XMLElement *outflow = doc->NewElement("outflow");
        xvar->InsertEndChild(outflow);
        outflow->SetText(SpaceToUnderBar(out->GetAlternateName()).c_str());
      }
    }

    tinyxml2::XMLElement *xelement = xvar;  // usually these are the same - but for non a2a we have element entries
    size_t eq_ind = 0;
    size_t eq_pos = 0;
    std::vector<Symbol *> subs;               // [ship,location]
    std::vector<std::vector<Symbol *>> elms;  // [s1,l1]
    std::vector<std::set<Symbol *>> entries;
    std::vector<Symbol *> dims;
    while (eq_ind < eq_count) {
      Equation *eqn = eqns[eq_ind];
      if (eq_count > 1) {
        if (entries.empty())
          entries.resize(dim_count);
        // we will blow up everything to single elements
        if (elms.empty()) {
          eq_pos = 0;
          elms.clear();
          eqn->SubscriptExpand(elms, subs);
          if (!elms.empty()) {
            for (std::vector<Symbol *> elm : elms) {
              for (int i = 0; i < dim_count; i++) {
                entries[i].insert(elm[i]);
              }
            }
          }
        }
        if (!elms.empty()) {
          dims = elms[eq_pos];
          std::string s;
          int dim_count = dims.size();
          for (int j = 0; j < dim_count; j++) {
            if (j)
              s += ", ";
            s += dims[j]->GetName();
          }
          xelement = doc->NewElement("element");
          xelement->SetAttribute("subscript", s.c_str());
          xvar->InsertEndChild(xelement);
        }
      }
      // skip it altogether if it is an A FUNCTION OF equation
      std::string rhs = eqn->RHSFormattedXMILE(var, subs, dims, false);
      if (eq_count <= 1 || rhs.size() < 42 || rhs.substr(28, 13) != "A FUNCTION OF") {
        tinyxml2::XMLElement *xeqn = doc->NewElement("eqn");
        xelement->InsertEndChild(xeqn);
        xeqn->SetText(rhs.c_str());

        // it it is active init we need to store that separately
        if (eqn->IsActiveInit()) {
          tinyxml2::XMLElement *xieqn = doc->NewElement("init_eqn");
          xelement->InsertEndChild(xieqn);
          xieqn->SetText(eqn->RHSFormattedXMILE(var, subs, dims, true).c_str());
        }

        // if it has a lookup we need to store that separately
        ExpressionTable *et = eqn->GetTable();
        if (et) {
          assert(type == XMILE_Type_AUX || type == XMILE_Type_FLOW);
          std::vector<double> *xvals = et->GetXVals();
          std::vector<double> *yvals = et->GetYVals();
          tinyxml2::XMLElement *gf = doc->NewElement("gf");
          if (et->Extrapolate())
            gf->SetAttribute("type", "extrapolate");
          xelement->InsertEndChild(gf);
          tinyxml2::XMLElement *yscale = doc->NewElement("yscale");
          gf->InsertEndChild(yscale);
          tinyxml2::XMLElement *xpts = doc->NewElement("xpts");
          gf->InsertEndChild(xpts);
          tinyxml2::XMLElement *ypts = doc->NewElement("ypts");
          gf->InsertEndChild(ypts);

          std::string xstr;
          for (size_t i = 0; i < xvals->size(); i++) {
            if (i)
              xstr += ",";
            xstr += StringFromDouble((*xvals)[i]);
          }
          xpts->SetText(xstr.c_str());

          std::string ystr;
          double ymin = 0;
          double ymax = 0;
          for (size_t i = 0; i < yvals->size(); i++) {
            if (i) {
              ystr += ",";
              if ((*yvals)[i] < ymin)
                ymin = (*yvals)[i];
              else if ((*yvals)[i] > ymax)
                ymax = (*yvals)[i];
            } else
              ymin = ymax = (*yvals)[i];
            ystr += StringFromDouble((*yvals)[i]);
          }
          ypts->SetText(ystr.c_str());

          if (ymin == ymax)
            ymax = ymin + 1;
          yscale->SetAttribute("min", StringFromDouble(ymin).c_str());
          yscale->SetAttribute("max", StringFromDouble(ymax).c_str());
        }
      }
      if (eq_count > 1) {
        eq_pos++;
        if (eq_pos >= elms.size()) {
          elms.clear();
          eq_ind++;
        }
      } else
        eq_ind++;
    }

    // use entries to try to figure out the appropriate dimensions
    if (dim_count) {
      // Vensim allowed partial definition sets - XMILE uses subranges as separate dimensions so we
      // try to find the most compact set of dimensions possible that inlcude all the equations include
      std::vector<Variable *> dimensions;

      tinyxml2::XMLElement *xdims = doc->NewElement("dimensions");
      for (int i = 0; i < dim_count; i++) {
        tinyxml2::XMLElement *xdim = doc->NewElement("dim");
        if (entries.empty()) {
          // we might get a subrange in elmlist so need to get parent - but only if there is more than 1 equation
          if (eq_count > 1 || elmlist[i]->GetAllEquations().empty())
            xdim->SetAttribute("name", elmlist[i]->Owner()->GetName().c_str());
          else
            xdim->SetAttribute("name", elmlist[i]->GetName().c_str());
        } else {
          std::set<Symbol *> &entry = entries[i];
          Symbol *parent = (*entry.begin())->Owner();
          Symbol *best = parent;
          if (parent->Subranges() != NULL && static_cast<Variable *>(parent)->Nelm() > entry.size()) {
            for (Symbol *subrange : *parent->Subranges()) {
              if (static_cast<Variable *>(subrange)->Nelm() >= entry.size() &&
                  static_cast<Variable *>(subrange)->Nelm() < static_cast<Variable *>(best)->Nelm()) {
                // does it have them all
                bool complete = true;
                std::vector<Symbol *> telms;
                Equation::GetSubscriptElements(telms, subrange);
                for (Symbol *elm : entries[i]) {
                  if (std::find(telms.begin(), telms.end(), elm) == telms.end()) {
                    complete = false;
                    break;
                  }
                }
                if (complete)
                  best = subrange;
              }
            }
          }
          xdim->SetAttribute("name", best->GetName().c_str());
        }
        xdims->InsertEndChild(xdim);
      }
      xvar->InsertEndChild(xdims);
    }

    UnitExpression *un = var->Units();
    if (un) {
      tinyxml2::XMLElement *units = doc->NewElement("units");
      xvar->InsertEndChild(units);
      units->SetText(un->GetEquationString().c_str());
    }
  }
}

void XMILEGenerator::generateModelAsModules(tinyxml2::XMLElement *element, std::vector<std::string> &errs,
                                            SymbolNameSpace *ns) {
  // we will leave the base model completely empty and only generate the modules - letting the opening software
  // lay out the modules and mark the connections between them
  // we will also leave in anything that does not exist in any view at a reasonable level of
  // attachment
  std::vector<View *> &views = _model->Views();
  tinyxml2::XMLDocument *doc = element->GetDocument();
  if (views.size() < 2) {
    tinyxml2::XMLElement *model = doc->NewElement("model");
    generateModelAsSectors(model, errs, ns, true);
    element->InsertEndChild(model);
    return;
  }
  std::vector<Variable *> vars = _model->GetVariables(ns);  // all symbols that are variables
  // for every variable in a view, put its causes into the same view if they are not in any view
  // otherwise we let the go cross level
  for (Variable *var : vars) {
    var->SetViewOfCauses();
  }
  // and now for any variables still not assigned a view we do essentially the opposite
  for (Variable *var : vars) {
    var->SetViewToCause(5);
  }

  tinyxml2::XMLElement *mainmodel = doc->NewElement("model");
  element->InsertEndChild(mainmodel);
  tinyxml2::XMLElement *modules = doc->NewElement("variables");
  mainmodel->InsertEndChild(modules);
  int uid_off = 0;

  for (View *gview : views) {
    VensimView *view = static_cast<VensimView *>(gview);
    tinyxml2::XMLElement *submodel = doc->NewElement("model");
    submodel->SetAttribute("name", view->Title().c_str());
    element->InsertEndChild(submodel);

    tinyxml2::XMLElement *variables = doc->NewElement("variables");
    submodel->InsertEndChild(variables);
    // first we get a list of variables included in the view and all of their inputs - if an
    // input is not in the view we need to make a ghost to hold its place
    std::set<Variable *> included;
    for (Variable *var : vars) {
      if (var->Unwanted())
        continue;
      if (var->GetView() == gview)  // wanted normal
        included.insert(var);
    }
    std::set<Variable *> needed;
    for (Variable *var : included) {
      std::vector<Variable *> inputs = var->GetInputVars();
      for (Variable *input : inputs) {
        if (included.find(input) == included.end() && input->VariableType() != XMILE_Type_ARRAY &&
            input->VariableType() != XMILE_Type_ARRAY_ELM && input->VariableType() != XMILE_Type_UNKNOWN &&
            !StringMatch(input->GetName(), "Time") && !input->Unwanted()) {
          needed.insert(input);
        }
      }
    }

    // needed will be cross level
    // add this as a module to the main model
    tinyxml2::XMLElement *module = doc->NewElement("module");
    module->SetAttribute("name", view->Title().c_str());
    modules->InsertEndChild(module);
    // and mark the cross levels
    for (Variable *var : needed) {
      tinyxml2::XMLElement *connect = doc->NewElement("connect");
      std::string to = view->Title() + "." + var->GetAlternateName();
      connect->SetAttribute("to", SpaceToUnderBar(to).c_str());
      std::string from;
      if (var->GetView())
        from = static_cast<VensimView *>(var->GetView())->Title();
      from += "." + var->GetAlternateName();
      connect->SetAttribute("from", SpaceToUnderBar(from).c_str());
      module->InsertEndChild(connect);
    }
    generateEquations(included, doc, variables);
    // for the incomming cross levels they get no equations but need to be included as variables
    for (Variable *var : needed) {
      tinyxml2::XMLElement *xvar;
      if (var->VariableType() == XMILE_Type_STOCK)
        xvar = doc->NewElement("stock");
      else
        xvar = doc->NewElement("aux");

      variables->InsertEndChild(xvar);
      xvar->SetAttribute("name", var->GetAlternateName().c_str());
      xvar->SetAttribute("access", "input");
    }

    tinyxml2::XMLElement *xviews = doc->NewElement("views");
    submodel->InsertEndChild(xviews);
    tinyxml2::XMLElement *xview = doc->NewElement("view");
    if (_model->LetterPolarity())
      xview->SetAttribute("isee:use_lettered_polarity", "true");
    xviews->InsertEndChild(xview);
    uid_off = view->SetViewStart(100, 100, _xratio, _yratio, uid_off);
    this->generateView(view, xview, errs, &needed);
  }
  std::set<Variable *> remnant;
  for (Variable *var : vars) {
    if (var->GetView() == NULL && !var->Unwanted())
      remnant.insert(var);
  }
  generateEquations(remnant, doc, modules);
  // tinyxml2::XMLElement* views = doc->NewElement("views");
  // this->generateSectorViews(views, variables, errs, ns == NULL);
  // element->InsertEndChild(views);
}

void XMILEGenerator::generateSectorViews(tinyxml2::XMLElement *element, tinyxml2::XMLElement *xvars,
                                         std::vector<std::string> &errs, bool mainmodel) {
  tinyxml2::XMLDocument *doc = element->GetDocument();

  std::vector<View *> &views = _model->Views();
  if (views.empty() && mainmodel) {
    std::vector<ModelGroup> &groups = _model->Groups();
    if (!groups.empty()) {
      for (ModelGroup &group : groups) {
        tinyxml2::XMLElement *xgroup = doc->NewElement("group");
        xgroup->SetAttribute("name", group.sName.c_str());
        if (group.sOwner != group.sName)
          xgroup->SetAttribute("owner", group.sOwner.c_str());
        element->InsertEndChild(xgroup);
        for (Variable *var : group.vVariables) {
          tinyxml2::XMLElement *xvar = doc->NewElement("var");
          xvar->SetText(SpaceToUnderBar(var->GetAlternateName()).c_str());
          xgroup->InsertEndChild(xvar);
        }
      }
    }
    return;
  }
  int x, y;
  // start at a reasonable distance from 0 - the x,y values are generally around hte center
  // of the var
  x = 100;
  y = 100;
  // all the views against a single xmile view - or break up into modules - need vector of models as input to do that
  tinyxml2::XMLElement *xview = doc->NewElement("view");
  if (_model->LetterPolarity())
    xview->SetAttribute("isee:use_lettered_polarity", "true");
  element->InsertEndChild(xview);
  int uid_off = 0;
  for (View *gview : views) {
    VensimView *view = static_cast<VensimView *>(gview);
    // first update geometry - we put views one after another along the y axix - could lay out in pages or something
    uid_off = view->SetViewStart(x, y + 20, _xratio, _yratio, uid_off);
    int width = view->GetViewMaxX(100);
    int height = view->GetViewMaxY(y + 80) - y;
    // add a surrounding sector to contain this view - call it the view name
    // 				<group locked="false" x="184" y="154" width="300" height="184" name="Sector 1"/>

    if (views.size() > 1) {
      std::string name = view->Title();
      tinyxml2::XMLElement *xsectorvar = doc->NewElement("group");
      xvars->InsertEndChild(xsectorvar);
      xsectorvar->SetAttribute("name", name.c_str());
      tinyxml2::XMLElement *xsector = doc->NewElement("group");
      xview->InsertEndChild(xsector);
      xsector->SetAttribute("name", name.c_str());
      xsector->SetAttribute("x", StringFromDouble(x - 40).c_str());
      xsector->SetAttribute("y", StringFromDouble(y).c_str());
      xsector->SetAttribute("width", StringFromDouble(width + 60).c_str());
      xsector->SetAttribute("height", StringFromDouble(height + 40).c_str());
    }

    y += height + 80;

    this->generateView(view, xview, errs, NULL);
  }
}

void XMILEGenerator::generateView(VensimView *view, tinyxml2::XMLElement *element, std::vector<std::string> &errs,
                                  std::set<Variable *> *adds) {
  tinyxml2::XMLDocument *doc = element->GetDocument();
  int uid = view->UIDOffset();
  int local_uid = 0;
  VensimViewElements &elements = view->Elements();
  for (VensimViewElement *ele : elements) {
    if (ele) {
      if (ele->Type() == VensimViewElement::ElementTypeVARIABLE) {
        assert(ele->X() > 0 && ele->Y() > 0);
        VensimVariableElement *vele = static_cast<VensimVariableElement *>(ele);
        Variable *var = vele->GetVariable();
        // skip time altogether - this never shows up under xmil
        if (!var || StringMatch(vele->GetVariable()->GetName(), "Time") || var->Unwanted())
          ;  // do nothing
        else if (vele->Ghost(adds)) {
          assert(vele->GetVariable()->VariableType() != XMILE_Type_ARRAY);
          tinyxml2::XMLElement *xghost = doc->NewElement("alias");
          element->InsertEndChild(xghost);
          xghost->SetAttribute("x", vele->X());
          xghost->SetAttribute("y", vele->Y());
          if (vele->GetVariable() && vele->GetVariable()->VariableType() == XMILE_Type_STOCK) {
            xghost->SetAttribute("x", vele->X() - 22);
            xghost->SetAttribute("y", vele->Y() - 17);
            xghost->SetAttribute("width", 45);
            xghost->SetAttribute("height", 35);
          } else {
            xghost->SetAttribute("x", vele->X());
            xghost->SetAttribute("y", vele->Y());
          }
          xghost->SetAttribute("uid", uid);
          tinyxml2::XMLElement *xof = doc->NewElement("of");
          xghost->InsertEndChild(xof);
          xof->SetText(SpaceToUnderBar(vele->GetVariable()->GetAlternateName()).c_str());
        } else {
          XMILE_Type type = vele->GetVariable()->VariableType();
          std::string tag;
          switch (type) {
          case XMILE_Type_DELAYAUX:
          case XMILE_Type_AUX:
            tag = "aux";
            break;
          case XMILE_Type_STOCK:
            tag = "stock";
            break;
          case XMILE_Type_FLOW:
            tag = "flow";
            break;
          default:
            log("unknown view element type %d\n", type);
          }
          if (tag.empty())
            continue;
          tinyxml2::XMLElement *xvar = doc->NewElement(tag.c_str());

          element->InsertEndChild(xvar);

          std::string name = vele->GetVariable()->GetAlternateName();
          xvar->SetAttribute("name", SpaceToUnderBar(vele->GetVariable()->GetAlternateName()).c_str());
          if (type == XMILE_Type_FLOW && vele->Attached() && elements[local_uid - 1] &&
              elements[local_uid - 1]->Type() == VensimViewElement::ElementTypeVALVE) {
            xvar->SetAttribute("x", elements[local_uid - 1]->X());
            xvar->SetAttribute("y", elements[local_uid - 1]->Y());
          } else {
            // pretty big things - Vensim's default size is 80x40 - width and height are half vals so a fair bit bigger
            // 90x50 then bring size across
            if (type == XMILE_Type_STOCK && !vele->CrossLevel() && !vele->Ghost(NULL) &&
                (vele->Width() > 45 || vele->Height() > 25)) {
              int x = vele->X();
              int y = vele->Y();
              int width = 2 * vele->Width();
              int height = 2 * vele->Height();
              if (width < 60)
                width = 60;
              if (height < 40)
                height = 40;
              x -= width / 2;
              y -= height / 2;
              xvar->SetAttribute("x", x);
              xvar->SetAttribute("y", y);
              xvar->SetAttribute("width", width);
              xvar->SetAttribute("height", height);
            } else {
              xvar->SetAttribute("x", vele->X());
              xvar->SetAttribute("y", vele->Y());
            }
          }
          if (type == XMILE_Type_FLOW) {
            // need points - these are the location of the from and to - no matter what they are
            // but we need to search through the list of eleemnts to find the from and to - flow
            // arrows are always out of the attached value which is just before us in the list
            // flow direction we need to take from the model proper - arbitrary if flow is not connected
            size_t n = elements.size();
            int count = 0;
            int toind = -1;
            int xpt[2];
            int ypt[2];
            int xanchor[2];
            int yanchor[2];
            for (size_t i = 0; i < n; i++) {
              VensimConnectorElement *cele = static_cast<VensimConnectorElement *>(elements[i]);
              if (cele && cele->Type() == VensimViewElement::ElementTypeCONNECTOR) {
                if (cele->From() == local_uid - 1) {
                  // check to see what to is
                  VensimVariableElement *stock = static_cast<VensimVariableElement *>(elements[cele->To()]);
                  bool isgood = false;
                  if (stock) {
                    if ((stock->Type() == VensimViewElement::ElementTypeVARIABLE)) {
                      Variable *var = stock->GetVariable();
                      if (var && var->VariableType() == XMILE_Type_STOCK)
                        isgood = true;
                    } else if (stock->Type() == VensimViewElement::ElementTypeCOMMENT) {
                      isgood = true;
                    }
                  }
                  if (isgood) {
                    xpt[count] = cele->X();
                    xanchor[count] = stock->X();
                    ypt[count] = cele->Y();
                    yanchor[count] = stock->Y();
                    if (stock->Type() == VensimViewElement::ElementTypeVARIABLE) {
                      Variable *var = stock->GetVariable();
                      if (toind == -1 && var && var->VariableType() == XMILE_Type_STOCK) {
                        // are we an inflow or an outflow
                        for (Variable *inflow : var->Inflows()) {
                          if (inflow == vele->GetVariable()) {
                            toind = count;
                            break;
                          }
                        }
                        if (toind == -1) {
                          for (Variable *outflow : var->Outflows()) {
                            if (outflow == vele->GetVariable()) {
                              toind = count ? 0 : 1;
                              break;
                            }
                          }
                        }
                      }
                    }
                    count++;
                    if (count == 2)
                      break;
                  }
                }
              }
            }
            if (count < 2 || toind < 0) {
              xpt[0] = vele->X() - 150;
              xpt[1] = vele->X() + 25;
              ypt[0] = ypt[1] = vele->Y();
              toind = 1;
            } else {
              if (xpt[0] == xpt[1]) {
                // vertical put the ys at the achors
                ypt[0] = yanchor[0];
                ypt[1] = yanchor[1];
              } else {
                // horizontal put the xs at the achors
                xpt[0] = xanchor[0];
                xpt[1] = xanchor[1];
              }
            }
            tinyxml2::XMLElement *xpts = doc->NewElement("pts");
            xvar->InsertEndChild(xpts);
            tinyxml2::XMLElement *xxpt = doc->NewElement("pt");
            xpts->InsertEndChild(xxpt);
            xxpt->SetAttribute("x", xpt[1 - toind]);
            xxpt->SetAttribute("y", ypt[1 - toind]);
            xxpt = doc->NewElement("pt");
            xpts->InsertEndChild(xxpt);
            xxpt->SetAttribute("x", xpt[toind]);
            xxpt->SetAttribute("y", ypt[toind]);
          }
        }
      } else if (ele->Type() == VensimViewElement::ElementTypeCONNECTOR) {
        VensimConnectorElement *cele = static_cast<VensimConnectorElement *>(ele);
        if (cele->From() > 0 && cele->To() > 0) {
          VensimVariableElement *from = static_cast<VensimVariableElement *>(elements[cele->From()]);
          VensimVariableElement *to = static_cast<VensimVariableElement *>(elements[cele->To()]);
          if (from && to) {
            // if from is a valve we switch it to the next element in the list which should be a var
            if (from->Type() == VensimViewElement::ElementTypeVALVE &&
                static_cast<VensimValveElement *>(elements[cele->From()])->Attached()) {
              from = static_cast<VensimVariableElement *>(elements[cele->From() + 1]);
            }
            if (to->Type() == VensimViewElement::ElementTypeVALVE &&
                static_cast<VensimValveElement *>(elements[cele->To()])->Attached())
              to = static_cast<VensimVariableElement *>(elements[cele->To() + 1]);
            if (from->Type() == VensimViewElement::ElementTypeVARIABLE && to &&
                to->Type() == VensimViewElement::ElementTypeVARIABLE && to->GetVariable() &&
                to->GetVariable()->VariableType() != XMILE_Type_STOCK) {
              // valid xmile connector
              tinyxml2::XMLElement *xconnector = doc->NewElement("connector");
              element->InsertEndChild(xconnector);
              xconnector->SetAttribute("uid", uid);
              // try to figure out the angle based on the 3 points -
#ifndef NDEBUG
              double thetax = 999;
              if (to->GetVariable()->GetName() == "US crude death rate")
                thetax = AngleFromPoints(from->X(), from->Y(), cele->X(), cele->Y(), to->X(), to->Y());
#endif
              xconnector->SetAttribute("angle",
                                       AngleFromPoints(from->X(), from->Y(), cele->X(), cele->Y(), to->X(), to->Y()));
              if (cele->Polarity()) {
                char cbuf[2];
                cbuf[0] = cele->Polarity();
                cbuf[1] = 0;
                xconnector->SetAttribute("polarity", cbuf);
              }
              tinyxml2::XMLElement *xfrom = doc->NewElement("from");
              xconnector->InsertEndChild(xfrom);
              if (from->Ghost(NULL)) {
                tinyxml2::XMLElement *xalias = doc->NewElement("alias");
                xfrom->InsertEndChild(xalias);
                xalias->SetAttribute("uid", view->UIDOffset() + cele->From());
              } else if (from->GetVariable()) {
                xfrom->SetText(QuotedSpaceToUnderBar(from->GetVariable()->GetAlternateName()).c_str());
              }
              tinyxml2::XMLElement *xto = doc->NewElement("to");
              xconnector->InsertEndChild(xto);
              xto->SetText(QuotedSpaceToUnderBar(to->GetVariable()->GetAlternateName()).c_str());
            }
          }
        }
      }
    }
    uid++;
    local_uid++;
  }
}
