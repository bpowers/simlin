#ifndef _XMUTIL_SYMBOL_NAMESPACE_H
#define _XMUTIL_SYMBOL_NAMESPACE_H
#include <set>
#include <string>
#include <unordered_map>
class Symbol;
class SymbolTableBase;  // forward declaration

/* Namespace gives hashed lookup for names

   to make it case and _ insensitive we convert the incoming name before
   passing it to the lookup functions - altering the hash and equality
   function would (likely) be a bit faster */

#define SNSitToSymbol(it) (it.second)
class SymbolNameSpace {
public:
  SymbolNameSpace(void);
  ~SymbolNameSpace(void);
  Symbol *Find(const std::string &name);
  void Insert(Symbol *sym);
  bool Remove(Symbol *sym);
  bool Rename(Symbol *sym, const std::string &newname);
  void DeleteAllUnconfirmedAllocations(void);
  void ConfirmAllAllocations(void);
  void RemoveUnconfirmedAllocation(SymbolTableBase *s) {
    sUnconfirmedAllocations.erase(s);
  }
  inline void AddUnconfirmedAllocation(SymbolTableBase *s) {
    sUnconfirmedAllocations.insert(s);
  }
  typedef std::unordered_map<std::string, Symbol *> HashTable;
  typedef HashTable::value_type iterator;  // allows iterator type to be used directly with C++11 range-based for loops
  inline HashTable *GetHashTable(void) {
    return &mHashTable;
  }
  static std::string *ToLowerSpace(const std::string &name);

private:
  std::set<SymbolTableBase *> sUnconfirmedAllocations;
  HashTable mHashTable;
};

#endif
