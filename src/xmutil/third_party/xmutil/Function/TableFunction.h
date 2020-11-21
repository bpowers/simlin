#ifndef _XMUTIL_TABLEFUNCTION_H
#define _XMUTIL_TABLEFUNCTION_H

class Expression;
class ExpressionVariable;
class ContextInfo;

/* table or lookup functions are used implicitly and are therefore not derived from
   Function (this it is called TableFunction not Function Table)
   nor is it derived from State

   first pass this is never instantiated - only eval is ever called
   */
class TableFunction {
public:
  TableFunction(void);
  ~TableFunction(void);
  static double Eval(ExpressionVariable *v, Expression *e, ContextInfo *info);
};

#endif
