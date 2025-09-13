/* A Bison parser, made by GNU Bison 3.8.2.  */

/* Bison implementation for Yacc-like parsers in C

   Copyright (C) 1984, 1989-1990, 2000-2015, 2018-2021 Free Software Foundation,
   Inc.

   This program is free software: you can redistribute it and/or modify
   it under the terms of the GNU General Public License as published by
   the Free Software Foundation, either version 3 of the License, or
   (at your option) any later version.

   This program is distributed in the hope that it will be useful,
   but WITHOUT ANY WARRANTY; without even the implied warranty of
   MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
   GNU General Public License for more details.

   You should have received a copy of the GNU General Public License
   along with this program.  If not, see <https://www.gnu.org/licenses/>.  */

/* As a special exception, you may create a larger work that contains
   part or all of the Bison parser skeleton and distribute that work
   under terms of your choice, so long as that work isn't itself a
   parser generator using the skeleton or a modified version thereof
   as a parser skeleton.  Alternatively, if you modify or redistribute
   the parser skeleton itself, you may (at your option) remove this
   special exception, which will cause the skeleton and the resulting
   Bison output files to be licensed under the GNU General Public
   License without this special exception.

   This special exception was added by the Free Software Foundation in
   version 2.2 of Bison.  */

/* C LALR(1) parser skeleton written by Richard Stallman, by
   simplifying the original so-called "semantic" parser.  */

/* DO NOT RELY ON FEATURES THAT ARE NOT DOCUMENTED in the manual,
   especially those whose name start with YY_ or yy_.  They are
   private implementation details that can be changed or removed.  */

/* All symbols defined below should begin with yy or YY, to avoid
   infringing on user name space.  This should be done even for local
   variables, as they might otherwise be expanded by user macros.
   There are some unavoidable exceptions within include files to
   define necessary library symbols; they are noted "INFRINGES ON
   USER NAME SPACE" below.  */

/* Identify Bison output, and Bison version.  */
#define YYBISON 30802

/* Bison version string.  */
#define YYBISON_VERSION "3.8.2"

/* Skeleton name.  */
#define YYSKELETON_NAME "yacc.c"

/* Pure parsers.  */
#define YYPURE 0

/* Push parsers.  */
#define YYPUSH 0

/* Pull parsers.  */
#define YYPULL 1

/* Substitute the variable and function names.  */
#define yyparse dpyyparse
#define yylex dpyylex
#define yyerror dpyyerror
#define yydebug dpyydebug
#define yynerrs dpyynerrs
#define yylval dpyylval
#define yychar dpyychar

/* First part of user prologue.  */
#line 10 "DYacc.y"

#include "../Log.h"
#include "../Symbol/Parse.h"
#include "DynamoParseFunctions.h"
extern int dpyylex(void);
extern void dpyyerror(char const *);
#define YYSTYPE ParseUnion
#define YYFPRINTF fprintf

#line 88 "DYacc.tab.cpp"

#ifndef YY_CAST
#ifdef __cplusplus
#define YY_CAST(Type, Val) static_cast<Type>(Val)
#define YY_REINTERPRET_CAST(Type, Val) reinterpret_cast<Type>(Val)
#else
#define YY_CAST(Type, Val) ((Type)(Val))
#define YY_REINTERPRET_CAST(Type, Val) ((Type)(Val))
#endif
#endif
#ifndef YY_NULLPTR
#if defined __cplusplus
#if 201103L <= __cplusplus
#define YY_NULLPTR nullptr
#else
#define YY_NULLPTR 0
#endif
#else
#define YY_NULLPTR ((void *)0)
#endif
#endif

#include "DYacc.tab.hpp"
/* Symbol kind.  */
enum yysymbol_kind_t {
  YYSYMBOL_YYEMPTY = -2,
  YYSYMBOL_YYEOF = 0,              /* "end of file"  */
  YYSYMBOL_YYerror = 1,            /* error  */
  YYSYMBOL_YYUNDEF = 2,            /* "invalid token"  */
  YYSYMBOL_DPTT_aux = 3,           /* DPTT_aux  */
  YYSYMBOL_DPTT_table = 4,         /* DPTT_table  */
  YYSYMBOL_DPTT_level = 5,         /* DPTT_level  */
  YYSYMBOL_DPTT_init = 6,          /* DPTT_init  */
  YYSYMBOL_DPTT_constant = 7,      /* DPTT_constant  */
  YYSYMBOL_DPTT_eoq = 8,           /* DPTT_eoq  */
  YYSYMBOL_DPTT_groupstar = 9,     /* DPTT_groupstar  */
  YYSYMBOL_DPTT_specs = 10,        /* DPTT_specs  */
  YYSYMBOL_DPTT_save = 11,         /* DPTT_save  */
  YYSYMBOL_DPTT_and = 12,          /* DPTT_and  */
  YYSYMBOL_DPTT_macro = 13,        /* DPTT_macro  */
  YYSYMBOL_DPTT_end_of_macro = 14, /* DPTT_end_of_macro  */
  YYSYMBOL_DPTT_or = 15,           /* DPTT_or  */
  YYSYMBOL_DPTT_not = 16,          /* DPTT_not  */
  YYSYMBOL_DPTT_ge = 17,           /* DPTT_ge  */
  YYSYMBOL_DPTT_le = 18,           /* DPTT_le  */
  YYSYMBOL_DPTT_ne = 19,           /* DPTT_ne  */
  YYSYMBOL_DPTT_number = 20,       /* DPTT_number  */
  YYSYMBOL_DPTT_symbol = 21,       /* DPTT_symbol  */
  YYSYMBOL_DPTT_units_symbol = 22, /* DPTT_units_symbol  */
  YYSYMBOL_DPTT_function = 23,     /* DPTT_function  */
  YYSYMBOL_DPTT_dt_to_one = 24,    /* DPTT_dt_to_one  */
  YYSYMBOL_25_ = 25,               /* '-'  */
  YYSYMBOL_26_ = 26,               /* '+'  */
  YYSYMBOL_27_ = 27,               /* '='  */
  YYSYMBOL_28_ = 28,               /* '<'  */
  YYSYMBOL_29_ = 29,               /* '>'  */
  YYSYMBOL_30_ = 30,               /* '*'  */
  YYSYMBOL_31_ = 31,               /* '/'  */
  YYSYMBOL_32_ = 32,               /* '^'  */
  YYSYMBOL_33_ = 33,               /* ','  */
  YYSYMBOL_34_ = 34,               /* '('  */
  YYSYMBOL_35_ = 35,               /* ')'  */
  YYSYMBOL_36_ = 36,               /* ':'  */
  YYSYMBOL_37_ = 37,               /* '['  */
  YYSYMBOL_38_ = 38,               /* ']'  */
  YYSYMBOL_39_ = 39,               /* '!'  */
  YYSYMBOL_40_ = 40,               /* ';'  */
  YYSYMBOL_YYACCEPT = 41,          /* $accept  */
  YYSYMBOL_fulleq = 42,            /* fulleq  */
  YYSYMBOL_teqn = 43,              /* teqn  */
  YYSYMBOL_tabledef = 44,          /* tabledef  */
  YYSYMBOL_eqn = 45,               /* eqn  */
  YYSYMBOL_stock_eqn = 46,         /* stock_eqn  */
  YYSYMBOL_lhs = 47,               /* lhs  */
  YYSYMBOL_var = 48,               /* var  */
  YYSYMBOL_sublist = 49,           /* sublist  */
  YYSYMBOL_symlist = 50,           /* symlist  */
  YYSYMBOL_subdef = 51,            /* subdef  */
  YYSYMBOL_number = 52,            /* number  */
  YYSYMBOL_maplist = 53,           /* maplist  */
  YYSYMBOL_exprlist = 54,          /* exprlist  */
  YYSYMBOL_exp = 55,               /* exp  */
  YYSYMBOL_tablevals = 56,         /* tablevals  */
  YYSYMBOL_xytablevals = 57,       /* xytablevals  */
  YYSYMBOL_xytablevec = 58,        /* xytablevec  */
  YYSYMBOL_tablepairs = 59         /* tablepairs  */
};
typedef enum yysymbol_kind_t yysymbol_kind_t;

#ifdef short
#undef short
#endif

/* On compilers that do not define __PTRDIFF_MAX__ etc., make sure
   <limits.h> and (if available) <stdint.h> are included
   so that the code can choose integer types of a good width.  */

#ifndef __PTRDIFF_MAX__
#include <limits.h> /* INFRINGES ON USER NAME SPACE */
#if defined __STDC_VERSION__ && 199901 <= __STDC_VERSION__
#include <stdint.h> /* INFRINGES ON USER NAME SPACE */
#define YY_STDINT_H
#endif
#endif

/* Narrow types that promote to a signed type and that can represent a
   signed or unsigned integer of at least N bits.  In tables they can
   save space and decrease cache pressure.  Promoting to a signed type
   helps avoid bugs in integer arithmetic.  */

#ifdef __INT_LEAST8_MAX__
typedef __INT_LEAST8_TYPE__ yytype_int8;
#elif defined YY_STDINT_H
typedef int_least8_t yytype_int8;
#else
typedef signed char yytype_int8;
#endif

#ifdef __INT_LEAST16_MAX__
typedef __INT_LEAST16_TYPE__ yytype_int16;
#elif defined YY_STDINT_H
typedef int_least16_t yytype_int16;
#else
typedef short yytype_int16;
#endif

/* Work around bug in HP-UX 11.23, which defines these macros
   incorrectly for preprocessor constants.  This workaround can likely
   be removed in 2023, as HPE has promised support for HP-UX 11.23
   (aka HP-UX 11i v2) only through the end of 2022; see Table 2 of
   <https://h20195.www2.hpe.com/V2/getpdf.aspx/4AA4-7673ENW.pdf>.  */
#ifdef __hpux
#undef UINT_LEAST8_MAX
#undef UINT_LEAST16_MAX
#define UINT_LEAST8_MAX 255
#define UINT_LEAST16_MAX 65535
#endif

#if defined __UINT_LEAST8_MAX__ && __UINT_LEAST8_MAX__ <= __INT_MAX__
typedef __UINT_LEAST8_TYPE__ yytype_uint8;
#elif (!defined __UINT_LEAST8_MAX__ && defined YY_STDINT_H && UINT_LEAST8_MAX <= INT_MAX)
typedef uint_least8_t yytype_uint8;
#elif !defined __UINT_LEAST8_MAX__ && UCHAR_MAX <= INT_MAX
typedef unsigned char yytype_uint8;
#else
typedef short yytype_uint8;
#endif

#if defined __UINT_LEAST16_MAX__ && __UINT_LEAST16_MAX__ <= __INT_MAX__
typedef __UINT_LEAST16_TYPE__ yytype_uint16;
#elif (!defined __UINT_LEAST16_MAX__ && defined YY_STDINT_H && UINT_LEAST16_MAX <= INT_MAX)
typedef uint_least16_t yytype_uint16;
#elif !defined __UINT_LEAST16_MAX__ && USHRT_MAX <= INT_MAX
typedef unsigned short yytype_uint16;
#else
typedef int yytype_uint16;
#endif

#ifndef YYPTRDIFF_T
#if defined __PTRDIFF_TYPE__ && defined __PTRDIFF_MAX__
#define YYPTRDIFF_T __PTRDIFF_TYPE__
#define YYPTRDIFF_MAXIMUM __PTRDIFF_MAX__
#elif defined PTRDIFF_MAX
#ifndef ptrdiff_t
#include <stddef.h> /* INFRINGES ON USER NAME SPACE */
#endif
#define YYPTRDIFF_T ptrdiff_t
#define YYPTRDIFF_MAXIMUM PTRDIFF_MAX
#else
#define YYPTRDIFF_T long
#define YYPTRDIFF_MAXIMUM LONG_MAX
#endif
#endif

#ifndef YYSIZE_T
#ifdef __SIZE_TYPE__
#define YYSIZE_T __SIZE_TYPE__
#elif defined size_t
#define YYSIZE_T size_t
#elif defined __STDC_VERSION__ && 199901 <= __STDC_VERSION__
#include <stddef.h> /* INFRINGES ON USER NAME SPACE */
#define YYSIZE_T size_t
#else
#define YYSIZE_T unsigned
#endif
#endif

#define YYSIZE_MAXIMUM \
  YY_CAST(YYPTRDIFF_T, (YYPTRDIFF_MAXIMUM < YY_CAST(YYSIZE_T, -1) ? YYPTRDIFF_MAXIMUM : YY_CAST(YYSIZE_T, -1)))

#define YYSIZEOF(X) YY_CAST(YYPTRDIFF_T, sizeof(X))

/* Stored state numbers (used for stacks). */
typedef yytype_uint8 yy_state_t;

/* State numbers in computations.  */
typedef int yy_state_fast_t;

#ifndef YY_
#if defined YYENABLE_NLS && YYENABLE_NLS
#if ENABLE_NLS
#include <libintl.h> /* INFRINGES ON USER NAME SPACE */
#define YY_(Msgid) dgettext("bison-runtime", Msgid)
#endif
#endif
#ifndef YY_
#define YY_(Msgid) Msgid
#endif
#endif

#ifndef YY_ATTRIBUTE_PURE
#if defined __GNUC__ && 2 < __GNUC__ + (96 <= __GNUC_MINOR__)
#define YY_ATTRIBUTE_PURE __attribute__((__pure__))
#else
#define YY_ATTRIBUTE_PURE
#endif
#endif

#ifndef YY_ATTRIBUTE_UNUSED
#if defined __GNUC__ && 2 < __GNUC__ + (7 <= __GNUC_MINOR__)
#define YY_ATTRIBUTE_UNUSED __attribute__((__unused__))
#else
#define YY_ATTRIBUTE_UNUSED
#endif
#endif

/* Suppress unused-variable warnings by "using" E.  */
#if !defined lint || defined __GNUC__
#define YY_USE(E) ((void)(E))
#else
#define YY_USE(E) /* empty */
#endif

/* Suppress an incorrect diagnostic about yylval being uninitialized.  */
#if defined __GNUC__ && !defined __ICC && 406 <= __GNUC__ * 100 + __GNUC_MINOR__
#if __GNUC__ * 100 + __GNUC_MINOR__ < 407
#define YY_IGNORE_MAYBE_UNINITIALIZED_BEGIN \
  _Pragma("GCC diagnostic push") _Pragma("GCC diagnostic ignored \"-Wuninitialized\"")
#else
#define YY_IGNORE_MAYBE_UNINITIALIZED_BEGIN                                            \
  _Pragma("GCC diagnostic push") _Pragma("GCC diagnostic ignored \"-Wuninitialized\"") \
      _Pragma("GCC diagnostic ignored \"-Wmaybe-uninitialized\"")
#endif
#define YY_IGNORE_MAYBE_UNINITIALIZED_END _Pragma("GCC diagnostic pop")
#else
#define YY_INITIAL_VALUE(Value) Value
#endif
#ifndef YY_IGNORE_MAYBE_UNINITIALIZED_BEGIN
#define YY_IGNORE_MAYBE_UNINITIALIZED_BEGIN
#define YY_IGNORE_MAYBE_UNINITIALIZED_END
#endif
#ifndef YY_INITIAL_VALUE
#define YY_INITIAL_VALUE(Value) /* Nothing. */
#endif

#if defined __cplusplus && defined __GNUC__ && !defined __ICC && 6 <= __GNUC__
#define YY_IGNORE_USELESS_CAST_BEGIN _Pragma("GCC diagnostic push") _Pragma("GCC diagnostic ignored \"-Wuseless-cast\"")
#define YY_IGNORE_USELESS_CAST_END _Pragma("GCC diagnostic pop")
#endif
#ifndef YY_IGNORE_USELESS_CAST_BEGIN
#define YY_IGNORE_USELESS_CAST_BEGIN
#define YY_IGNORE_USELESS_CAST_END
#endif

#define YY_ASSERT(E) ((void)(0 && (E)))

#if !defined yyoverflow

/* The parser invokes alloca or malloc; define the necessary symbols.  */

#ifdef YYSTACK_USE_ALLOCA
#if YYSTACK_USE_ALLOCA
#ifdef __GNUC__
#define YYSTACK_ALLOC __builtin_alloca
#elif defined __BUILTIN_VA_ARG_INCR
#include <alloca.h> /* INFRINGES ON USER NAME SPACE */
#elif defined _AIX
#define YYSTACK_ALLOC __alloca
#elif defined _MSC_VER
#include <malloc.h> /* INFRINGES ON USER NAME SPACE */
#define alloca _alloca
#else
#define YYSTACK_ALLOC alloca
#if !defined _ALLOCA_H && !defined EXIT_SUCCESS
#include <stdlib.h> /* INFRINGES ON USER NAME SPACE */
/* Use EXIT_SUCCESS as a witness for stdlib.h.  */
#ifndef EXIT_SUCCESS
#define EXIT_SUCCESS 0
#endif
#endif
#endif
#endif
#endif

#ifdef YYSTACK_ALLOC
/* Pacify GCC's 'empty if-body' warning.  */
#define YYSTACK_FREE(Ptr) \
  do { /* empty */        \
    ;                     \
  } while (0)
#ifndef YYSTACK_ALLOC_MAXIMUM
/* The OS might guarantee only one guard page at the bottom of the stack,
   and a page size can be as small as 4096 bytes.  So we cannot safely
   invoke alloca (N) if N exceeds 4096.  Use a slightly smaller number
   to allow for a few compiler-allocated temporary stack slots.  */
#define YYSTACK_ALLOC_MAXIMUM 4032 /* reasonable circa 2006 */
#endif
#else
#define YYSTACK_ALLOC YYMALLOC
#define YYSTACK_FREE YYFREE
#ifndef YYSTACK_ALLOC_MAXIMUM
#define YYSTACK_ALLOC_MAXIMUM YYSIZE_MAXIMUM
#endif
#if (defined __cplusplus && !defined EXIT_SUCCESS && \
     !((defined YYMALLOC || defined malloc) && (defined YYFREE || defined free)))
#include <stdlib.h> /* INFRINGES ON USER NAME SPACE */
#ifndef EXIT_SUCCESS
#define EXIT_SUCCESS 0
#endif
#endif
#ifndef YYMALLOC
#define YYMALLOC malloc
#if !defined malloc && !defined EXIT_SUCCESS
void *malloc(YYSIZE_T); /* INFRINGES ON USER NAME SPACE */
#endif
#endif
#ifndef YYFREE
#define YYFREE free
#if !defined free && !defined EXIT_SUCCESS
void free(void *); /* INFRINGES ON USER NAME SPACE */
#endif
#endif
#endif
#endif /* !defined yyoverflow */

#if (!defined yyoverflow && (!defined __cplusplus || (defined YYSTYPE_IS_TRIVIAL && YYSTYPE_IS_TRIVIAL)))

/* A type that is properly aligned for any stack member.  */
union yyalloc {
  yy_state_t yyss_alloc;
  YYSTYPE yyvs_alloc;
};

/* The size of the maximum gap between one aligned stack and the next.  */
#define YYSTACK_GAP_MAXIMUM (YYSIZEOF(union yyalloc) - 1)

/* The size of an array large to enough to hold all stacks, each with
   N elements.  */
#define YYSTACK_BYTES(N) ((N) * (YYSIZEOF(yy_state_t) + YYSIZEOF(YYSTYPE)) + YYSTACK_GAP_MAXIMUM)

#define YYCOPY_NEEDED 1

/* Relocate STACK from its old location to the new one.  The
   local variables YYSIZE and YYSTACKSIZE give the old and new number of
   elements in the stack, and YYPTR gives the new location of the
   stack.  Advance YYPTR to a properly aligned location for the next
   stack.  */
#define YYSTACK_RELOCATE(Stack_alloc, Stack)                           \
  do {                                                                 \
    YYPTRDIFF_T yynewbytes;                                            \
    YYCOPY(&yyptr->Stack_alloc, Stack, yysize);                        \
    Stack = &yyptr->Stack_alloc;                                       \
    yynewbytes = yystacksize * YYSIZEOF(*Stack) + YYSTACK_GAP_MAXIMUM; \
    yyptr += yynewbytes / YYSIZEOF(*yyptr);                            \
  } while (0)

#endif

#if defined YYCOPY_NEEDED && YYCOPY_NEEDED
/* Copy COUNT objects from SRC to DST.  The source and destination do
   not overlap.  */
#ifndef YYCOPY
#if defined __GNUC__ && 1 < __GNUC__
#define YYCOPY(Dst, Src, Count) __builtin_memcpy(Dst, Src, YY_CAST(YYSIZE_T, (Count)) * sizeof(*(Src)))
#else
#define YYCOPY(Dst, Src, Count)         \
  do {                                  \
    YYPTRDIFF_T yyi;                    \
    for (yyi = 0; yyi < (Count); yyi++) \
      (Dst)[yyi] = (Src)[yyi];          \
  } while (0)
#endif
#endif
#endif /* !YYCOPY_NEEDED */

/* YYFINAL -- State number of the termination state.  */
#define YYFINAL 22
/* YYLAST -- Last index in YYTABLE.  */
#define YYLAST 221

/* YYNTOKENS -- Number of terminals.  */
#define YYNTOKENS 41
/* YYNNTS -- Number of nonterminals.  */
#define YYNNTS 19
/* YYNRULES -- Number of rules.  */
#define YYNRULES 72
/* YYNSTATES -- Number of states.  */
#define YYNSTATES 165

/* YYMAXUTOK -- Last valid token kind.  */
#define YYMAXUTOK 279

/* YYTRANSLATE(TOKEN-NUM) -- Symbol number corresponding to TOKEN-NUM
   as returned by yylex, with out-of-bounds checking.  */
#define YYTRANSLATE(YYX) \
  (0 <= (YYX) && (YYX) <= YYMAXUTOK ? YY_CAST(yysymbol_kind_t, yytranslate[YYX]) : YYSYMBOL_YYUNDEF)

/* YYTRANSLATE[TOKEN-NUM] -- Symbol number corresponding to TOKEN-NUM
   as returned by yylex.  */
static const yytype_int8 yytranslate[] = {
    0, 2,  2, 2, 2, 2, 2, 2, 2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2, 2, 2, 2,  2,  2,  2,  2,  2,
    2, 39, 2, 2, 2, 2, 2, 2, 34, 35, 30, 26, 33, 25, 2,  31, 2,  2,  2,  2,  2,  2,  2,  2, 2, 2, 36, 40, 28, 27, 29, 2,
    2, 2,  2, 2, 2, 2, 2, 2, 2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2, 2, 2, 2,  37, 2,  38, 32, 2,
    2, 2,  2, 2, 2, 2, 2, 2, 2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2, 2, 2, 2,  2,  2,  2,  2,  2,
    2, 2,  2, 2, 2, 2, 2, 2, 2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2, 2, 2, 2,  2,  2,  2,  2,  2,
    2, 2,  2, 2, 2, 2, 2, 2, 2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2, 2, 2, 2,  2,  2,  2,  2,  2,
    2, 2,  2, 2, 2, 2, 2, 2, 2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2, 2, 2, 2,  2,  2,  2,  2,  2,
    2, 2,  2, 2, 2, 2, 2, 2, 2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2, 2, 2, 2,  2,  2,  2,  2,  2,
    1, 2,  3, 4, 5, 6, 7, 8, 9,  10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24};

#if YYDEBUG
/* YYRLINE[YYN] -- Source line where rule number YYN was defined.  */
static const yytype_uint8 yyrline[] = {0,   86,  86,  87,  88,  89,  90,  91,  92,  93,  94,  98,  103, 104, 105,
                                       112, 113, 114, 115, 116, 120, 127, 131, 132, 136, 140, 141, 142, 143, 146,
                                       147, 148, 149, 153, 154, 155, 160, 165, 166, 167, 168, 172, 173, 174, 175,
                                       176, 177, 178, 179, 180, 181, 182, 183, 184, 185, 186, 187, 188, 189, 190,
                                       191, 192, 193, 194, 198, 199, 201, 206, 207, 212, 213, 218, 219};
#endif

/** Accessing symbol of state STATE.  */
#define YY_ACCESSING_SYMBOL(State) YY_CAST(yysymbol_kind_t, yystos[State])

#if YYDEBUG || 0
/* The user-facing name of the symbol whose (internal) number is
   YYSYMBOL.  No bounds checking.  */
static const char *yysymbol_name(yysymbol_kind_t yysymbol) YY_ATTRIBUTE_UNUSED;

/* YYTNAME[SYMBOL-NUM] -- String name of the symbol SYMBOL-NUM.
   First, the terminals, then, starting at YYNTOKENS, nonterminals.  */
static const char *const yytname[] = {"\"end of file\"",
                                      "error",
                                      "\"invalid token\"",
                                      "DPTT_aux",
                                      "DPTT_table",
                                      "DPTT_level",
                                      "DPTT_init",
                                      "DPTT_constant",
                                      "DPTT_eoq",
                                      "DPTT_groupstar",
                                      "DPTT_specs",
                                      "DPTT_save",
                                      "DPTT_and",
                                      "DPTT_macro",
                                      "DPTT_end_of_macro",
                                      "DPTT_or",
                                      "DPTT_not",
                                      "DPTT_ge",
                                      "DPTT_le",
                                      "DPTT_ne",
                                      "DPTT_number",
                                      "DPTT_symbol",
                                      "DPTT_units_symbol",
                                      "DPTT_function",
                                      "DPTT_dt_to_one",
                                      "'-'",
                                      "'+'",
                                      "'='",
                                      "'<'",
                                      "'>'",
                                      "'*'",
                                      "'/'",
                                      "'^'",
                                      "','",
                                      "'('",
                                      "')'",
                                      "':'",
                                      "'['",
                                      "']'",
                                      "'!'",
                                      "';'",
                                      "$accept",
                                      "fulleq",
                                      "teqn",
                                      "tabledef",
                                      "eqn",
                                      "stock_eqn",
                                      "lhs",
                                      "var",
                                      "sublist",
                                      "symlist",
                                      "subdef",
                                      "number",
                                      "maplist",
                                      "exprlist",
                                      "exp",
                                      "tablevals",
                                      "xytablevals",
                                      "xytablevec",
                                      "tablepairs",
                                      YY_NULLPTR};

static const char *yysymbol_name(yysymbol_kind_t yysymbol) {
  return yytname[yysymbol];
}
#endif

#define YYPACT_NINF (-67)

#define yypact_value_is_default(Yyn) ((Yyn) == YYPACT_NINF)

#define YYTABLE_NINF (-1)

#define yytable_value_is_error(Yyn) 0

/* YYPACT[STATE-NUM] -- Index in YYTABLE of the portion describing
   STATE-NUM.  */
static const yytype_int16 yypact[] = {
    208, 10,  32,  32,  10,  10,  -67, -67, -67, -67, 63,  41,  66,  -14, -67, 53,  83,  74,  102, 93,  114,
    116, -67, -19, 122, -67, -67, 79,  72,  -67, 50,  -67, 32,  -67, -67, -67, 123, 109, 96,  19,  79,  -67,
    111, 79,  79,  79,  112, 18,  100, -67, 127, 135, 50,  124, -67, 128, 129, 132, 133, 1,   -67, 130, 137,
    -13, -67, -67, 136, -67, 144, 159, 121, 121, 54,  79,  79,  79,  79,  79,  79,  79,  79,  79,  79,  79,
    79,  79,  79,  79,  79,  -67, -67, 134, 50,  -67, -67, 50,  143, 50,  50,  79,  147, -67, 157, 148, -67,
    -24, -67, -21, 100, 100, 29,  142, -2,  -2,  -2,  121, 121, -2,  -2,  -2,  144, 144, 144, 50,  150, -67,
    50,  -67, -67, 18,  146, 161, -67, 175, -67, -67, 153, 50,  156, -67, 169, -67, -67, 162, 50,  164, 167,
    168, -67, 170, -67, 50,  172, 50,  171, 70,  173, 187, 78,  30,  188, 132, 133, 173, 133};

/* YYDEFACT[STATE-NUM] -- Default reduction number in state STATE-NUM.
   Performed when YYTABLE does not specify something else to do.  Zero
   means the default is an error.  */
static const yytype_int8 yydefact[] = {
    0,  0,  0,  0,  0,  0,  2,  3,  4,  5,  0,  22, 0,  18, 21, 22, 0,  0,  0,  0,  0,  0,  1,  0,  0,  23, 10, 0,
    0,  6,  0,  9,  0,  8,  7,  29, 0,  36, 25, 0,  0,  41, 0,  0,  0,  0,  42, 15, 37, 33, 0,  0,  0,  0,  69, 0,
    0,  67, 64, 11, 12, 0,  0,  0,  19, 26, 0,  24, 59, 0,  61, 62, 0,  0,  0,  40, 0,  0,  0,  0,  0,  0,  0,  0,
    0,  0,  0,  0,  0,  34, 35, 0,  0,  16, 17, 0,  0,  0,  0,  0,  0,  31, 0,  27, 47, 0,  44, 0,  38, 39, 58, 57,
    55, 53, 56, 49, 48, 60, 52, 54, 50, 51, 63, 0,  0,  70, 0,  14, 13, 20, 0,  0,  28, 0,  45, 43, 0,  0,  0,  30,
    0,  46, 71, 0,  0,  0,  0,  0,  32, 0,  72, 0,  0,  0,  0,  0,  0,  0,  0,  0,  0,  68, 65, 0,  66};

/* YYPGOTO[NTERM-NUM].  */
static const yytype_int8 yypgoto[] = {-67, -67, -67, -67, 13,  -67, 85,  22, -67, -67,
                                      -67, -30, -67, -66, -39, -67, -67, 43, -22};

/* YYDEFGOTO[NTERM-NUM].  */
static const yytype_int8 yydefgoto[] = {0, 10, 16, 59, 12, 18, 13, 46, 25, 39, 37, 54, 64, 47, 48, 55, 56, 57, 58};

/* YYTABLE[YYPACT[STATE-NUM]] -- What to do in state STATE-NUM.  If
   positive, shift that token.  If negative, reduce the rule whose
   number is the opposite.  If YYTABLE_NINF, syntax error.  */
static const yytype_uint8 yytable[] = {
    60,  68,  35,  105, 70,  71,  72,  107, 101, 133, 76,  134, 74,  27,  135, 36,  75,  20,  21,  75,  28,  102, 91,
    14,  14,  14,  14,  14,  86,  87,  88,  11,  97,  129, 98,  108, 109, 110, 111, 112, 113, 114, 115, 116, 117, 118,
    119, 120, 121, 122, 49,  74,  66,  15,  61,  50,  51,  67,  75,  86,  87,  88,  124, 22,  52,  125, 76,  127, 128,
    77,  49,  78,  79,  80,  26,  50,  51,  23,  24,  81,  82,  83,  84,  85,  86,  87,  88,  17,  19,  106, 24,  29,
    49,  136, 108, 40,  138, 50,  51,  41,  15,  30,  42,  156, 43,  44,  52,  143, 157, 53,  31,  96,  76,  45,  147,
    77,  160, 78,  79,  80,  32,  152, 33,  154, 34,  81,  82,  83,  84,  85,  86,  87,  88,  76,  158, 65,  77,  162,
    78,  79,  80,  164, 63,  38,  62,  69,  73,  89,  83,  84,  85,  86,  87,  88,  76,  90,  99,  103, 92,  78,  79,
    80,  100, 93,  94,  95,  96,  123, 130, 83,  84,  85,  86,  87,  88,  40,  88,  126, 131, 41,  15,  139, 42,  137,
    43,  44,  140, 132, 142, 144, 145, 40,  149, 45,  104, 41,  15,  146, 42,  148, 43,  44,  161, 150, 151, 153, 155,
    52,  0,   45,  141, 1,   2,   3,   4,   5,   6,   7,   8,   9,   159, 163};

static const yytype_int16 yycheck[] = {
    30, 40, 21,  69, 43, 44, 45, 73,  21, 33,  12, 35, 33,  27, 35,  34, 40,  4,  5,   40,  34, 34,  52, 1,   2,
    3,  4,  5,   30, 31, 32, 21, 31,  99, 33,  74, 75, 76,  77, 78,  79, 80,  81, 82,  83,  84, 85,  86, 87,  88,
    20, 33, 33,  21, 32, 25, 26, 38,  40, 30,  31, 32, 92,  0,  34,  95, 12,  97, 98,  15,  20, 17,  18, 19,  8,
    25, 26, 36,  37, 25, 26, 27, 28,  29, 30,  31, 32, 2,   3,  35,  37, 8,   20, 123, 133, 16, 126, 25, 26,  20,
    21, 27, 23,  33, 25, 26, 34, 137, 38, 37,  8,  33, 12,  34, 144, 15, 38,  17, 18,  19,  27, 151, 8,  153, 8,
    25, 26, 27,  28, 29, 30, 31, 32,  12, 156, 39, 15, 159, 17, 18,  19, 163, 33, 21,  21,  34, 34,  20, 27,  28,
    29, 30, 31,  32, 12, 20, 26, 21,  34, 17,  18, 19, 25,  35, 35,  33, 33,  33, 21,  27,  28, 29,  30, 31,  32,
    16, 32, 34,  21, 20, 21, 35, 23,  33, 25,  26, 25, 39,  35, 33,  21, 16,  25, 34,  35,  20, 21,  35, 23,  35,
    25, 26, 159, 35, 34, 33, 35, 34,  -1, 34,  35, 3,  4,   5,  6,   7,  8,   9,  10,  11,  33, 33};

/* YYSTOS[STATE-NUM] -- The symbol kind of the accessing symbol of
   state STATE-NUM.  */
static const yytype_int8 yystos[] = {
    0,  3,  4,  5,  6,  7,  8,  9,  10, 11, 42, 21, 45, 47, 48, 21, 43, 47, 46, 47, 45, 45, 0,  36, 37, 49, 8,  27,
    34, 8,  27, 8,  27, 8,  8,  21, 34, 51, 21, 50, 16, 20, 23, 25, 26, 34, 48, 54, 55, 20, 25, 26, 34, 37, 52, 56,
    57, 58, 59, 44, 52, 48, 21, 33, 53, 39, 33, 38, 55, 34, 55, 55, 55, 34, 33, 40, 12, 15, 17, 18, 19, 25, 26, 27,
    28, 29, 30, 31, 32, 20, 20, 52, 34, 35, 35, 33, 33, 31, 33, 26, 25, 21, 34, 21, 35, 54, 35, 54, 55, 55, 55, 55,
    55, 55, 55, 55, 55, 55, 55, 55, 55, 55, 55, 33, 52, 52, 34, 52, 52, 54, 21, 21, 39, 33, 35, 35, 52, 33, 52, 35,
    25, 35, 35, 52, 33, 21, 35, 52, 35, 25, 35, 34, 52, 33, 52, 35, 33, 38, 59, 33, 38, 58, 59, 33, 59};

/* YYR1[RULE-NUM] -- Symbol kind of the left-hand side of rule RULE-NUM.  */
static const yytype_int8 yyr1[] = {0,  41, 42, 42, 42, 42, 42, 42, 42, 42, 42, 43, 44, 44, 44, 45, 45, 45, 45,
                                   45, 46, 47, 48, 48, 49, 50, 50, 50, 50, 51, 51, 51, 51, 52, 52, 52, 53, 54,
                                   54, 54, 54, 55, 55, 55, 55, 55, 55, 55, 55, 55, 55, 55, 55, 55, 55, 55, 55,
                                   55, 55, 55, 55, 55, 55, 55, 56, 56, 56, 57, 57, 58, 58, 59, 59};

/* YYR2[RULE-NUM] -- Number of symbols on the right-hand side of rule RULE-NUM.  */
static const yytype_int8 yyr2[] = {0, 2, 1, 1, 1, 1, 3, 3, 3, 3, 3, 3, 1, 3, 3, 3,  4,  4, 1,  4, 5, 1, 1, 2, 3,
                                   1, 2, 3, 4, 1, 5, 3, 7, 1, 2, 2, 0, 1, 3, 3, 2,  1,  1, 4,  3, 4, 5, 3, 3, 3,
                                   3, 3, 3, 3, 3, 3, 3, 3, 3, 2, 3, 2, 2, 3, 1, 15, 17, 1, 15, 1, 3, 5, 7};

enum { YYENOMEM = -2 };

#define yyerrok (yyerrstatus = 0)
#define yyclearin (yychar = YYEMPTY)

#define YYACCEPT goto yyacceptlab
#define YYABORT goto yyabortlab
#define YYERROR goto yyerrorlab
#define YYNOMEM goto yyexhaustedlab

#define YYRECOVERING() (!!yyerrstatus)

#define YYBACKUP(Token, Value)                      \
  do                                                \
    if (yychar == YYEMPTY) {                        \
      yychar = (Token);                             \
      yylval = (Value);                             \
      YYPOPSTACK(yylen);                            \
      yystate = *yyssp;                             \
      goto yybackup;                                \
    } else {                                        \
      yyerror(YY_("syntax error: cannot back up")); \
      YYERROR;                                      \
    }                                               \
  while (0)

/* Backward compatibility with an undocumented macro.
   Use YYerror or YYUNDEF. */
#define YYERRCODE YYUNDEF

/* Enable debugging if requested.  */
#if YYDEBUG

#ifndef YYFPRINTF
#include <stdio.h> /* INFRINGES ON USER NAME SPACE */
#define YYFPRINTF fprintf
#endif

#define YYDPRINTF(Args) \
  do {                  \
    if (yydebug)        \
      YYFPRINTF Args;   \
  } while (0)

#define YY_SYMBOL_PRINT(Title, Kind, Value, Location) \
  do {                                                \
    if (yydebug) {                                    \
      YYFPRINTF(stderr, "%s ", Title);                \
      yy_symbol_print(stderr, Kind, Value);           \
      YYFPRINTF(stderr, "\n");                        \
    }                                                 \
  } while (0)

/*-----------------------------------.
| Print this symbol's value on YYO.  |
`-----------------------------------*/

static void yy_symbol_value_print(FILE *yyo, yysymbol_kind_t yykind, YYSTYPE const *const yyvaluep) {
  FILE *yyoutput = yyo;
  YY_USE(yyoutput);
  if (!yyvaluep)
    return;
  YY_IGNORE_MAYBE_UNINITIALIZED_BEGIN
  YY_USE(yykind);
  YY_IGNORE_MAYBE_UNINITIALIZED_END
}

/*---------------------------.
| Print this symbol on YYO.  |
`---------------------------*/

static void yy_symbol_print(FILE *yyo, yysymbol_kind_t yykind, YYSTYPE const *const yyvaluep) {
  YYFPRINTF(yyo, "%s %s (", yykind < YYNTOKENS ? "token" : "nterm", yysymbol_name(yykind));

  yy_symbol_value_print(yyo, yykind, yyvaluep);
  YYFPRINTF(yyo, ")");
}

/*------------------------------------------------------------------.
| yy_stack_print -- Print the state stack from its BOTTOM up to its |
| TOP (included).                                                   |
`------------------------------------------------------------------*/

static void yy_stack_print(yy_state_t *yybottom, yy_state_t *yytop) {
  YYFPRINTF(stderr, "Stack now");
  for (; yybottom <= yytop; yybottom++) {
    int yybot = *yybottom;
    YYFPRINTF(stderr, " %d", yybot);
  }
  YYFPRINTF(stderr, "\n");
}

#define YY_STACK_PRINT(Bottom, Top)    \
  do {                                 \
    if (yydebug)                       \
      yy_stack_print((Bottom), (Top)); \
  } while (0)

/*------------------------------------------------.
| Report that the YYRULE is going to be reduced.  |
`------------------------------------------------*/

static void yy_reduce_print(yy_state_t *yyssp, YYSTYPE *yyvsp, int yyrule) {
  int yylno = yyrline[yyrule];
  int yynrhs = yyr2[yyrule];
  int yyi;
  YYFPRINTF(stderr, "Reducing stack by rule %d (line %d):\n", yyrule - 1, yylno);
  /* The symbols being reduced.  */
  for (yyi = 0; yyi < yynrhs; yyi++) {
    YYFPRINTF(stderr, "   $%d = ", yyi + 1);
    yy_symbol_print(stderr, YY_ACCESSING_SYMBOL(+yyssp[yyi + 1 - yynrhs]), &yyvsp[(yyi + 1) - (yynrhs)]);
    YYFPRINTF(stderr, "\n");
  }
}

#define YY_REDUCE_PRINT(Rule)              \
  do {                                     \
    if (yydebug)                           \
      yy_reduce_print(yyssp, yyvsp, Rule); \
  } while (0)

/* Nonzero means print parse trace.  It is left uninitialized so that
   multiple parsers can coexist.  */
int yydebug;
#else /* !YYDEBUG */
#define YYDPRINTF(Args) ((void)0)
#define YY_SYMBOL_PRINT(Title, Kind, Value, Location)
#define YY_STACK_PRINT(Bottom, Top)
#define YY_REDUCE_PRINT(Rule)
#endif /* !YYDEBUG */

/* YYINITDEPTH -- initial size of the parser's stacks.  */
#ifndef YYINITDEPTH
#define YYINITDEPTH 200
#endif

/* YYMAXDEPTH -- maximum size the stacks can grow to (effective only
   if the built-in stack extension method is used).

   Do not make this value too large; the results are undefined if
   YYSTACK_ALLOC_MAXIMUM < YYSTACK_BYTES (YYMAXDEPTH)
   evaluated with infinite-precision integer arithmetic.  */

#ifndef YYMAXDEPTH
#define YYMAXDEPTH 10000
#endif

/*-----------------------------------------------.
| Release the memory associated to this symbol.  |
`-----------------------------------------------*/

static void yydestruct(const char *yymsg, yysymbol_kind_t yykind, YYSTYPE *yyvaluep) {
  YY_USE(yyvaluep);
  if (!yymsg)
    yymsg = "Deleting";
  YY_SYMBOL_PRINT(yymsg, yykind, yyvaluep, yylocationp);

  YY_IGNORE_MAYBE_UNINITIALIZED_BEGIN
  YY_USE(yykind);
  YY_IGNORE_MAYBE_UNINITIALIZED_END
}

/* Lookahead token kind.  */
int yychar;

/* The semantic value of the lookahead symbol.  */
YYSTYPE yylval;
/* Number of syntax errors so far.  */
int yynerrs;

/*----------.
| yyparse.  |
`----------*/

int yyparse(void) {
  yy_state_fast_t yystate = 0;
  /* Number of tokens to shift before error messages enabled.  */
  int yyerrstatus = 0;

  /* Refer to the stacks through separate pointers, to allow yyoverflow
     to reallocate them elsewhere.  */

  /* Their size.  */
  YYPTRDIFF_T yystacksize = YYINITDEPTH;

  /* The state stack: array, bottom, top.  */
  yy_state_t yyssa[YYINITDEPTH];
  yy_state_t *yyss = yyssa;
  yy_state_t *yyssp = yyss;

  /* The semantic value stack: array, bottom, top.  */
  YYSTYPE yyvsa[YYINITDEPTH];
  YYSTYPE *yyvs = yyvsa;
  YYSTYPE *yyvsp = yyvs;

  int yyn;
  /* The return value of yyparse.  */
  int yyresult;
  /* Lookahead symbol kind.  */
  yysymbol_kind_t yytoken = YYSYMBOL_YYEMPTY;
  /* The variables used to return semantic value and location from the
     action routines.  */
  YYSTYPE yyval;

#define YYPOPSTACK(N) (yyvsp -= (N), yyssp -= (N))

  /* The number of symbols on the RHS of the reduced rule.
     Keep to zero when no symbol should be popped.  */
  int yylen = 0;

  YYDPRINTF((stderr, "Starting parse\n"));

  yychar = YYEMPTY; /* Cause a token to be read.  */

  goto yysetstate;

/*------------------------------------------------------------.
| yynewstate -- push a new state, which is found in yystate.  |
`------------------------------------------------------------*/
yynewstate:
  /* In all cases, when you get here, the value and location stacks
     have just been pushed.  So pushing a state here evens the stacks.  */
  yyssp++;

/*--------------------------------------------------------------------.
| yysetstate -- set current state (the top of the stack) to yystate.  |
`--------------------------------------------------------------------*/
yysetstate:
  YYDPRINTF((stderr, "Entering state %d\n", yystate));
  YY_ASSERT(0 <= yystate && yystate < YYNSTATES);
  YY_IGNORE_USELESS_CAST_BEGIN
  *yyssp = YY_CAST(yy_state_t, yystate);
  YY_IGNORE_USELESS_CAST_END
  YY_STACK_PRINT(yyss, yyssp);

  if (yyss + yystacksize - 1 <= yyssp)
#if !defined yyoverflow && !defined YYSTACK_RELOCATE
    YYNOMEM;
#else
  {
    /* Get the current used size of the three stacks, in elements.  */
    YYPTRDIFF_T yysize = yyssp - yyss + 1;

#if defined yyoverflow
    {
      /* Give user a chance to reallocate the stack.  Use copies of
         these so that the &'s don't force the real ones into
         memory.  */
      yy_state_t *yyss1 = yyss;
      YYSTYPE *yyvs1 = yyvs;

      /* Each stack pointer address is followed by the size of the
         data in use in that stack, in bytes.  This used to be a
         conditional around just the two extra args, but that might
         be undefined if yyoverflow is a macro.  */
      yyoverflow(YY_("memory exhausted"), &yyss1, yysize * YYSIZEOF(*yyssp), &yyvs1, yysize * YYSIZEOF(*yyvsp),
                 &yystacksize);
      yyss = yyss1;
      yyvs = yyvs1;
    }
#else /* defined YYSTACK_RELOCATE */
    /* Extend the stack our own way.  */
    if (YYMAXDEPTH <= yystacksize)
      YYNOMEM;
    yystacksize *= 2;
    if (YYMAXDEPTH < yystacksize)
      yystacksize = YYMAXDEPTH;

    {
      yy_state_t *yyss1 = yyss;
      union yyalloc *yyptr = YY_CAST(union yyalloc *, YYSTACK_ALLOC(YY_CAST(YYSIZE_T, YYSTACK_BYTES(yystacksize))));
      if (!yyptr)
        YYNOMEM;
      YYSTACK_RELOCATE(yyss_alloc, yyss);
      YYSTACK_RELOCATE(yyvs_alloc, yyvs);
#undef YYSTACK_RELOCATE
      if (yyss1 != yyssa)
        YYSTACK_FREE(yyss1);
    }
#endif

    yyssp = yyss + yysize - 1;
    yyvsp = yyvs + yysize - 1;

    YY_IGNORE_USELESS_CAST_BEGIN
    YYDPRINTF((stderr, "Stack size increased to %ld\n", YY_CAST(long, yystacksize)));
    YY_IGNORE_USELESS_CAST_END

    if (yyss + yystacksize - 1 <= yyssp)
      YYABORT;
  }
#endif /* !defined yyoverflow && !defined YYSTACK_RELOCATE */

  if (yystate == YYFINAL)
    YYACCEPT;

  goto yybackup;

/*-----------.
| yybackup.  |
`-----------*/
yybackup:
  /* Do appropriate processing given the current state.  Read a
     lookahead token if we need one and don't already have one.  */

  /* First try to decide what to do without reference to lookahead token.  */
  yyn = yypact[yystate];
  if (yypact_value_is_default(yyn))
    goto yydefault;

  /* Not known => get a lookahead token if don't already have one.  */

  /* YYCHAR is either empty, or end-of-input, or a valid lookahead.  */
  if (yychar == YYEMPTY) {
    YYDPRINTF((stderr, "Reading a token\n"));
    yychar = yylex();
  }

  if (yychar <= YYEOF) {
    yychar = YYEOF;
    yytoken = YYSYMBOL_YYEOF;
    YYDPRINTF((stderr, "Now at end of input.\n"));
  } else if (yychar == YYerror) {
    /* The scanner already issued an error message, process directly
       to error recovery.  But do not keep the error token as
       lookahead, it is too special and may lead us to an endless
       loop in error recovery. */
    yychar = YYUNDEF;
    yytoken = YYSYMBOL_YYerror;
    goto yyerrlab1;
  } else {
    yytoken = YYTRANSLATE(yychar);
    YY_SYMBOL_PRINT("Next token is", yytoken, &yylval, &yylloc);
  }

  /* If the proper action on seeing token YYTOKEN is to reduce or to
     detect an error, take that action.  */
  yyn += yytoken;
  if (yyn < 0 || YYLAST < yyn || yycheck[yyn] != yytoken)
    goto yydefault;
  yyn = yytable[yyn];
  if (yyn <= 0) {
    if (yytable_value_is_error(yyn))
      goto yyerrlab;
    yyn = -yyn;
    goto yyreduce;
  }

  /* Count tokens shifted since error; after three, turn off error
     status.  */
  if (yyerrstatus)
    yyerrstatus--;

  /* Shift the lookahead token.  */
  YY_SYMBOL_PRINT("Shifting", yytoken, &yylval, &yylloc);
  yystate = yyn;
  YY_IGNORE_MAYBE_UNINITIALIZED_BEGIN
  *++yyvsp = yylval;
  YY_IGNORE_MAYBE_UNINITIALIZED_END

  /* Discard the shifted token.  */
  yychar = YYEMPTY;
  goto yynewstate;

/*-----------------------------------------------------------.
| yydefault -- do the default action for the current state.  |
`-----------------------------------------------------------*/
yydefault:
  yyn = yydefact[yystate];
  if (yyn == 0)
    goto yyerrlab;
  goto yyreduce;

/*-----------------------------.
| yyreduce -- do a reduction.  |
`-----------------------------*/
yyreduce:
  /* yyn is the number of a rule to reduce with.  */
  yylen = yyr2[yyn];

  /* If YYLEN is nonzero, implement the default value of the action:
     '$$ = $1'.

     Otherwise, the following line sets YYVAL to garbage.
     This behavior is undocumented and Bison
     users should not rely upon it.  Assigning to YYVAL
     unconditionally makes the parser a bit smaller, and it avoids a
     GCC warning that YYVAL may be used uninitialized.  */
  yyval = yyvsp[1 - yylen];

  YY_REDUCE_PRINT(yyn);
  switch (yyn) {
  case 2: /* fulleq: DPTT_eoq  */
#line 86 "DYacc.y"
  {
    return DPTT_eoq;
  }
#line 1246 "DYacc.tab.cpp"
  break;

  case 3: /* fulleq: DPTT_groupstar  */
#line 87 "DYacc.y"
  {
    return DPTT_groupstar;
  }
#line 1252 "DYacc.tab.cpp"
  break;

  case 4: /* fulleq: DPTT_specs  */
#line 88 "DYacc.y"
  {
    return DPTT_specs;
  }
#line 1258 "DYacc.tab.cpp"
  break;

  case 5: /* fulleq: DPTT_save  */
#line 89 "DYacc.y"
  {
    return DPTT_save;
  }
#line 1264 "DYacc.tab.cpp"
  break;

  case 6: /* fulleq: DPTT_table teqn DPTT_eoq  */
#line 90 "DYacc.y"
  {
    dpyy_addfulleq((yyvsp[-1].eqn), DPTT_table);
    return DPTT_eoq;
  }
#line 1270 "DYacc.tab.cpp"
  break;

  case 7: /* fulleq: DPTT_constant eqn DPTT_eoq  */
#line 91 "DYacc.y"
  {
    dpyy_addfulleq((yyvsp[-1].eqn), DPTT_constant);
    return DPTT_eoq;
  }
#line 1276 "DYacc.tab.cpp"
  break;

  case 8: /* fulleq: DPTT_init eqn DPTT_eoq  */
#line 92 "DYacc.y"
  {
    dpyy_addfulleq((yyvsp[-1].eqn), DPTT_init);
    return DPTT_eoq;
  }
#line 1282 "DYacc.tab.cpp"
  break;

  case 9: /* fulleq: DPTT_level stock_eqn DPTT_eoq  */
#line 93 "DYacc.y"
  {
    dpyy_addfulleq((yyvsp[-1].eqn), DPTT_level);
    return DPTT_eoq;
  }
#line 1288 "DYacc.tab.cpp"
  break;

  case 10: /* fulleq: DPTT_aux eqn DPTT_eoq  */
#line 94 "DYacc.y"
  {
    dpyy_addfulleq((yyvsp[-1].eqn), DPTT_aux);
    return DPTT_eoq;
  }
#line 1294 "DYacc.tab.cpp"
  break;

  case 11: /* teqn: lhs '=' tabledef  */
#line 98 "DYacc.y"
  {
    (yyval.eqn) = dpyy_add_lookup((yyvsp[-2].lhs), NULL, (yyvsp[0].tbl), 0);
  }
#line 1300 "DYacc.tab.cpp"
  break;

  case 12: /* tabledef: number  */
#line 103 "DYacc.y"
  {
    (yyval.tbl) = dpyy_tablevec(NULL, (yyvsp[0].num));
  }
#line 1306 "DYacc.tab.cpp"
  break;

  case 13: /* tabledef: tabledef ',' number  */
#line 104 "DYacc.y"
  {
    (yyval.tbl) = dpyy_tablevec((yyvsp[-2].tbl), (yyvsp[0].num));
  }
#line 1312 "DYacc.tab.cpp"
  break;

  case 14: /* tabledef: tabledef '/' number  */
#line 105 "DYacc.y"
  {
    (yyval.tbl) = dpyy_tablevec((yyvsp[-2].tbl), (yyvsp[0].num));
  }
#line 1318 "DYacc.tab.cpp"
  break;

  case 15: /* eqn: lhs '=' exprlist  */
#line 112 "DYacc.y"
  {
    (yyval.eqn) = dpyy_addeq((yyvsp[-2].lhs), NULL, (yyvsp[0].exl), '=');
  }
#line 1324 "DYacc.tab.cpp"
  break;

  case 16: /* eqn: lhs '(' tablevals ')'  */
#line 113 "DYacc.y"
  {
    (yyval.eqn) = dpyy_add_lookup((yyvsp[-3].lhs), NULL, (yyvsp[-1].tbl), 0);
  }
#line 1330 "DYacc.tab.cpp"
  break;

  case 17: /* eqn: lhs '(' xytablevals ')'  */
#line 114 "DYacc.y"
  {
    (yyval.eqn) = dpyy_add_lookup((yyvsp[-3].lhs), NULL, (yyvsp[-1].tbl), 1);
  }
#line 1336 "DYacc.tab.cpp"
  break;

  case 18: /* eqn: lhs  */
#line 115 "DYacc.y"
  {
    (yyval.eqn) = dpyy_add_lookup((yyvsp[0].lhs), NULL, NULL, 0);
  }
#line 1342 "DYacc.tab.cpp"
  break;

  case 19: /* eqn: DPTT_symbol ':' subdef maplist  */
#line 116 "DYacc.y"
  {
    (yyval.eqn) = dpyy_addeq(dpyy_addexceptinterp(dpyy_var_expression((yyvsp[-3].sym), NULL), NULL, 0),
                             (Expression *)dpyy_symlist_expression((yyvsp[-1].sml), (yyvsp[0].sml)), NULL, ':');
  }
#line 1348 "DYacc.tab.cpp"
  break;

  case 20: /* stock_eqn: lhs '=' var '+' exprlist  */
#line 120 "DYacc.y"
  {
    (yyval.eqn) = dpyy_addstockeq((yyvsp[-4].lhs), (yyvsp[-2].var), (yyvsp[0].exl), '=');
  }
#line 1354 "DYacc.tab.cpp"
  break;

  case 21: /* lhs: var  */
#line 127 "DYacc.y"
  {
    (yyval.lhs) = dpyy_addexceptinterp((yyvsp[0].var), NULL, 0);
  }
#line 1360 "DYacc.tab.cpp"
  break;

  case 22: /* var: DPTT_symbol  */
#line 131 "DYacc.y"
  {
    (yyval.var) = dpyy_var_expression((yyvsp[0].sym), NULL);
  }
#line 1366 "DYacc.tab.cpp"
  break;

  case 23: /* var: DPTT_symbol sublist  */
#line 132 "DYacc.y"
  {
    (yyval.var) = dpyy_var_expression((yyvsp[-1].sym), (yyvsp[0].sml));
  }
#line 1372 "DYacc.tab.cpp"
  break;

  case 24: /* sublist: '[' symlist ']'  */
#line 136 "DYacc.y"
  {
    (yyval.sml) = (yyvsp[-1].sml);
  }
#line 1378 "DYacc.tab.cpp"
  break;

  case 25: /* symlist: DPTT_symbol  */
#line 140 "DYacc.y"
  {
    (yyval.sml) = dpyy_symlist(NULL, (yyvsp[0].sym), 0, NULL);
  }
#line 1384 "DYacc.tab.cpp"
  break;

  case 26: /* symlist: DPTT_symbol '!'  */
#line 141 "DYacc.y"
  {
    (yyval.sml) = dpyy_symlist(NULL, (yyvsp[-1].sym), 1, NULL);
  }
#line 1390 "DYacc.tab.cpp"
  break;

  case 27: /* symlist: symlist ',' DPTT_symbol  */
#line 142 "DYacc.y"
  {
    (yyval.sml) = dpyy_symlist((yyvsp[-2].sml), (yyvsp[0].sym), 0, NULL);
  }
#line 1396 "DYacc.tab.cpp"
  break;

  case 28: /* symlist: symlist ',' DPTT_symbol '!'  */
#line 143 "DYacc.y"
  {
    (yyval.sml) = dpyy_symlist((yyvsp[-3].sml), (yyvsp[-1].sym), 1, NULL);
  }
#line 1402 "DYacc.tab.cpp"
  break;

  case 29: /* subdef: DPTT_symbol  */
#line 146 "DYacc.y"
  {
    (yyval.sml) = dpyy_symlist(NULL, (yyvsp[0].sym), 0, NULL);
  }
#line 1408 "DYacc.tab.cpp"
  break;

  case 30: /* subdef: '(' DPTT_symbol '-' DPTT_symbol ')'  */
#line 147 "DYacc.y"
  {
    (yyval.sml) = dpyy_symlist(NULL, (yyvsp[-3].sym), 0, (yyvsp[-1].sym));
  }
#line 1414 "DYacc.tab.cpp"
  break;

  case 31: /* subdef: subdef ',' DPTT_symbol  */
#line 148 "DYacc.y"
  {
    (yyval.sml) = dpyy_symlist((yyvsp[-2].sml), (yyvsp[0].sym), 0, NULL);
  }
#line 1420 "DYacc.tab.cpp"
  break;

  case 32: /* subdef: subdef ',' '(' DPTT_symbol '-' DPTT_symbol ')'  */
#line 149 "DYacc.y"
  {
    (yyval.sml) = dpyy_symlist((yyvsp[-6].sml), (yyvsp[-3].sym), 0, (yyvsp[-1].sym));
  }
#line 1426 "DYacc.tab.cpp"
  break;

  case 33: /* number: DPTT_number  */
#line 153 "DYacc.y"
  {
    (yyval.num) = (yyvsp[0].num);
  }
#line 1432 "DYacc.tab.cpp"
  break;

  case 34: /* number: '-' DPTT_number  */
#line 154 "DYacc.y"
  {
    (yyval.num) = -(yyvsp[0].num);
  }
#line 1438 "DYacc.tab.cpp"
  break;

  case 35: /* number: '+' DPTT_number  */
#line 155 "DYacc.y"
  {
    (yyval.num) = (yyvsp[0].num);
  }
#line 1444 "DYacc.tab.cpp"
  break;

  case 36: /* maplist: %empty  */
#line 160 "DYacc.y"
  {
    (yyval.sml) = NULL;
  }
#line 1450 "DYacc.tab.cpp"
  break;

  case 37: /* exprlist: exp  */
#line 165 "DYacc.y"
  {
    (yyval.exl) = dpyy_chain_exprlist(NULL, (yyvsp[0].exn));
  }
#line 1456 "DYacc.tab.cpp"
  break;

  case 38: /* exprlist: exprlist ',' exp  */
#line 166 "DYacc.y"
  {
    (yyval.exl) = dpyy_chain_exprlist((yyvsp[-2].exl), (yyvsp[0].exn));
  }
#line 1462 "DYacc.tab.cpp"
  break;

  case 39: /* exprlist: exprlist ';' exp  */
#line 167 "DYacc.y"
  {
    (yyval.exl) = dpyy_chain_exprlist((yyvsp[-2].exl), (yyvsp[0].exn));
  }
#line 1468 "DYacc.tab.cpp"
  break;

  case 40: /* exprlist: exprlist ';'  */
#line 168 "DYacc.y"
  {
    (yyval.exl) = (yyvsp[-1].exl);
  }
#line 1474 "DYacc.tab.cpp"
  break;

  case 41: /* exp: DPTT_number  */
#line 172 "DYacc.y"
  {
    (yyval.exn) = dpyy_num_expression((yyvsp[0].num));
  }
#line 1480 "DYacc.tab.cpp"
  break;

  case 42: /* exp: var  */
#line 173 "DYacc.y"
  {
    (yyval.exn) = (Expression *)(yyvsp[0].var);
  }
#line 1486 "DYacc.tab.cpp"
  break;

  case 43: /* exp: var '(' exprlist ')'  */
#line 174 "DYacc.y"
  {
    (yyval.exn) = dpyy_lookup_expression((yyvsp[-3].var), (yyvsp[-1].exl));
  }
#line 1492 "DYacc.tab.cpp"
  break;

  case 44: /* exp: '(' exp ')'  */
#line 175 "DYacc.y"
  {
    (yyval.exn) = dpyy_operator_expression('(', (yyvsp[-1].exn), NULL);
  }
#line 1498 "DYacc.tab.cpp"
  break;

  case 45: /* exp: DPTT_function '(' exprlist ')'  */
#line 176 "DYacc.y"
  {
    (yyval.exn) = dpyy_function_expression((yyvsp[-3].fnc), (yyvsp[-1].exl));
  }
#line 1504 "DYacc.tab.cpp"
  break;

  case 46: /* exp: DPTT_function '(' exprlist ',' ')'  */
#line 177 "DYacc.y"
  {
    (yyval.exn) =
        dpyy_function_expression((yyvsp[-4].fnc), dpyy_chain_exprlist((yyvsp[-2].exl), dpyy_literal_expression("?")));
  }
#line 1510 "DYacc.tab.cpp"
  break;

  case 47: /* exp: DPTT_function '(' ')'  */
#line 178 "DYacc.y"
  {
    (yyval.exn) = dpyy_function_expression((yyvsp[-2].fnc), NULL);
  }
#line 1516 "DYacc.tab.cpp"
  break;

  case 48: /* exp: exp '+' exp  */
#line 179 "DYacc.y"
  {
    (yyval.exn) = dpyy_operator_expression('+', (yyvsp[-2].exn), (yyvsp[0].exn));
  }
#line 1522 "DYacc.tab.cpp"
  break;

  case 49: /* exp: exp '-' exp  */
#line 180 "DYacc.y"
  {
    (yyval.exn) = dpyy_operator_expression('-', (yyvsp[-2].exn), (yyvsp[0].exn));
  }
#line 1528 "DYacc.tab.cpp"
  break;

  case 50: /* exp: exp '*' exp  */
#line 181 "DYacc.y"
  {
    (yyval.exn) = dpyy_operator_expression('*', (yyvsp[-2].exn), (yyvsp[0].exn));
  }
#line 1534 "DYacc.tab.cpp"
  break;

  case 51: /* exp: exp '/' exp  */
#line 182 "DYacc.y"
  {
    (yyval.exn) = dpyy_operator_expression('/', (yyvsp[-2].exn), (yyvsp[0].exn));
  }
#line 1540 "DYacc.tab.cpp"
  break;

  case 52: /* exp: exp '<' exp  */
#line 183 "DYacc.y"
  {
    (yyval.exn) = dpyy_operator_expression('<', (yyvsp[-2].exn), (yyvsp[0].exn));
  }
#line 1546 "DYacc.tab.cpp"
  break;

  case 53: /* exp: exp DPTT_le exp  */
#line 184 "DYacc.y"
  {
    (yyval.exn) = dpyy_operator_expression(DPTT_le, (yyvsp[-2].exn), (yyvsp[0].exn));
  }
#line 1552 "DYacc.tab.cpp"
  break;

  case 54: /* exp: exp '>' exp  */
#line 185 "DYacc.y"
  {
    (yyval.exn) = dpyy_operator_expression('>', (yyvsp[-2].exn), (yyvsp[0].exn));
  }
#line 1558 "DYacc.tab.cpp"
  break;

  case 55: /* exp: exp DPTT_ge exp  */
#line 186 "DYacc.y"
  {
    (yyval.exn) = dpyy_operator_expression(DPTT_ge, (yyvsp[-2].exn), (yyvsp[0].exn));
  }
#line 1564 "DYacc.tab.cpp"
  break;

  case 56: /* exp: exp DPTT_ne exp  */
#line 187 "DYacc.y"
  {
    (yyval.exn) = dpyy_operator_expression(DPTT_ne, (yyvsp[-2].exn), (yyvsp[0].exn));
  }
#line 1570 "DYacc.tab.cpp"
  break;

  case 57: /* exp: exp DPTT_or exp  */
#line 188 "DYacc.y"
  {
    (yyval.exn) = dpyy_operator_expression(DPTT_or, (yyvsp[-2].exn), (yyvsp[0].exn));
  }
#line 1576 "DYacc.tab.cpp"
  break;

  case 58: /* exp: exp DPTT_and exp  */
#line 189 "DYacc.y"
  {
    (yyval.exn) = dpyy_operator_expression(DPTT_and, (yyvsp[-2].exn), (yyvsp[0].exn));
  }
#line 1582 "DYacc.tab.cpp"
  break;

  case 59: /* exp: DPTT_not exp  */
#line 190 "DYacc.y"
  {
    (yyval.exn) = dpyy_operator_expression(DPTT_not, (yyvsp[0].exn), NULL);
  }
#line 1588 "DYacc.tab.cpp"
  break;

  case 60: /* exp: exp '=' exp  */
#line 191 "DYacc.y"
  {
    (yyval.exn) = dpyy_operator_expression('=', (yyvsp[-2].exn), (yyvsp[0].exn));
  }
#line 1594 "DYacc.tab.cpp"
  break;

  case 61: /* exp: '-' exp  */
#line 192 "DYacc.y"
  {
    (yyval.exn) = dpyy_operator_expression('-', NULL, (yyvsp[0].exn));
  }
#line 1600 "DYacc.tab.cpp"
  break;

  case 62: /* exp: '+' exp  */
#line 193 "DYacc.y"
  {
    (yyval.exn) = dpyy_operator_expression('+', NULL, (yyvsp[0].exn));
  }
#line 1606 "DYacc.tab.cpp"
  break;

  case 63: /* exp: exp '^' exp  */
#line 194 "DYacc.y"
  {
    (yyval.exn) = dpyy_operator_expression('^', (yyvsp[-2].exn), (yyvsp[0].exn));
  }
#line 1612 "DYacc.tab.cpp"
  break;

  case 64: /* tablevals: tablepairs  */
#line 198 "DYacc.y"
  {
    (yyval.tbl) = (yyvsp[0].tbl);
  }
#line 1618 "DYacc.tab.cpp"
  break;

  case 65: /* tablevals: '[' '(' number ',' number ')' '-' '(' number ',' number ')' ']' ',' tablepairs  */
#line 200 "DYacc.y"
  {
    (yyval.tbl) = dpyy_tablerange((yyvsp[0].tbl), (yyvsp[-12].num), (yyvsp[-10].num), (yyvsp[-6].num), (yyvsp[-4].num));
  }
#line 1624 "DYacc.tab.cpp"
  break;

  case 66: /* tablevals: '[' '(' number ',' number ')' '-' '(' number ',' number ')' ',' tablepairs ']' ',' tablepairs
            */
#line 202 "DYacc.y"
  {
    (yyval.tbl) = dpyy_tablerange((yyvsp[0].tbl), (yyvsp[-14].num), (yyvsp[-12].num), (yyvsp[-8].num), (yyvsp[-6].num));
  }
#line 1630 "DYacc.tab.cpp"
  break;

  case 67: /* xytablevals: xytablevec  */
#line 206 "DYacc.y"
  {
    (yyval.tbl) = (yyvsp[0].tbl);
  }
#line 1636 "DYacc.tab.cpp"
  break;

  case 68: /* xytablevals: '[' '(' number ',' number ')' '-' '(' number ',' number ')' ']' ',' xytablevec  */
#line 208 "DYacc.y"
  {
    (yyval.tbl) = dpyy_tablerange((yyvsp[0].tbl), (yyvsp[-12].num), (yyvsp[-10].num), (yyvsp[-6].num), (yyvsp[-4].num));
  }
#line 1642 "DYacc.tab.cpp"
  break;

  case 69: /* xytablevec: number  */
#line 212 "DYacc.y"
  {
    (yyval.tbl) = dpyy_tablevec(NULL, (yyvsp[0].num));
  }
#line 1648 "DYacc.tab.cpp"
  break;

  case 70: /* xytablevec: xytablevec ',' number  */
#line 213 "DYacc.y"
  {
    (yyval.tbl) = dpyy_tablevec((yyvsp[-2].tbl), (yyvsp[0].num));
  }
#line 1654 "DYacc.tab.cpp"
  break;

  case 71: /* tablepairs: '(' number ',' number ')'  */
#line 218 "DYacc.y"
  {
    (yyval.tbl) = dpyy_tablepair(NULL, (yyvsp[-3].num), (yyvsp[-1].num));
  }
#line 1660 "DYacc.tab.cpp"
  break;

  case 72: /* tablepairs: tablepairs ',' '(' number ',' number ')'  */
#line 219 "DYacc.y"
  {
    (yyval.tbl) = dpyy_tablepair((yyvsp[-6].tbl), (yyvsp[-3].num), (yyvsp[-1].num));
  }
#line 1666 "DYacc.tab.cpp"
  break;

#line 1670 "DYacc.tab.cpp"

  default:
    break;
  }
  /* User semantic actions sometimes alter yychar, and that requires
     that yytoken be updated with the new translation.  We take the
     approach of translating immediately before every use of yytoken.
     One alternative is translating here after every semantic action,
     but that translation would be missed if the semantic action invokes
     YYABORT, YYACCEPT, or YYERROR immediately after altering yychar or
     if it invokes YYBACKUP.  In the case of YYABORT or YYACCEPT, an
     incorrect destructor might then be invoked immediately.  In the
     case of YYERROR or YYBACKUP, subsequent parser actions might lead
     to an incorrect destructor call or verbose syntax error message
     before the lookahead is translated.  */
  YY_SYMBOL_PRINT("-> $$ =", YY_CAST(yysymbol_kind_t, yyr1[yyn]), &yyval, &yyloc);

  YYPOPSTACK(yylen);
  yylen = 0;

  *++yyvsp = yyval;

  /* Now 'shift' the result of the reduction.  Determine what state
     that goes to, based on the state we popped back to and the rule
     number reduced by.  */
  {
    const int yylhs = yyr1[yyn] - YYNTOKENS;
    const int yyi = yypgoto[yylhs] + *yyssp;
    yystate = (0 <= yyi && yyi <= YYLAST && yycheck[yyi] == *yyssp ? yytable[yyi] : yydefgoto[yylhs]);
  }

  goto yynewstate;

/*--------------------------------------.
| yyerrlab -- here on detecting error.  |
`--------------------------------------*/
yyerrlab:
  /* Make sure we have latest lookahead translation.  See comments at
     user semantic actions for why this is necessary.  */
  yytoken = yychar == YYEMPTY ? YYSYMBOL_YYEMPTY : YYTRANSLATE(yychar);
  /* If not already recovering from an error, report this error.  */
  if (!yyerrstatus) {
    ++yynerrs;
    yyerror(YY_("syntax error"));
  }

  if (yyerrstatus == 3) {
    /* If just tried and failed to reuse lookahead token after an
       error, discard it.  */

    if (yychar <= YYEOF) {
      /* Return failure if at end of input.  */
      if (yychar == YYEOF)
        YYABORT;
    } else {
      yydestruct("Error: discarding", yytoken, &yylval);
      yychar = YYEMPTY;
    }
  }

  /* Else will try to reuse lookahead token after shifting the error
     token.  */
  goto yyerrlab1;

/*---------------------------------------------------.
| yyerrorlab -- error raised explicitly by YYERROR.  |
`---------------------------------------------------*/
yyerrorlab:
  /* Pacify compilers when the user code never invokes YYERROR and the
     label yyerrorlab therefore never appears in user code.  */
  if (0)
    YYERROR;
  ++yynerrs;

  /* Do not reclaim the symbols of the rule whose action triggered
     this YYERROR.  */
  YYPOPSTACK(yylen);
  yylen = 0;
  YY_STACK_PRINT(yyss, yyssp);
  yystate = *yyssp;
  goto yyerrlab1;

/*-------------------------------------------------------------.
| yyerrlab1 -- common code for both syntax error and YYERROR.  |
`-------------------------------------------------------------*/
yyerrlab1:
  yyerrstatus = 3; /* Each real token shifted decrements this.  */

  /* Pop stack until we find a state that shifts the error token.  */
  for (;;) {
    yyn = yypact[yystate];
    if (!yypact_value_is_default(yyn)) {
      yyn += YYSYMBOL_YYerror;
      if (0 <= yyn && yyn <= YYLAST && yycheck[yyn] == YYSYMBOL_YYerror) {
        yyn = yytable[yyn];
        if (0 < yyn)
          break;
      }
    }

    /* Pop the current state because it cannot handle the error token.  */
    if (yyssp == yyss)
      YYABORT;

    yydestruct("Error: popping", YY_ACCESSING_SYMBOL(yystate), yyvsp);
    YYPOPSTACK(1);
    yystate = *yyssp;
    YY_STACK_PRINT(yyss, yyssp);
  }

  YY_IGNORE_MAYBE_UNINITIALIZED_BEGIN
  *++yyvsp = yylval;
  YY_IGNORE_MAYBE_UNINITIALIZED_END

  /* Shift the error token.  */
  YY_SYMBOL_PRINT("Shifting", YY_ACCESSING_SYMBOL(yyn), yyvsp, yylsp);

  yystate = yyn;
  goto yynewstate;

/*-------------------------------------.
| yyacceptlab -- YYACCEPT comes here.  |
`-------------------------------------*/
yyacceptlab:
  yyresult = 0;
  goto yyreturnlab;

/*-----------------------------------.
| yyabortlab -- YYABORT comes here.  |
`-----------------------------------*/
yyabortlab:
  yyresult = 1;
  goto yyreturnlab;

/*-----------------------------------------------------------.
| yyexhaustedlab -- YYNOMEM (memory exhaustion) comes here.  |
`-----------------------------------------------------------*/
yyexhaustedlab:
  yyerror(YY_("memory exhausted"));
  yyresult = 2;
  goto yyreturnlab;

/*----------------------------------------------------------.
| yyreturnlab -- parsing is finished, clean up and return.  |
`----------------------------------------------------------*/
yyreturnlab:
  if (yychar != YYEMPTY) {
    /* Make sure we have latest lookahead translation.  See comments at
       user semantic actions for why this is necessary.  */
    yytoken = YYTRANSLATE(yychar);
    yydestruct("Cleanup: discarding lookahead", yytoken, &yylval);
  }
  /* Do not reclaim the symbols of the rule whose action triggered
     this YYABORT or YYACCEPT.  */
  YYPOPSTACK(yylen);
  YY_STACK_PRINT(yyss, yyssp);
  while (yyssp != yyss) {
    yydestruct("Cleanup: popping", YY_ACCESSING_SYMBOL(+*yyssp), yyvsp);
    YYPOPSTACK(1);
  }
#ifndef yyoverflow
  if (yyss != yyssa)
    YYSTACK_FREE(yyss);
#endif

  return yyresult;
}

#line 225 "DYacc.y"
