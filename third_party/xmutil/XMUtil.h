#ifndef _XMUTIL_XMUTIL_H
#define _XMUTIL_XMUTIL_H

#include <string>

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

extern "C" {
// returns NULL on error or a string containing XMILE that the caller now owns
char *_convert_mdl_to_xmile(const char *mdlSource, uint32_t mdlSourceLen, bool isCompact);
}

char *utf8ToLower(const char *src, size_t srcLen);

// utility functions
std::string SpaceToUnderBar(const std::string &s);
// ascii only
bool StringMatch(const std::string &f, const std::string &s);
double AngleFromPoints(double startx, double starty, double pointx, double pointy, double endx, double endy);
std::string ReadFile(FILE *file, int &error);
#endif
