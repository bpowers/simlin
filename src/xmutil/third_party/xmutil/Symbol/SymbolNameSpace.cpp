#include "SymbolNameSpace.h"

#include <assert.h>
#include <string.h>

#include "../Unicode.h"
#include "Symbol.h"

SymbolNameSpace::SymbolNameSpace(void) {
  assert(sUnconfirmedAllocations.size() == 0);
}

SymbolNameSpace::~SymbolNameSpace(void) {
  /* delete the symbols which will in turn delete equations etc */
  // for (iterator node: mHashTable) {
  //    delete SNSitToSymbol(node) ;
  // }
}

Symbol *SymbolNameSpace::Find(const std::string &sin) {
  std::string *s = ToLowerSpace(sin);
  HashTable::iterator node = mHashTable.find(*s);
  delete s;
  if (node != mHashTable.end())
    return node->second;
  return NULL;
}

void SymbolNameSpace::Insert(Symbol *sym) {
  std::string *s = ToLowerSpace(sym->GetName());
  HashTable::iterator node = mHashTable.find(*s);
  if (node != mHashTable.end()) {
    assert(node->second == sym);
    delete s;
    return; /* already in place */
  }
  mHashTable[*s] = sym;
  delete s;
}

bool SymbolNameSpace::Remove(Symbol *sym) {
  std::string *s = ToLowerSpace(sym->GetName());
  HashTable::iterator node = mHashTable.find(*s);
  delete s;
  if (node != mHashTable.end()) {
    mHashTable.erase(node);
    return true; /* already in place */
  }
  return false;
}

bool SymbolNameSpace::Rename(Symbol *sym, const std::string &newname) {
  std::string *s = ToLowerSpace(sym->GetName());
  HashTable::iterator oldnode = mHashTable.find(*s);
  delete s;
  std::string *s2 = ToLowerSpace(newname);
  HashTable::iterator newnode = mHashTable.find(*s2);
  if (oldnode != mHashTable.end() && newnode == mHashTable.end()) {
    mHashTable.erase(oldnode);
    mHashTable[*s2] = sym;
    delete s2;
    sym->SetName(newname);
    return true; /* already in place */
  }
  delete s2;
  return false;
}

// Note GlobalUCaseMap has to have been set for this to wok -
// not sure if that is thread safe
//
// also note that this works on escaped strings - but does not validate
// escaping beyond stripping a leading " matched to a terminal " (that is
// "this is "not a good" string" would become
// this is "not a good" string
// which is invalid
//
std::string *SymbolNameSpace::ToLowerSpace(const std::string &sin) {
  int n = sin.length();
  char *ws = new char[2 * n + 2];
  memcpy(ws, sin.c_str(), n);
  if (*ws == '\"' && ws[n - 1] == '\"' && n > 1) {
    memcpy(ws, sin.c_str() + 1, n - 2);
    n -= 2;
  }
  ws[n] = '\0';

  int i, j;
  for (i = j = 0; i < n; i++)  // remove leading blanks
    if (ws[i] != ' ' && ws[i] != '_' && ws[i] != '\t' && ws[i] != '\n' && ws[i] != '\r')
      break;
  // convert underbars to blanks and compact
  for (; i < n; i++) {
    if (ws[i] == '\\' && i < n - 1 && ws[i + 1] == '_') {
      ws[j++] = '\\';
      ws[j++] = '_';
      i++;  // hard underbar treat as a nonspace character
    } else if (ws[i] == '_' || ws[i] == ' ' || ws[i] == '\t' || ws[i] == '\n' || ws[i] == '\r') {
      for (; i < n - 1; i++) {
        if (ws[i + 1] != ' ' && ws[i + 1] != '_' && ws[i + 1] != '\t' && ws[i + 1] != '\n' && ws[i + 1] != '\r')
          break;
      }
      ws[j++] = ' ';
    } else
      ws[j++] = ws[i];
  }
  for (; j > 0; j--) {
    if (ws[j - 1] != ' ' && ws[j - 1] != '_' && ws[j - 1] != '\t' && ws[j - 1] != '\n' && ws[j - 1] != '\r')
      break;
  }
  ws[j] = '\0';
  char *ws2 = utf8ToLower(ws, j);
  if (ws2 == nullptr) {
    throw "Bad unicode string";
  }
  std::string *s = new std::string(ws2);
  delete[] ws;
  delete[] ws2;
  return s;
}

void SymbolNameSpace::DeleteAllUnconfirmedAllocations(void) {
  std::set<SymbolTableBase *>::iterator i;
  while ((i = sUnconfirmedAllocations.begin()) != sUnconfirmedAllocations.end()) {
    SymbolTableBase *s = *i;
    sUnconfirmedAllocations.erase(i);
    delete (s);
  }
  sUnconfirmedAllocations.clear();
}

void SymbolNameSpace::ConfirmAllAllocations(void) {
  for (std::set<SymbolTableBase *>::value_type i : sUnconfirmedAllocations) {
    i->MarkGoodAlloc();
  }
  sUnconfirmedAllocations.clear();
}
