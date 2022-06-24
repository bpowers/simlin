#ifndef _XMUTIL_VENSIM_VENSIMLEX_H
#define _XMUTIL_VENSIM_VENSIMLEX_H

/* a tokenizer for Vensim files - it is called by VensimPare both
  indirectly throught the Bison generated parser and directly for
  comments and group defs */
#include <string>
#include <vector>

#include "../Symbol/Parse.h"

class VensimLex {
public:
  VensimLex(void);
  ~VensimLex(void);
  void Initialize(const char *content, off_t length);
  std::string *CurToken(void);
  void GetReady(void);
  int yylex(void);
  int GetEndToken(void);
  int LineNumber(void) {
    return iLineNumber;
  }
  int Position(void) {
    return iCurPos - iLineStart;
  }
  std::string GetComment(const char *tok);
  bool FindToken(const char *tok);
  bool BufferReadLine(char *buf, size_t buflen);  // start with buffer then read the line
  bool ReadLine(char *buf, size_t buflen);        // read a line if enough room otherwise part of it
private:
  char GetNextChar(bool store);
  void PushBack(char c, bool store);
  void SyncBuffers(void);
  bool TestTokenMatch(const char *tok, bool update);
  std::string sToken;
  std::string sBuffer;
  const char *ucContent;
  off_t iCurPos, iHoldPos;
  off_t iLineStart, iHoldStart;
  void MarkPosition(void) {
    iHoldPos = iCurPos;
    iHoldStart = iLineStart;
  }
  void ReturnToMark(void) {
    iCurPos = iHoldPos;
    iLineStart = iHoldStart;
    sBuffer.clear();
  }
  int iLineNumber;
  off_t iFileLength;
  int NextToken(void);
  bool IsGetXLSorVDF(void);
  bool KeywordMatch(const char *target);
  void GetDigits(void);
  int iInUnitsComment;  // 0 no, 1 units, 2 comment
  int TestColonKeyword(void);
  int ReadTabbedArray(void);
  bool bInUnits;
  bool sawExplicitEqEnd = false;
};

#endif
