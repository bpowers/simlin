/* VensimLex -
 */
#include "VensimLex.h"

#include "VensimParse.h"
/* try to avoid the tab.h file as it is C  */
#define YYSTYPE ParseUnion
#include "../Symbol/Expression.h"
#include "../Symbol/Variable.h"
#include "../XMUtil.h"
#include "VYacc.tab.hpp"

VensimLex::VensimLex(void) {
  ucContent = NULL;
  iCurPos = iFileLength = 0;
  GetReady();
}
VensimLex::~VensimLex() {
}
void VensimLex::Initialize(const char *content, off_t length) {
  ucContent = content;
  iFileLength = length;
  iLineStart = iCurPos = 0;
  iLineNumber = 1;
  GetReady();
}
int VensimLex::GetEndToken(void) {
  return VPTT_eqend;
}

void VensimLex::GetReady(void) {
  iInUnitsComment = 0;
  bInUnits = false;
}
std::string *VensimLex::CurToken() {
  return &sToken;
}

int VensimLex::yylex() {
  int toktype = NextToken();
  switch (toktype) {
  case VPTT_literal:
    vpyylval.lit = sToken.c_str();
    break;
  case VPTT_number:
    vpyylval.num = atof(sToken.c_str());
    break;
  case VPTT_symbol:
    if (bInUnits) {
      vpyylval.uni = VPObject->InsertUnitExpression(VPObject->InsertUnits(sToken));
      toktype = VPTT_units_symbol;
      break;
    }
    // special things here - try to do almost everything (including INTEG) as a function but some need to call out to
    // different toktypes
    if ((sToken[0] == 'w' || sToken[0] == 'W') && (sToken[1] == 'i' || sToken[1] == 'I') &&
        (sToken[2] == 't' || sToken[2] == 'T') && (sToken[3] == 'h' || sToken[3] == 'H') &&
        (sToken[4] == ' ' || sToken[4] == ' ') && (sToken[5] == 'l' || sToken[5] == 'L') &&
        (sToken[6] == 'o' || sToken[6] == 'O') && (sToken[7] == 'o' || sToken[7] == 'O') &&
        (sToken[8] == 'k' || sToken[8] == 'K') && (sToken[9] == 'u' || sToken[9] == 'U') &&
        (sToken[10] == 'p' || sToken[10] == 'P')) {
      toktype = VPTT_with_lookup;
    } else {
      vpyylval.sym = VPObject->InsertVariable(sToken);
      if (vpyylval.sym->isType() == Symtype_Function) {
        Function *f = static_cast<Function *>(static_cast<Symbol *>(vpyylval.sym));
        if (f->AsKeyword()) {
          return ReadTabbedArray();  // todo other keywords - this will return an ExpressionNumberTable
        } else
          toktype = VPTT_function;
      }
    }

    break;
  default:
    break;
  }
  return toktype;
}

void VensimLex::GetDigits() {
  char c;
  while (1) {
    c = GetNextChar(true);
    if (c < '0' || c > '9') {
      PushBack(c, true);
      break;
    }
  }
}

int VensimLex::ReadTabbedArray(void) {
  char c;
  int row;
  int toktype;
  do {
    c = GetNextChar(false);
  } while (c && c != '(' && c != '~');
  if (c != '(') {
    this->PushBack(c, false);
    return c;
  }
  // then just numbers - tab or space separated with new lines
  ExpressionNumberTable *ent = new ExpressionNumberTable(VPObject->GetSymbolNameSpace());
  row = 0;
  while ((toktype = NextToken())) {
    if ((toktype == '+' || toktype == '-')) {
      if (NextToken() == VPTT_number) {
        vpyylval.num = -vpyylval.num;
        toktype = VPTT_number;
      } else
        throw "Bad numbers";
    }
    if (toktype == ')') {  // finished
      vpyylval.exn = ent;
      return VPTT_tabbed_array;
    }
    if (toktype != VPTT_number)
      throw "Bad numbers";
    vpyylval.num = atof(sToken.c_str());
    ent->AddValue(row, vpyylval.num);
    // test for \n
    while ((c = GetNextChar(false))) {
      if (c == '\n') {
        row++;
        break;
      }
      if (c == '\r') {
        c = GetNextChar(false);
        if (c != '\n') {
          PushBack(c, false);
        }
        row++;
        break;
      }
      if (c != '\t' && c != '\r' && c != ' ') {
        PushBack(c, false);
        break;
      }
    }
  }
  return 0;
}

int VensimLex::NextToken()  // also sets token type
{
  unsigned char c;
  int toktype;

  do {
    c = GetNextChar(false);
  } while (c == ' ' || c == '\t' || c == '\n' || c == '\r');  // consume whitespace
  if (!c) {
    if (sawExplicitEqEnd) {
      return 0;
    }
    // if we're at the end of the buffer and we didn't see the marker for equations ending,
    // pretend we did (a bunch of mdl files from e.g. sdeverywhere and pysd are hand-written
    // as just-the-equation/no diagram files).
    return GetEndToken();
  }
  sToken.clear();
  PushBack(c, false);
  c = GetNextChar(true);

  toktype = c;  // default for many tokens
  switch (c) {
  case '*':  // check for *** which names groups - we are dropping group info for now
    if (TestTokenMatch("**", false)) {
      // look for
      // ****
      // groupname.nested.with.d
      // ****
      sToken.clear();
      while ((c = GetNextChar(false)) == '*')
        ;
      while (c == '\r' || c == '\n' || c == ' ' || c == '\t')
        c = GetNextChar(false);
      do {
        this->sToken.push_back(c);
        if (c == '.') {
          sToken.pop_back();
          if (!sToken.empty())
            sToken.push_back('-');  // can't use . in a module name
        }
        c = GetNextChar(false);
      } while (c != '\r' && c != '\n' && c != ' ' && c != '\t');
      while ((c = GetNextChar(false)) != '*' && c != '|')
        ;
      while (c && c != '|')
        c = GetNextChar(false);
      return VPTT_groupstar;
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
      return '=';  // we ignore the invariant == that Vensim supports
    break;
  case '/':
    // either / or ///---\\\      but skip last \ because continuation line chars will mess thigns up xxx
    if (TestTokenMatch("//---\\\\", false)) {
      assert(!sBuffer.length());
      sBuffer = "///---\\\\";
      iCurPos--;  // back up - wont be a problem with continuation lines in this case
      sawExplicitEqEnd = true;
      return VPTT_eqend;  // finished normal parse
    }
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
      return VPTT_ge;
    break;
  case '-':
    if (TestTokenMatch(">", true))
      return VPTT_map;
    break;
  case '<':
    if (TestTokenMatch("->", true))
      return VPTT_equiv;
    if (TestTokenMatch("=", true))
      return VPTT_le;
    if (TestTokenMatch(">", true))
      return VPTT_ne;
    break;
  case '1':
    if (bInUnits) {
      vpyylval.uni = VPObject->InsertUnitExpression(VPObject->InsertUnits("1"));
      return VPTT_units_symbol;
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
    } else {
      GetDigits();
      c = GetNextChar(true);
      if (c != '.')
        PushBack(c, true);
    }
    toktype = VPTT_number;
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
    c = GetNextChar(true);
    if (c == '=')
      return VPTT_dataequals;
    PushBack(c, true);
    return TestColonKeyword();  // might return ':'
  case '{':                     // a comment, find matching }
  {                             // nesting, len local in scope
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
          return VPTT_groupstar;
        } else
          PushBack(c, false);
      }
    }
    ReturnToMark();  // failed to find pair give up
  } break;           // give up and just return the one char - will throw error message
  case '\'':         // vensim literal - just look for matching '
  {
    int len;
    for (len = 1; (c = GetNextChar(true)); len++) {
      if (c == '\'') {
        return VPTT_literal;  // the returned token includes both the opening and closing quote
      }
    }
  } break;
  case '\"':  // a quoted variable name potentially with embedded escaped quotes
  {
    int len;
    MarkPosition();
    for (len = 1; (c = GetNextChar(true)); len++) {
      if (c == '\"') {
        return VPTT_symbol;    // the returned token includes both the opening and closing quote
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
  case '\\':  // either \\\---/// or a continuation line or an error
    if (TestTokenMatch("\\\\---///", false)) {
      assert(!sBuffer.length());
      sBuffer = "\\\\\\---///";
      iCurPos--;  // back up - wont be a problem with continuation lines in this case
      sawExplicitEqEnd = true;
      return VPTT_eqend;  // finished normal parse
    }
    break;
  default:  // a variable name or an unrecognizable token
    if (c == 'G' || c == 'g') {
      // the GET XLS functins don't really translat so we return 0 and the entire expression as a comment
      if (IsGetXLSorVDF())
        return VPTT_symbol;
    }
    if (isalpha(c) || c > 127 || ((iInUnitsComment == 1) && c == '$')) {  // a variable
      while ((c = GetNextChar(true))) {
        if (!isalnum(c) && c != ' ' && c != '_' && c != '$' && c != '\t' && c != '\'' && c < 128) {
          PushBack(c, true);
          break;
        }
      }
      // strip any terminal spaces
      while (sToken.back() == ' ' || sToken.back() == '_')
        sToken.pop_back();
      return VPTT_symbol;
    }
  }
  return toktype;
}

int VensimLex::TestColonKeyword() {
  // := :AND:  :HOLD BACKWARD: :IMPLIES: :INTERPOLATE: :LOOK FORWARD: :OR: :NA:  :NOT: :RAW: :TEST INPUT: :THE
  // CONDITION:

  const char *keywords[] = {
      ":AND:",   ":END OF MACRO:", ":EXCEPT:", ":HOLD BACKWARD:", ":IMPLIES:", ":INTERPOLATE:", ":LOOK FORWARD:",
      ":MACRO:", ":OR:",           ":NA:",     ":NOT:",           ":RAW:",     ":TESTINPUT:",   ":THECONDITION:",
      NULL};

  int keyvals[] = {VPTT_and,
                   VPTT_end_of_macro,
                   VPTT_except,
                   VPTT_hold_backward,
                   VPTT_implies,
                   VPTT_interpolate,
                   VPTT_look_forward,
                   VPTT_macro,
                   VPTT_or,
                   VPTT_na,
                   VPTT_not,
                   VPTT_raw,
                   VPTT_test_input,
                   VPTT_the_condition,
                   -1};
  int i;
  char c = GetNextChar(true);
  for (i = 0; keywords[i]; i++) {
    if (toupper(c) == keywords[i][1]) {
      if (KeywordMatch(keywords[i] + 2))
        return keyvals[i];
    }
  }
  PushBack(c, true);
  return ':';
}

// treat the GET XLS functions as varialbles to make it easier to figure out what to do on the other side
bool VensimLex::IsGetXLSorVDF() {
  // Vensim has a bunch of GET functions that use strings and aren't worth translating
  if (KeywordMatch("ET 123"))
    sToken = "{GET 123";
  else if (KeywordMatch("ET DATA"))
    sToken = "{GET DATA";
  else if (KeywordMatch("ET DIRECT"))
    sToken = "{GET DIRECT";
  else if (KeywordMatch("ET VDF"))
    sToken = "{GET VDF";
  else if (KeywordMatch("ET XLS"))
    sToken = "{GET XLS";
  else
    return false;
  char c;
  while ((c = GetNextChar(true))) {
    if (c == '(')
      break;
  }
  int nesting = 1;
  while (nesting && (c = GetNextChar(true))) {
    if (c == '(')
      nesting++;
    else if (c == ')')
      nesting--;
  }
  sToken.push_back('}');
  return true;
}

// target assumed upper case
bool VensimLex::KeywordMatch(const char *target) {
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

char VensimLex::GetNextChar(bool store) {
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
bool VensimLex::TestTokenMatch(const char *tok, bool storeonsuccess) {
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

std::string VensimLex::GetComment(const char *tok) {
  char c;
  std::string rval;
  while ((c = GetNextChar(false))) {
    if (c == *tok && TestTokenMatch(tok + 1, true)) {
      PushBack(c, false);  // next call to findToken will find this
      // strip trailing white space
      while (!rval.empty()) {
        c = rval.back();
        if (c != ' ' && c != '\t' && c != '\r' && c != '\n')
          break;
        rval.pop_back();
      }
      return rval;
    } else if (c == '\\' && TestTokenMatch("\\\\---///", false)) {
      PushBack(c, false);
      SyncBuffers();
      return rval;  // an error
    }
    rval.push_back(c);
  }
  return rval;  // this is an error condition
}

bool VensimLex::FindToken(const char *tok) {
  char c;
  while ((c = GetNextChar(false))) {
    if (c == *tok && TestTokenMatch(tok + 1, true))
      return true;
    else if (c == '\\' && TestTokenMatch("\\\\---///", false)) {
      PushBack(c, false);
      SyncBuffers();
      return false;
    } else if (c == '/')
      if (TestTokenMatch("//---\\\\", false)) {
        PushBack(c, false);
        SyncBuffers();
        return false;
      }
  }
  return false;
}

bool VensimLex::BufferReadLine(char *buf, size_t buflen) {
  const char *tv = sBuffer.c_str();
  while (buflen > 0 && *tv) {
    *buf = *tv;
    buf++;
    buflen--;
    tv++;
  }
  return ReadLine(buf, buflen);
}
bool VensimLex::ReadLine(char *buf, size_t buflen) {
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

void VensimLex::PushBack(char c, bool store) {
  assert(c);
  sBuffer.push_back(c);
  if (store)
    sToken.pop_back();
}
void VensimLex::SyncBuffers(void) {
  int i = sBuffer.length();
  iCurPos -= i;
  sBuffer.clear();
}
