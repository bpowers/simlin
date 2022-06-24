#include "Log.h"

#include <cstdarg>
#include <cstdio>
#include <cstdlib>
#include <string>

#ifdef WIN32
#define ATTRIBUTE_PRINTF
#else
// this gives us better compiler error messages for callers
#define ATTRIBUTE_PRINTF __attribute__((format(printf, 1, 2)))
#endif

static std::string global_log;

void ATTRIBUTE_PRINTF log(const char *msg_fmt, ...) {
  char *msg;
  va_list args;
  int ret;

  va_start(args, msg_fmt);
  ret = vasprintf(&msg, msg_fmt, args);
  va_end(args);

  if (ret != -1) {
    global_log += msg;
  }

  free(msg);
}

extern "C" {
const char *xmutil_get_log(void) {
  return global_log.c_str();
}
void xmutil_clear_log(void) {
  global_log = "";
}
}
