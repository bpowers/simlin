#ifndef _XMUTIL_UNICODE_H
#define _XMUTIL_UNICODE_H

#include <cstdlib>

// OpenUnicode returns true if we successfully initialized global unicode state.
bool OpenUnicode();
// CloseUnicode cleans up/frees global unicode state, and should only be called on program close.
void CloseUnicode();

char *utf8ToLower(const char *src, size_t srcLen);

#endif