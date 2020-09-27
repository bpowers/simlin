#pragma once
#ifndef __XMILE_H
#define __XMILE_H

#include <tinyxml2.h>

#include <string>
#include <vector>

class Model;
class VensimView;
class SymbolNameSpace;

class XMILEGenerator {
public:
  XMILEGenerator(Model *model);

  std::string Print(bool isCompact, std::vector<std::string> &errs);
  bool Generate(FILE *file, std::vector<std::string> &errs);

protected:
  void generateHeader(tinyxml2::XMLElement *element, std::vector<std::string> &errs);
  void generateSimSpecs(tinyxml2::XMLElement *element, std::vector<std::string> &errs);
  void generateModelUnits(tinyxml2::XMLElement *element, std::vector<std::string> &errs);
  void generateDimensions(tinyxml2::XMLElement *element, std::vector<std::string> &errs);
  void generateModel(tinyxml2::XMLElement *element, std::vector<std::string> &errs, SymbolNameSpace *ns);
  void generateViews(tinyxml2::XMLElement *views, tinyxml2::XMLElement *vars, std::vector<std::string> &errs,
                     bool mainmodel);
  void generateView(VensimView *view, tinyxml2::XMLElement *element, std::vector<std::string> &errs);

private:
  Model *_model;
};

#endif
