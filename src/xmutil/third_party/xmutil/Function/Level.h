#ifndef _XMUTIL_FUNCTION_LEVEL_H
#define _XMUTIL_FUNCTION_LEVEL_H

/* Level is the abstract class for functions with states
   it is subclassed from Function - but might also be treated
   in parallel */

typedef struct _tag_state_info_ {
  int size;
  int n;
} lkjkljklj;

#include "Function.h"
class Level : public Function {
public:
  Level(SymbolNameSpace *sns, const std::string &name, int nargs);
  virtual ~Level(void) = 0;
  inline double EvalEq() {
    return GetStateVal();
  }
  virtual double GetStateVal() = 0;
};

#endif