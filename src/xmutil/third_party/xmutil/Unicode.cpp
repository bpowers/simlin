// Unicode.cpp : Wraps unicode-specific functionality

#include "Unicode.h"

#include <cstring>

#include "libutf/utf.h"

bool OpenUnicode() {
  return true;
}

void CloseUnicode() {
}

char *utf8ToLower(const char *src, size_t srcLen) {
  int n;
  Rune u;

  size_t dstLen = 0;
  for (size_t srcOff = 0; srcOff<srcLen && * src> 0 && (n = chartorune(&u, &src[srcOff])); srcOff += n) {
    const Rune l = tolowerrune(u);
    dstLen += runelen(l);
  }

  char *dst = new char[dstLen + 1];
  memset(dst, 0, dstLen + 1);

  size_t dstOff = 0;
  for (size_t srcOff = 0; srcOff<srcLen && * src> 0 && (n = chartorune(&u, &src[srcOff])); srcOff += n) {
    Rune l = tolowerrune(u);
    const int size = runetochar(&dst[dstOff], &l);
    dstOff += size;
  }

  return dst;
}
