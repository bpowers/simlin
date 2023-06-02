// Log.h - functions for emitting logging information - wraps writing to stderr.
#ifndef _XMUTIL_LOG_H
#define _XMUTIL_LOG_H
#include <stdio.h>

// for use by bison-generated parser
#define XmutilLogf(file, msgFmt, args...) log(msgFmt, ##args)

void log(const char *msgFmt, ...);

#endif
