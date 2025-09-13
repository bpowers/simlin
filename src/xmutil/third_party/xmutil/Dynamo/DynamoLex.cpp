/* DynamoLex -
 */
#include "DynamoLex.h"

#include "DynamoParse.h"
/* try to avoid the tab.h file as it is C  */
#define YYSTYPE ParseUnion
#include "../Model.h"
#include "../Symbol/Expression.h"
#include "../Symbol/Variable.h"
#include "../XMUtil.h"
#include "DYacc.tab.hpp"

DynamoLex::DynamoLex(void) {
  ucContent = NULL;
  iCurPos = iFileLength = 0;
  bInEquation = false;
  bNoSpace = true;
  bClassicParsing = true;
  GetReady();
}
DynamoLex::~DynamoLex() {
}
void DynamoLex::Initialize(const char *content, off_t length) {
  ucContent = content;
  iFileLength = length;
  iLineStart = iCurPos = 0;
  iLineNumber = 1;
  GetReady();
  unsigned char c = GetNextChar(false);
  if (c == 0xEF) {
    c = GetNextChar(false);
    if (c == 0xBB) {
      c = GetNextChar(false);
      if (c != 0xBF) {
        if (c)
          this->PushBack(c, false);
        this->PushBack(0xBF, false);
        this->PushBack(0xBB, false);
      }
    } else {
      if (c)
        this->PushBack(c, false);
      this->PushBack(0xBB, false);
    }
  } else if (c)
    this->PushBack(c, false);
}
int DynamoLex::GetEndToken(void) {
  return DPTT_eoq;
}

void DynamoLex::GetReady(void) {
  iInUnitsComment = 0;
  bInUnits = false;
}
std::string *DynamoLex::CurToken() {
  return &sToken;
}

int DynamoLex::yylex() {
  int toktype = NextToken();
  // printf("Token type %d %s\n", toktype, sToken.c_str());
  switch (toktype) {
  case DPTT_number:
    dpyylval.num = atof(sToken.c_str());
    break;
  case DPTT_symbol:
    if (bInUnits) {
      Units *units = DPObject->InsertUnits(sToken);
      if (!units) {
        // InsertUnits failed - return end token to stop parsing
        return DPTT_eoq;
      }
      dpyylval.uni = DPObject->InsertUnitExpression(units);
      toktype = DPTT_units_symbol;
      break;
    }
    // special things here - try to do almost everything (including INTEG) as a function but some need to call out to
    // different toktypes
    dpyylval.sym = DPObject->InsertVariable(sToken);
    if (!dpyylval.sym) {
      // InsertVariable failed - return end token to stop parsing
      return DPTT_eoq;
    }
    if (dpyylval.sym->isType() == Symtype_Function) {
      toktype = DPTT_function;
    }

    break;
  case 0:
    toktype = DPTT_eoq;
    // Fall through to also clear bInEquation flag when we hit end of input
    [[fallthrough]];
  case DPTT_eoq:
    bInEquation = false;
    break;
  default:
    break;
  }
  return toktype;
}

void DynamoLex::GetDigits() {
  char c;
  while (1) {
    c = GetNextChar(true);
    if (c < '0' || c > '9') {
      PushBack(c, true);
      break;
    }
  }
}

int DynamoLex::NextToken()  // also sets token type
{
  unsigned char c;
  int toktype;

  while (true) {
    c = GetNextChar(false);
    if (c == '\n' || c == '\r' || c == '\f')
      bNoSpace = true;
    else if (c == ' ' || c == '\t')
      bNoSpace = false;  // consume whitespace
    else
      break;
    if (bInEquation && bClassicParsing)
      return DPTT_eoq;
  }
  if (!c)
    return 0;
  sToken.clear();
  PushBack(c, false);
  c = GetNextChar(true);

  toktype = c;  // default for many tokens
  switch (c) {
  case '*':  // check for *** which names groups - we are dropping group info for now
    if (!bInEquation) {
      sToken.pop_back();
      assert(sToken.empty());
      while (true) {
        c = GetNextChar(true);
        if (c != '*')
          break;
      }
      if (c)
        PushBack(c, true);
      do {
        c = GetNextChar(false);
      } while (c == ' ' || c == '\t');  // consume whitespace
      if (c)
        PushBack(c, false);
      // the treat everything till end of line as the group name - it is is empty call it so
      do {
        c = GetNextChar(true);
      } while (c != '\n' && c != '\r');  // consume whitespace
      if (c)
        PushBack(c, true);
      return DPTT_groupstar;
    }
    break;
  // single character tokens
  case '~':
    if (iInUnitsComment == 0)
      bInUnits = true;
    iInUnitsComment++;
    break;
  case '=':  // := is handled by :
    if (TestTokenMatch("=", true))
      return '=';  // we ignore the invariant == that Dynamo supports
    break;
  case '/':
    break;
  case '^':
    break;
  case '!':
    break;
  case '(':
    break;
  case ')':
    break;
  case '}':
    break;
  case '[':
    if (iInUnitsComment)
      bInUnits = false;
  case ']':
    break;
  case '|':
    break;
  case ',':
    break;
  case '+':
    break;
  case '>':
    if (TestTokenMatch("=", true))
      return DPTT_ge;
    break;
  case '-':
    break;
  case '<':
    if (TestTokenMatch("=", true))
      return DPTT_le;
    if (TestTokenMatch(">", true))
      return DPTT_ne;
    break;
  case '1':
    if (bInUnits) {
      Units *units = DPObject->InsertUnits("1");
      if (!units) {
        // InsertUnits failed - return end token to stop parsing
        return DPTT_eoq;
      }
      dpyylval.uni = DPObject->InsertUnitExpression(units);
      return DPTT_units_symbol;
    }
    /* fallthrough */
  case '.':  // maybe a number check next digit
  case '0':

  case '2':
  case '3':
  case '4':
  case '5':
  case '6':
  case '7':
  case '8':
  case '9':
    // a number follow it through till it is no longer a number
    if (c == '.') {
      c = GetNextChar(true);
      if (c < '0' || c > '9') {
        PushBack(c, true);
        break;  // not a number return '.'
      }
      GetDigits();
    } else {
      GetDigits();
      c = GetNextChar(true);
      if (c != '.')
        PushBack(c, true);
    }
    toktype = DPTT_number;
    if (c == '.')
      GetDigits();
    c = GetNextChar(true);
    if (c == 'E' || c == 'e') {  // xxx.xxxE+-xx
      c = GetNextChar(true);
      if (c != '+' && c != '-')
        PushBack(c, true);
      GetDigits();
    } else
      PushBack(c, true);
    break;
  case ':':  // := :AND:  :HOLD BACKWARD: :IMPLIES: :INTERPOLATE: :LOOK FORWARD: :OR: :NA:  :NOT: :RAW: :TEST INPUT:
             // :THE CONDITION:
    break;
  case '{':  // a comment, find matching }
  {          // nesting, len local in scope
    int nesting = 1;
    int len = 1;
    MarkPosition();
    while ((c = GetNextChar(false))) {
      len++;
      if (len > 1028)
        break;  // excessive comments not considered valid
      if (c == '}') {
        nesting--;
        if (!nesting) {        // comment is in sToken right now
          return NextToken();  // just skip the comment for now - as if not there
        }
      } else if (c == '{')
        nesting++;
      else if (c == '*' && nesting == 1) {
        c = GetNextChar(false);
        if (c == '*')  // treat this as a group - from the original dynamo format
        {
          while ((c = GetNextChar(false)) == '*')
            ;
          while (c == '\r' || c == '\n' || c == ' ' || c == '\t')
            c = GetNextChar(false);
          if (c == '}')
            return NextToken();  // no useful group name
          sToken.clear();
          do {
            this->sToken.push_back(c);
            if (c == '.') {
              sToken.pop_back();
              if (!sToken.empty())
                sToken.push_back('-');  // can't use . in a module name
            }
            c = GetNextChar(false);
          } while (c != '\r' && c != '\n' && c != '*' && c != '}');
          while (sToken.back() == ' ')
            sToken.pop_back();
          while (c && c != '}')
            c = GetNextChar(false);
          return DPTT_groupstar;
        } else
          PushBack(c, false);
      }
    }
    ReturnToMark();  // failed to find pair give up
  } break;           // give up and just return the one char - will throw error message
  case '\'':
    break;
  case '\"':  // a quoted variable name potentially with embedded escaped quotes
  {
    int len;
    MarkPosition();
    for (len = 1; (c = GetNextChar(true)); len++) {
      if (c == '\"') {
        return DPTT_symbol;    // the returned token includes both the opening and closing quote
      } else if (c == '\\') {  // skip what follows in case it is a " or \ --
        GetNextChar(true);
        len++;
      }
      if (len > 1024)  // apparently unmmatched
        break;
    }
  }
    ReturnToMark();
    break;    // give up and just return the one char
  case '\\':  //
    break;
  default:  // a variable name or an unrecognizable token
    if (!bInEquation && bNoSpace) {
      int rtype = 0;
      switch (c) {
      case 'L':
      case 'l':
        rtype = DPTT_level;
        break;
      case 'T':
      case 't':
        rtype = DPTT_table;
        break;
      case 'A':
      case 'a':
        rtype = DPTT_aux;
        break;
      case 'C':
      case 'c':
        rtype = DPTT_constant;
        break;
      case 'N':
      case 'n':
        rtype = DPTT_init;
        break;
      case 'S':
      case 's':
      case 'P':
      case 'p': {
        // SPEC/SAVE/PRINT/PLOT
        char c1 = GetNextChar(true);
        char c2 = GetNextChar(true);
        char c3 = GetNextChar(true);
        int rval = 0;
        if ((c1 == 'P' || c1 == 'p') && (c2 == 'E' || c2 == 'e') && (c3 == 'C' || c3 == 'c'))
          rval = DPTT_specs;
        else if ((c1 == 'A' || c1 == 'a') && (c2 == 'V' || c2 == 'v') && (c3 == 'E' || c3 == 'e'))
          rval = DPTT_save;
        else if ((c1 == 'R' || c1 == 'r') && (c2 == 'I' || c2 == 'i') && (c3 == 'N' || c3 == 'n'))
          rval = DPTT_save;
        else if ((c1 == 'L' || c1 == 'l') && (c2 == 'O' || c2 == 'o') && (c3 == 'T' || c3 == 't'))
          rval = DPTT_save;
        if (rval) {
          do {
            c = GetNextChar(false);
          } while (c == ' ' || c == '\t');
          PushBack(c, false);
          return rval;
        }
        PushBack(c3, true);
        PushBack(c2, true);
        PushBack(c1, true);
        PushBack(c, true);
      }
      }
      if (rtype) {
        bInEquation = true;
        // and get rid of any blank space before the real start of the equation
        do {
          c = GetNextChar(false);
        } while (c == ' ' || c == '\t' || c == '\r' || c == '\n');
        if (c)
          PushBack(c, false);
        return rtype;
      }
    }

    if (isalpha(c) || c > 127 || ((iInUnitsComment == 1) && c == '$')) {  // a variable
      while ((c = GetNextChar(true))) {
        if (bClassicParsing && bInEquation && (c == ' ' || c == '\t')) {
          PushBack(c, true);
          break;
        }
        if (!isalnum(c) && c != ' ' && c != '_' && c != '$' && c != '\t' && c != '\'' && c < 128) {
          PushBack(c, true);
          if (c == '.') {
            // we alloc .j .k .jk and .kl everywhere by simply stripping them
            c = GetNextChar(false);
            c = GetNextChar(false);
            if (c == 'j' || c == 'J' || c == 'k' || c == 'K') {
              unsigned char c2 = GetNextChar(false);
              if (c2 != 'j' && c2 != 'J' && c2 != 'k' && c2 != 'K')
                PushBack(c2, false);
            } else {
              PushBack(c, false);
              PushBack('.', false);
            }
          }
          break;
        }
      }
      // strip any terminal spaces
      while (sToken.back() == ' ' || sToken.back() == '_')
        sToken.pop_back();
      return DPTT_symbol;
    }
  }
  return toktype;
}

// target assumed upper case
bool DynamoLex::KeywordMatch(const char *target) {
  std::string buffer;
  char c;
  int i;
  for (i = 0; target[i]; i++) {
    c = GetNextChar(true);
    if (target[i] == ' ') { /* one or more _ or space ok here */
      if (c != ' ' && c != '_' && c != '\t')
        break;
      buffer.push_back(c);
      while ((c = GetNextChar(true))) {
        if (c != ' ' && c != '_' && c != '\t')
          break;
        buffer.push_back(c);
      }
      PushBack(c, true);
    } else if (toupper(c) != target[i])
      break;
    else
      buffer.push_back(c);
  }
  if (target[i]) {      // not a match
    PushBack(c, true);  //  last one taken should be sent back
    while (buffer.length()) {
      PushBack(buffer.back(), true);
      buffer.pop_back();
    }
    return false;
  }
  return true;
}

char DynamoLex::GetNextChar(bool store) {
  char c;
  if (sBuffer.length()) {
    c = sBuffer.back();
    sBuffer.pop_back();
    if (store)
      sToken.push_back(c);
    return c;
  }
  if (iCurPos >= iFileLength)
    return 0;  // nothing to do
  c = ucContent[iCurPos++];
  if (c == '\\') {  // check for continuation lines
    if (iCurPos < iFileLength && (ucContent[iCurPos] == '\n' || ucContent[iCurPos] == '\r')) {
      for (; iCurPos < iFileLength;) {
        c = ucContent[iCurPos++];
        if (c == '\n') {
          iLineNumber++;
          iLineStart = iCurPos + 1;  // actually the next pos
        } else if (c != '\t' && c != ' ' && c != '\r')
          break;
        // note as in vensim two \ line ends in a row just cause an error
      }
    }
  } else if (c == '\n') {
    iLineNumber++;
    iLineStart = iCurPos;  // actually the next pos
  }
  if (store)
    sToken.push_back(c);
  return c;
}

// check for token match - advance position on success
bool DynamoLex::TestTokenMatch(const char *tok, bool storeonsuccess) {
  char c;
  if (!*tok)
    return true;
  int i;
  for (i = 0; tok[i]; i++) {
    if ((c = GetNextChar(storeonsuccess)) != tok[i])
      break;
  }
  if (tok[i]) {
    PushBack(c, storeonsuccess);
    while (i-- > 0)
      PushBack(tok[i], storeonsuccess);
    return false;
  }
  return true;
}

static void trim_ends(std::string &comment) {
  size_t i;
  for (i = 0; i < comment.length(); i++) {
    char c = comment[i];
    if (c != ' ' && c != '\t' && c != '\r' && c != '\n')
      break;
  }
  int start = i;
  for (i = comment.length(); i-- > 0;) {
    char c = comment[i];
    if (c != ' ' && c != '\t' && c != '\r' && c != '\n')
      break;
  }
  comment = comment.substr(start, i + 1 - start);
}

// todo figure out units
std::string DynamoLex::GetComment(std::string &units) {
  char c;
  std::string comment;
  bNoSpace = false;
  while (true) {
    c = GetNextChar(false);
    if (c == '\r' || c == '\n')
      bNoSpace = true;
    else if (c == ' ' || c == '\t')
      bNoSpace = false;
    else if (bNoSpace || !c) {
      if (c)
        PushBack(c, false);  // next call to findToken will find this
      // strip trailing white space
      trim_ends(comment);
      if (comment.back() == ')')  // maybe only do this for classic?
      {
        int nesting = 0;
        int pos = comment.length();
        while (pos-- > 0) {
          char c = comment[pos];
          if (c == ')')
            nesting++;
          else if (c == '(') {
            nesting--;
            if (nesting == 0) {
              comment.pop_back();
              units = comment.substr(pos + 1);
              comment = comment.substr(0, pos);
              trim_ends(comment);
              trim_ends(units);
            }
          }
        }
      }
      return comment;
    }
    if (bClassicParsing) {
      if (c == ' ' || c == '\t' || c == '\r' || c == '\n') {
        if (comment.back() != ' ')
          comment.push_back(' ');
      } else
        comment.push_back(c);
    } else
      comment.push_back(c);
  }
  return comment;  // this is never reached
}

bool DynamoLex::FindStartToken() {
  char c;
  while ((c = GetNextChar(false))) {
    if (c == '\r' || c == '\n')
      bNoSpace = true;
    else if (c == ' ' || c == '\t')
      bNoSpace = false;
    else {
      PushBack(c, false);
      return true;
    }
  }
  return false;
}

bool DynamoLex::BufferReadLine(char *buf, size_t buflen) {
  const char *tv = sBuffer.c_str();
  while (buflen > 0 && *tv) {
    *buf = *tv;
    buf++;
    buflen--;
    tv++;
  }
  return ReadLine(buf, buflen);
}
bool DynamoLex::ReadLine(char *buf, size_t buflen) {
  char c;
  buflen--;  // need \0
  size_t off = 0;
  while (iCurPos < iFileLength) {
    c = ucContent[iCurPos++];
    if (off >= buflen) {
      iCurPos--;
      buf[off] = '\0';
      return true;
    }
    if (c == '\n') {
      buf[off] = '\0';
      if (iCurPos < iFileLength && ucContent[iCurPos] == '\r')
        iCurPos++;
      return true;
    }
    if (c == '\r') {
      buf[off] = '\0';
      if (iCurPos < iFileLength && ucContent[iCurPos] == '\n')
        iCurPos++;
      return true;
    }
    buf[off++] = c;
  }
  buf[0] = '\0';
  return false;
}

void DynamoLex::PushBack(char c, bool store) {
  assert(c);
  sBuffer.push_back(c);
  if (store)
    sToken.pop_back();
}
void DynamoLex::SyncBuffers(void) {
  int i = sBuffer.length();
  iCurPos -= i;
  sBuffer.clear();
}

void DynamoLex::ParseSpecs(Model *model) {
  // expect SAVPER=2/Length=200/DT=.0078125/Rel_Err=.01 - try to get lenght and Dt
  if (model == NULL) {
    while (true) {
      char c = GetNextChar(false);
      if (c == '\r' || c == '\n') {
        PushBack(c, false);
        return;
      } else if (!c)
        return;
    }
    return;  // not reeached
  }
  bInEquation = true;
  bool oldclassic = bClassicParsing;
  bClassicParsing = true;
  // Spec SAVPER=2/Length=200/DT=.0078125/Rel_Err=.01
  int tok = DPTT_symbol;
  while (tok && tok != DPTT_eoq) {
    tok = NextToken();
    if (tok == DPTT_symbol) {
      std::string *name = SymbolNameSpace::ToLowerSpace(sToken);
      if (name) {
        tok = NextToken();
        if (tok == '=') {
          tok = NextToken();
          if (tok == DPTT_number) {
            double val = atof(sToken.c_str());
            if (*name == "dt")
              model->set_dt(val);
            else if (*name == "length") {
              model->set_initial_time(0);
              model->set_dt(val);
            }
          }
          delete name;
        }
      }
    }
  }
  assert(bInEquation);
  bInEquation = false;
  bClassicParsing = oldclassic;
}

void DynamoLex::ConsumeCurrentLine() {
  char c;
  do {
    c = GetNextChar(false);
  } while (c && c != '\r' && c != '\n');
  bInEquation = false;
}