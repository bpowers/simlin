// XMUtil.cpp : Defines the entry point for the console application.
//

#include <algorithm>
#include <boost/filesystem.hpp>
#include <cstdio>

#include "Model.h"
#include "Vensim/VensimParse.h"
#include "XMUtil.h"

#ifdef WITH_UI
#include <QApplication>

#include "UI/Main_Window.h"
#endif

static const char *argv0;

void cliUsage(void) {
  fprintf(stderr,
          "Usage: %s [OPTION...] PATH\n"
          "Convert Vensim MDL files to XMILE.\n\n"
          "Options:\n"
          "  --help:\tshow this message\n"
          "  --stdio:\tread from stdin, write to stdout\n",
          argv0);

  exit(EXIT_FAILURE);
}

int cliMain(int argc, char *argv[], Model *m) {
  int ret = 0;
  const char *path = nullptr;
  bool useStdio = false;
  bool wantComplete = false;

  for (argv0 = argv[0], argv++, argc--; argc > 0; argv++, argc--) {
    char const *arg = argv[0];
    if (strcmp("--help", arg) == 0) {
      cliUsage();
    } else if (strcmp("--stdio", arg) == 0) {
      useStdio = true;
    } else if (strcmp("--want-complete", arg) == 0) {
      wantComplete = true;
    } else if (arg[0] == '-') {
      fprintf(stderr, "unknown arg '%s'\n", arg);
      cliUsage();
    } else {
      if (!path) {
        path = arg;
      } else {
        fprintf(stderr, "specify a single path to a model\n");
        cliUsage();
      }
    }
  }

  if (useStdio) {
    path = "STDIN";
  } else if (!useStdio && path == nullptr) {
    fprintf(stderr, "ERROR: specify a path to a model or use --stdio\n");
    cliUsage();
  }

  {
    VensimParse vp(m);
    FILE *file = nullptr;
    if (useStdio) {
      file = stdin;
    } else {
      file = fopen(path, "r");
    }
    if (file == nullptr) {
      return false;
    }
    int err = 0;
    auto contents = ReadFile(file, err);
    if (err) {
      fprintf(stderr, "ReadFile(): %d (%s)\n", err, strerror(err));
      return false;
    }
    fclose(file);

    ret = !vp.ProcessFile(path, contents.c_str(), contents.size());
  }

  /*
    if(m->AnalyzeEquations()) {
      m->Simulate() ;
      m->OutputComputable(true);
    }
   */

  // mark variable types and potentially convert INTEG equations involving expressions
  // into flows (a single net flow on the first pass though this)
  m->MarkVariableTypes(nullptr);

  for (MacroFunction *mf : m->MacroFunctions()) {
    m->MarkVariableTypes(mf->NameSpace());
  }

  // if there is a view then try to make sure everything is defined in the views
  // put unknowns in a heap in the first view at 20,20 but for things that have
  // connections try to put them in the right place
  if (wantComplete) {
    m->AttachStragglers();
  }

  FILE *out = nullptr;
  if (useStdio) {
    out = stdout;
  } else {
    boost::filesystem::path p(path);
    p.replace_extension(".xmile");
    out = fopen(p.string().c_str(), "w");
    if (out == nullptr) {
      fprintf(stderr, "ERROR: couldn't open '%s' for writing.\n", p.string().c_str());
      exit(EXIT_FAILURE);
    }
  }

  std::vector<std::string> errs;
  auto xmile = m->PrintXMILE(false, errs);
  fprintf(out, "%s", xmile.c_str());
  fclose(out);

  for (const std::string &err : errs) {
    std::cerr << err << std::endl;
    ret++;
  }

  return ret;
}

#ifdef _DEBUG
void CheckMemoryTrack(int clear);
#endif

int main(int argc, char *argv[]) {
  int ret = 0;
  Model *m = new Model();
#ifndef WITH_UI
  ret = cliMain(argc, argv, m);
#else
  QApplication app(argc, argv);
  // QApplication::setWindowIcon(QIcon(":icons/icon.svg"));
  QApplication::setOrganizationName("XMUtil");
  QApplication::setOrganizationDomain("github.com/xmutil");
  QApplication::setApplicationName("MDL to XMILE");

  Main_Window window;
  window.show();

  ret = app.exec();
#endif
  delete m;
  // CheckMemoryTrack(1) ;

  // printf("Size of symbol is %d\n",sizeof(Symbol)) ;
  // printf("Size of variable is %d\n",sizeof(Variable)) ;
  // _CrtDumpMemoryLeaks() ;

  // if want to look at terminal

  return ret;
}

std::string ReadFile(FILE *file, int &error) {
  size_t bufLen = 0;
  size_t bufCapacity = 4096;
  char *buf = reinterpret_cast<char *>(malloc(bufCapacity));

  while (!feof(file) && !ferror(file)) {
    if (bufLen == bufCapacity) {
      bufCapacity *= 2;
      buf = reinterpret_cast<char *>(realloc(buf, bufCapacity));
    }
    auto len = fread(buf + bufLen, sizeof(char), bufCapacity - bufLen, file);
    // fprintf(stderr, "read: %zu %p %zu\n", len, buf + bufLen, bufCapacity - bufLen);
    bufLen += len;
  }

  if (!feof(file)) {
    error = ferror(file);
    assert(error != 0);
    // fprintf(stderr, "ferror :\\ %d %d %s \n", error, errno, strerror(errno));
    free(buf);
    return "";
  } else {
    error = 0;
  }

  std::string str{buf, bufLen};
  free(buf);
  return str;
}

#if defined(_DEBUG) && defined(wantownmemorytesting)
#include <assert.h>

#include <unordered_map>
#undef new     // regular new used in this section
#undef delete  // same for delete

typedef struct {
  size_t size;
  int line_no;
  char file[32];
} AllocInfo;

typedef std::unordered_map<void *, AllocInfo> MemTrackMap;

MemTrackMap *AllocList = 0;

void AddTrack(void *addr, size_t size, const char *fname, int lnum) {
  if (!AllocList)
    AllocList = new MemTrackMap();
  AllocInfo ai;
  ai.size = size;
  ai.line_no = lnum;
  if (strlen(fname) > 31)
    strcpy(ai.file, fname + strlen(fname) - 31);
  else
    strcpy(ai.file, fname);
  (*AllocList)[addr] = ai;
};

static int Uk = 0;
void RemoveTrack(void *addr) {
  if (AllocList) {
    MemTrackMap::iterator node = AllocList->find(addr);
    if (node != AllocList->end()) {
      AllocList->erase(node);
      return;
    }
  }
  // printf("%x %d\n",addr,++Uk) ;
  // ignore things that may have been allocated elsewhere - boost is not controllable
}

void CheckMemoryTrack(int clear) {
  if (!AllocList)
    return;
  MemTrackMap::iterator node = AllocList->begin();
  for (; node != AllocList->end(); node++) {
    fprintf(stderr, "Uncleared Memory at %u size %d from %s(%d)\n", node->first, node->second.size, node->second.file,
            node->second.line_no);
  }
  if (clear) {
    MemTrackMap *a = AllocList;
    AllocList = NULL;
    delete a;
  }
}

#endif
