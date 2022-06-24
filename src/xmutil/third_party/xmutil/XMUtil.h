#ifndef _XMUTIL_XMUTIL_H
#define _XMUTIL_XMUTIL_H

#include <string>

#include "Log.h"

#ifdef WIN32
// XMUtil.h - globally included - generally for help with
// memory leak detection
//
#if defined(rollyourown) && defined(_DEBUG) && defined(__cplusplus)
#include <new>
extern void AddTrack(void *p, size_t size, const char *file, int line);
extern void RemoveTrack(void *p);
/* specialized placement new to track allocations */
inline void *__cdecl operator new(size_t size, const char *file, int line) {
  void *ptr = (void *)malloc(size);
  if (!ptr)
    throw std::bad_alloc();
  AddTrack(ptr, size, file, line);
  return (ptr);
};
inline void *__cdecl operator new[](size_t size, const char *file, int line) {
  void *ptr = (void *)malloc(size);
  if (!ptr)
    throw std::bad_alloc();
  AddTrack(ptr, size, file, line);
  return (ptr);
};
/* matching placement delete for exception handling */
inline void __cdecl operator delete(void *p, const char *file, int line) {
  RemoveTrack(p);
  free(p);
}
inline void __cdecl operator delete[](void *p, const char *file, int line) {
  RemoveTrack(p);
  free(p);
}
inline void __cdecl operator delete(void *p) {
  RemoveTrack(p);
  free(p);
};
inline void __cdecl operator delete[](void *p) {
  RemoveTrack(p);
  free(p);
};

#define XDEBUG_NEW new (__FILE__, __LINE__)
#define new XDEBUG_NEW
#define XDEBUG_DELETE delete
#define delete XDEBUG_DELETE
#elif defined(_DEBUG)
#define _CRTDBG_MAP_ALLOC
#include <crtdbg.h>
#include <stdlib.h>
#define DBG_NEW new (_CLIENT_BLOCK, __FILE__, __LINE__)
#define new DBG_NEW
#endif
#endif

#ifdef WIN32
#define XMUTIL_EXPORT
#else
#define XMUTIL_EXPORT __attribute__((visibility("default")))
#endif

extern "C" {
// returns NULL on error or a string containing XMILE that the caller now owns
XMUTIL_EXPORT char *xmutil_convert_mdl_to_xmile(const char *mdlSource, uint32_t mdlSourceLen, const char *fileName,
                                                bool isCompact, bool isLongName, bool isAsSectors);
// returns a non-owned, null-terminated C-string with any log output from
// previous xmutil_convert_mdl_to_xmile invocations
XMUTIL_EXPORT const char *xmutil_get_log(void);
XMUTIL_EXPORT void xmutil_clear_log(void);
}

// utility functions
std::string StringFromDouble(double val);
std::string SpaceToUnderBar(const std::string &s);
std::string QuotedSpaceToUnderBar(const std::string &s);
bool StringMatch(const std::string &f, const std::string &s);  // asciii only;
double AngleFromPoints(double startx, double starty, double pointx, double pointy, double endx, double endy);
#endif
