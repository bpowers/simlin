#ifndef _XMUTIL_DYNAMO_DYNAMOLEX_H
#define _XMUTIL_DYNAMO_DYNAMOLEX_H

/* a tokenizer for Dynamo files - it is called by DynamoPare both
  indirectly throught the Bison generated parser and directly for
  comments and group defs */
#include <string>
#include <vector>

#include "../Symbol/Parse.h"

class Model;
class DynamoLex {
public:
  DynamoLex(void);
  ~DynamoLex(void);
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
  std::string GetComment(std::string &units);
  bool FindStartToken();                          // realistically just a line that begins with non white space
  bool BufferReadLine(char *buf, size_t buflen);  // start with buffer then read the line
  bool ReadLine(char *buf, size_t buflen);        // read a line if enough room otherwise part of it
  void ParseSpecs(Model *model);
  void ConsumeCurrentLine();

protected:
  int NextToken(void);

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
  bool KeywordMatch(const char *target);
  void GetDigits(void);
  int iInUnitsComment;  // 0 no, 1 units, 2 comment
  bool bInUnits;
  bool bInEquation;
  bool bNoSpace;         // since CR/LF - anything indented is treated as commentary outside of equations
  bool bClassicParsing;  // spaces end equations
};

#endif
