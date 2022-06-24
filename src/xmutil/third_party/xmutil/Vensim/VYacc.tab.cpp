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
#define yyparse vpyyparse
#define yylex vpyylex
#define yyerror vpyyerror
#define yydebug vpyydebug
#define yynerrs vpyynerrs
#define yylval vpyylval
#define yychar vpyychar

/* First part of user prologue.  */
#line 10 "VYacc.y"

#include "../Log.h"
#include "../Symbol/Parse.h"
#include "VensimParseFunctions.h"
extern int vpyylex(void);
extern void vpyyerror(char const *);
#define YYSTYPE ParseUnion
#define YYFPRINTF XmutilLogf

#line 88 "VYacc.tab.cpp"

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

#include "VYacc.tab.hpp"
/* Symbol kind.  */
enum yysymbol_kind_t {
  YYSYMBOL_YYEMPTY = -2,
  YYSYMBOL_YYEOF = 0,               /* "end of file"  */
  YYSYMBOL_YYerror = 1,             /* error  */
  YYSYMBOL_YYUNDEF = 2,             /* "invalid token"  */
  YYSYMBOL_VPTT_dataequals = 3,     /* VPTT_dataequals  */
  YYSYMBOL_VPTT_with_lookup = 4,    /* VPTT_with_lookup  */
  YYSYMBOL_VPTT_map = 5,            /* VPTT_map  */
  YYSYMBOL_VPTT_equiv = 6,          /* VPTT_equiv  */
  YYSYMBOL_VPTT_groupstar = 7,      /* VPTT_groupstar  */
  YYSYMBOL_VPTT_and = 8,            /* VPTT_and  */
  YYSYMBOL_VPTT_macro = 9,          /* VPTT_macro  */
  YYSYMBOL_VPTT_end_of_macro = 10,  /* VPTT_end_of_macro  */
  YYSYMBOL_VPTT_or = 11,            /* VPTT_or  */
  YYSYMBOL_VPTT_not = 12,           /* VPTT_not  */
  YYSYMBOL_VPTT_hold_backward = 13, /* VPTT_hold_backward  */
  YYSYMBOL_VPTT_look_forward = 14,  /* VPTT_look_forward  */
  YYSYMBOL_VPTT_except = 15,        /* VPTT_except  */
  YYSYMBOL_VPTT_na = 16,            /* VPTT_na  */
  YYSYMBOL_VPTT_interpolate = 17,   /* VPTT_interpolate  */
  YYSYMBOL_VPTT_raw = 18,           /* VPTT_raw  */
  YYSYMBOL_VPTT_test_input = 19,    /* VPTT_test_input  */
  YYSYMBOL_VPTT_the_condition = 20, /* VPTT_the_condition  */
  YYSYMBOL_VPTT_implies = 21,       /* VPTT_implies  */
  YYSYMBOL_VPTT_ge = 22,            /* VPTT_ge  */
  YYSYMBOL_VPTT_le = 23,            /* VPTT_le  */
  YYSYMBOL_VPTT_ne = 24,            /* VPTT_ne  */
  YYSYMBOL_VPTT_tabbed_array = 25,  /* VPTT_tabbed_array  */
  YYSYMBOL_VPTT_eqend = 26,         /* VPTT_eqend  */
  YYSYMBOL_VPTT_number = 27,        /* VPTT_number  */
  YYSYMBOL_VPTT_literal = 28,       /* VPTT_literal  */
  YYSYMBOL_VPTT_symbol = 29,        /* VPTT_symbol  */
  YYSYMBOL_VPTT_units_symbol = 30,  /* VPTT_units_symbol  */
  YYSYMBOL_VPTT_function = 31,      /* VPTT_function  */
  YYSYMBOL_32_ = 32,                /* '%'  */
  YYSYMBOL_33_ = 33,                /* '|'  */
  YYSYMBOL_34_ = 34,                /* '-'  */
  YYSYMBOL_35_ = 35,                /* '+'  */
  YYSYMBOL_36_ = 36,                /* '='  */
  YYSYMBOL_37_ = 37,                /* '<'  */
  YYSYMBOL_38_ = 38,                /* '>'  */
  YYSYMBOL_39_ = 39,                /* '*'  */
  YYSYMBOL_40_ = 40,                /* '/'  */
  YYSYMBOL_41_ = 41,                /* '^'  */
  YYSYMBOL_42_ = 42,                /* '~'  */
  YYSYMBOL_43_ = 43,                /* '('  */
  YYSYMBOL_44_ = 44,                /* ')'  */
  YYSYMBOL_45_ = 45,                /* ','  */
  YYSYMBOL_46_ = 46,                /* ':'  */
  YYSYMBOL_47_ = 47,                /* '['  */
  YYSYMBOL_48_ = 48,                /* ']'  */
  YYSYMBOL_49_ = 49,                /* '!'  */
  YYSYMBOL_50_ = 50,                /* '?'  */
  YYSYMBOL_51_ = 51,                /* ';'  */
  YYSYMBOL_YYACCEPT = 52,           /* $accept  */
  YYSYMBOL_fulleq = 53,             /* fulleq  */
  YYSYMBOL_macrostart = 54,         /* macrostart  */
  YYSYMBOL_55_1 = 55,               /* $@1  */
  YYSYMBOL_macroend = 56,           /* macroend  */
  YYSYMBOL_eqn = 57,                /* eqn  */
  YYSYMBOL_lhs = 58,                /* lhs  */
  YYSYMBOL_var = 59,                /* var  */
  YYSYMBOL_sublist = 60,            /* sublist  */
  YYSYMBOL_symlist = 61,            /* symlist  */
  YYSYMBOL_subdef = 62,             /* subdef  */
  YYSYMBOL_unitsrange = 63,         /* unitsrange  */
  YYSYMBOL_urangenum = 64,          /* urangenum  */
  YYSYMBOL_number = 65,             /* number  */
  YYSYMBOL_units = 66,              /* units  */
  YYSYMBOL_interpmode = 67,         /* interpmode  */
  YYSYMBOL_exceptlist = 68,         /* exceptlist  */
  YYSYMBOL_mapsymlist = 69,         /* mapsymlist  */
  YYSYMBOL_maplist = 70,            /* maplist  */
  YYSYMBOL_exprlist = 71,           /* exprlist  */
  YYSYMBOL_exp = 72,                /* exp  */
  YYSYMBOL_tablevals = 73,          /* tablevals  */
  YYSYMBOL_xytablevals = 74,        /* xytablevals  */
  YYSYMBOL_xytablevec = 75,         /* xytablevec  */
  YYSYMBOL_tablepairs = 76          /* tablepairs  */
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
void free(void *);      /* INFRINGES ON USER NAME SPACE */
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
#define YYFINAL 17
/* YYLAST -- Last index in YYTABLE.  */
#define YYLAST 318

/* YYNTOKENS -- Number of terminals.  */
#define YYNTOKENS 52
/* YYNNTS -- Number of nonterminals.  */
#define YYNNTS 25
/* YYNRULES -- Number of rules.  */
#define YYNRULES 99
/* YYNSTATES -- Number of states.  */
#define YYNSTATES 232

/* YYMAXUTOK -- Last valid token kind.  */
#define YYMAXUTOK 286

/* YYTRANSLATE(TOKEN-NUM) -- Symbol number corresponding to TOKEN-NUM
   as returned by yylex, with out-of-bounds checking.  */
#define YYTRANSLATE(YYX) \
  (0 <= (YYX) && (YYX) <= YYMAXUTOK ? YY_CAST(yysymbol_kind_t, yytranslate[YYX]) : YYSYMBOL_YYUNDEF)

/* YYTRANSLATE[TOKEN-NUM] -- Symbol number corresponding to TOKEN-NUM
   as returned by yylex.  */
static const yytype_int8 yytranslate[] = {
    0,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2, 2, 2, 2,
    2,  2,  2,  2,  49, 2,  2,  2,  32, 2,  2,  43, 44, 39, 35, 45, 34, 2,  40, 2,  2,  2,  2,  2,  2,  2, 2, 2, 2,
    46, 51, 37, 36, 38, 50, 2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2, 2, 2, 2,
    2,  2,  2,  2,  47, 2,  48, 41, 2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2, 2, 2, 2,
    2,  2,  2,  2,  2,  2,  2,  2,  33, 2,  42, 2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2, 2, 2, 2,
    2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2, 2, 2, 2,
    2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2, 2, 2, 2,
    2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2, 2, 2, 2,
    2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  1,  2, 3, 4, 5,
    6,  7,  8,  9,  10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31};

#if YYDEBUG
/* YYRLINE[YYN] -- Source line where rule number YYN was defined.  */
static const yytype_int16 yyrline[] = {
    0,   88,  88,  89,  90,  91,  92,  93,  94,  95,  99,  99,  103, 110, 111, 112, 113, 114, 115, 116,
    117, 118, 123, 124, 125, 129, 130, 134, 138, 139, 140, 141, 144, 145, 146, 147, 151, 152, 153, 154,
    155, 159, 160, 163, 164, 165, 169, 170, 171, 172, 177, 178, 179, 180, 184, 185, 189, 190, 191, 192,
    197, 198, 203, 204, 205, 206, 210, 211, 212, 213, 214, 215, 216, 217, 218, 219, 220, 221, 222, 223,
    224, 225, 226, 227, 228, 229, 230, 231, 232, 233, 234, 238, 239, 241, 246, 247, 252, 253, 258, 259};
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
                                      "VPTT_dataequals",
                                      "VPTT_with_lookup",
                                      "VPTT_map",
                                      "VPTT_equiv",
                                      "VPTT_groupstar",
                                      "VPTT_and",
                                      "VPTT_macro",
                                      "VPTT_end_of_macro",
                                      "VPTT_or",
                                      "VPTT_not",
                                      "VPTT_hold_backward",
                                      "VPTT_look_forward",
                                      "VPTT_except",
                                      "VPTT_na",
                                      "VPTT_interpolate",
                                      "VPTT_raw",
                                      "VPTT_test_input",
                                      "VPTT_the_condition",
                                      "VPTT_implies",
                                      "VPTT_ge",
                                      "VPTT_le",
                                      "VPTT_ne",
                                      "VPTT_tabbed_array",
                                      "VPTT_eqend",
                                      "VPTT_number",
                                      "VPTT_literal",
                                      "VPTT_symbol",
                                      "VPTT_units_symbol",
                                      "VPTT_function",
                                      "'%'",
                                      "'|'",
                                      "'-'",
                                      "'+'",
                                      "'='",
                                      "'<'",
                                      "'>'",
                                      "'*'",
                                      "'/'",
                                      "'^'",
                                      "'~'",
                                      "'('",
                                      "')'",
                                      "','",
                                      "':'",
                                      "'['",
                                      "']'",
                                      "'!'",
                                      "'?'",
                                      "';'",
                                      "$accept",
                                      "fulleq",
                                      "macrostart",
                                      "$@1",
                                      "macroend",
                                      "eqn",
                                      "lhs",
                                      "var",
                                      "sublist",
                                      "symlist",
                                      "subdef",
                                      "unitsrange",
                                      "urangenum",
                                      "number",
                                      "units",
                                      "interpmode",
                                      "exceptlist",
                                      "mapsymlist",
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

#define YYPACT_NINF (-160)

#define yypact_value_is_default(Yyn) ((Yyn) == YYPACT_NINF)

#define YYTABLE_NINF (-1)

#define yytable_value_is_error(Yyn) 0

/* YYPACT[STATE-NUM] -- Index in YYTABLE of the portion describing
   STATE-NUM.  */
static const yytype_int16 yypact[] = {
    13,   -160, -160, -160, -160, -3,   10,   -160, -160, -6,   2,    203,  -5,   -2,   -22,  26,   -160, -160,
    101,  250,  25,   69,   -160, -160, 20,   -160, -160, -160, 52,   27,   -160, -160, 89,   1,    71,   90,
    -160, -160, -160, -17,  -16,  -25,  61,   250,  -160, -160, -160, 20,   86,   250,  250,  250,  136,  170,
    141,  -160, -36,  170,  -160, 162,  169,  37,   154,  -160, 159,  168,  153,  178,  -160, 20,   250,  190,
    -15,  64,   -160, -160, 196,  -160, 75,   -160, 222,  -160, -160, -160, -17,  -17,  -16,  214,  210,  191,
    191,  128,  250,  250,  250,  250,  250,  250,  250,  250,  250,  250,  250,  250,  250,  250,  250,  250,
    250,  -160, -160, 223,  37,   -160, -160, 37,   193,  -160, 109,  227,  -160, 240,  225,  -160, 242,  226,
    -160, -16,  -160, -160, 231,  -160, 129,  -160, 131,  117,  211,  22,   22,   22,   191,  191,  22,   22,
    22,   214,  214,  214,  87,   170,  170,  37,   235,  -160, 37,   -160, 228,  236,  70,   249,  -160, 92,
    -16,  -160, 230,  -160, 243,  244,  37,   245,  -160, 26,   -160, 258,  260,  -16,  -160, 138,  -160, 59,
    -160, 247,  37,   97,   246,  251,  248,  -16,  -160, 254,  255,  264,  256,  -160, 26,   -160, -160, 253,
    37,   259,  261,  -160, 126,  -160, 257,  -160, 37,   -160, 37,   262,  265,  37,   271,  266,  263,  143,
    37,   268,  267,  269,  195,  31,   37,   270,  153,  178,  272,  268,  215,  178,  273,  268};

/* YYDEFACT[STATE-NUM] -- Default reduction number in state STATE-NUM.
   Performed when YYTABLE does not specify something else to do.  Zero
   means the default is an error.  */
static const yytype_int8 yydefact[] = {
    0,  3,  10, 12, 2,  25, 0,  4,  5,  0,  18, 22, 0,  0,  0,  0,  26, 1,  0,  0,  0,  0,  52, 53, 0,  50, 51, 24, 23,
    0,  20, 32, 0,  60, 28, 0,  46, 9,  8,  0,  0,  0,  36, 0,  67, 66, 69, 25, 0,  0,  0,  0,  68, 17, 0,  21, 13, 62,
    43, 0,  0,  0,  0,  96, 0,  0,  94, 91, 54, 0,  0,  0,  0,  0,  19, 29, 0,  27, 0,  42, 0,  41, 7,  6,  0,  0,  0,
    86, 0,  88, 89, 0,  0,  0,  0,  0,  0,  0,  0,  0,  0,  0,  0,  0,  0,  0,  0,  0,  65, 44, 45, 0,  0,  14, 15, 0,
    0,  55, 0,  0,  56, 0,  61, 34, 0,  30, 49, 0,  48, 47, 0,  74, 0,  71, 0,  85, 84, 82, 80, 83, 76, 75, 87, 79, 81,
    77, 78, 90, 0,  63, 64, 0,  0,  97, 0,  11, 0,  0,  0,  0,  31, 0,  0,  72, 0,  70, 0,  0,  0,  0,  33, 0,  58, 0,
    0,  0,  39, 0,  73, 0,  98, 0,  0,  0,  0,  0,  0,  0,  37, 0,  0,  0,  0,  57, 0,  35, 40, 0,  0,  0,  0,  99, 0,
    38, 0,  16, 0,  59, 0,  0,  0,  0,  0,  0,  0,  0,  0,  0,  0,  0,  0,  0,  0,  0,  95, 92, 0,  0,  0,  93, 0,  0};

/* YYPGOTO[NTERM-NUM].  */
static const yytype_int16 yypgoto[] = {-160, -160, -160, -160, -160, -160, -160, 294, -20, -159, -160, -160, -70,
                                       -21,  -37,  -160, -160, -160, -160, -19,  -18, 134, -160, 96,   -72};

/* YYDEFGOTO[NTERM-NUM].  */
static const yytype_int8 yydefgoto[] = {0,  6,  7,  12, 8,   9,  10, 52, 16, 35, 33, 41, 80,
                                        81, 42, 27, 28, 122, 74, 56, 57, 64, 65, 66, 67};

/* YYTABLE[YYPACT[STATE-NUM]] -- What to do in state STATE-NUM.  If
   positive, shift that token.  If negative, reduce the rule whose
   number is the opposite.  If YYTABLE_NINF, syntax error.  */
static const yytype_uint8 yytable[] = {
    63,  53,  78,  13,  68,  19,  72,  31,  82,  107, 17,  58,  183, 36,  120, 108, 130, 83,  59,  60,  1,   32,  2,
    3,   29,  87,  39,  30,  121, 54,  93,  89,  90,  91,  79,  202, 18,  43,  20,  4,   111, 44,  5,   14,  15,  21,
    73,  128, 129, 117, 55,  118, 45,  46,  47,  34,  48,  161, 58,  49,  50,  103, 104, 105, 58,  59,  60,  15,  51,
    132, 70,  59,  60,  134, 61,  135, 136, 137, 138, 139, 140, 141, 142, 143, 144, 145, 146, 147, 148, 149, 150, 152,
    177, 123, 153, 93,  58,  69,  94,  172, 84,  85,  61,  59,  60,  186, 189, 124, 86,  95,  96,  97,  61,  173, 84,
    85,  62,  197, 71,  126, 75,  98,  99,  100, 101, 102, 103, 104, 105, 88,  167, 36,  166, 169, 37,  76,  93,  175,
    77,  94,  176, 193, 76,  38,  39,  220, 149, 181, 40,  225, 95,  96,  97,  155, 107, 229, 103, 104, 105, 225, 108,
    192, 98,  99,  100, 101, 102, 103, 104, 105, 207, 76,  133, 163, 164, 165, 107, 204, 93,  92,  108, 94,  108, 187,
    106, 209, 188, 210, 217, 109, 213, 218, 95,  96,  97,  219, 110, 112, 115, 93,  63,  226, 94,  113, 98,  99,  100,
    101, 102, 103, 104, 105, 114, 95,  96,  97,  22,  23,  24,  93,  25,  26,  43,  116, 119, 125, 44,  100, 101, 102,
    103, 104, 105, 95,  96,  97,  154, 45,  46,  47,  116, 48,  43,  223, 49,  50,  44,  100, 101, 102, 103, 104, 105,
    51,  131, 105, 156, 45,  46,  47,  217, 48,  43,  230, 49,  50,  44,  127, 151, 157, 158, 159, 170, 51,  178, 160,
    162, 45,  46,  47,  168, 48,  171, 174, 49,  50,  179, 184, 180, 185, 182, 191, 194, 51,  11,  195, 196, 198, 200,
    199, 201, 203, 208, 205, 206, 214, 216, 211, 0,   212, 215, 61,  221, 190, 222, 227, 228, 224, 231};

static const yytype_int16 yycheck[] = {
    21,  19,  39,  6,   24, 3,  5,   29,  33,  45,  0,  27,  171, 30,  29,  51,  86,  42,  34,  35,  7,   43,  9,
    10,  29,  43,  43,  29, 43, 4,   8,   49,  50,  51, 50,  194, 42,  12,  36,  26,  61,  16,  29,  46,  47,  43,
    45,  84,  85,  69,  25, 70, 27,  28,  29,  29,  31, 127, 27,  34,  35,  39,  40,  41,  27,  34,  35,  47,  43,
    88,  43,  34,  35,  92, 43, 93,  94,  95,  96,  97, 98,  99,  100, 101, 102, 103, 104, 105, 106, 107, 108, 112,
    162, 29,  115, 8,   27, 45, 11,  29,  39,  40,  43, 34,  35,  175, 47,  43,  47,  22,  23,  24,  43,  43,  39,
    40,  47,  187, 29,  44, 49, 34,  35,  36,  37,  38, 39,  40,  41,  43,  151, 30,  45,  154, 33,  45,  8,   45,
    48,  11,  48,  44,  45, 42, 43,  217, 164, 168, 47, 221, 22,  23,  24,  44,  45,  227, 39,  40,  41,  231, 51,
    182, 34,  35,  36,  37, 38, 39,  40,  41,  44,  45, 44,  44,  45,  44,  45,  198, 8,   43,  51,  11,  51,  45,
    43,  206, 48,  208, 45, 27, 211, 48,  22,  23,  24, 216, 27,  43,  45,  8,   221, 222, 11,  44,  34,  35,  36,
    37,  38,  39,  40,  41, 44, 22,  23,  24,  13,  14, 15,  8,   17,  18,  12,  45,  34,  29,  16,  36,  37,  38,
    39,  40,  41,  22,  23, 24, 43,  27,  28,  29,  45, 31,  12,  48,  34,  35,  16,  36,  37,  38,  39,  40,  41,
    43,  44,  41,  29,  27, 28, 29,  45,  31,  12,  48, 34,  35,  16,  45,  45,  29,  45,  29,  44,  43,  44,  49,
    45,  27,  28,  29,  45, 31, 46,  34,  34,  35,  43, 29,  44,  29,  45,  44,  46,  43,  0,   44,  48,  43,  34,
    44,  44,  48,  45,  44, 43, 34,  43,  45,  -1,  44, 44,  43,  45,  179, 45,  45,  44,  221, 45};

/* YYSTOS[STATE-NUM] -- The symbol kind of the accessing symbol of
   state STATE-NUM.  */
static const yytype_int8 yystos[] = {
    0,  7,  9,  10, 26, 29, 53, 54, 56, 57, 58, 59, 55, 6,  46, 47, 60, 0,  42, 3,  36, 43, 13, 14, 15, 17, 18, 67, 68,
    29, 29, 29, 43, 62, 29, 61, 30, 33, 42, 43, 47, 63, 66, 12, 16, 27, 28, 29, 31, 34, 35, 43, 59, 72, 4,  25, 71, 72,
    27, 34, 35, 43, 47, 65, 73, 74, 75, 76, 60, 45, 43, 29, 5,  45, 70, 49, 45, 48, 66, 50, 64, 65, 33, 42, 39, 40, 47,
    72, 43, 72, 72, 72, 43, 8,  11, 22, 23, 24, 34, 35, 36, 37, 38, 39, 40, 41, 43, 45, 51, 27, 27, 65, 43, 44, 44, 45,
    45, 60, 71, 34, 29, 43, 69, 29, 43, 29, 44, 45, 66, 66, 64, 44, 71, 44, 71, 72, 72, 72, 72, 72, 72, 72, 72, 72, 72,
    72, 72, 72, 72, 72, 72, 45, 65, 65, 43, 44, 29, 29, 45, 29, 49, 64, 45, 44, 45, 44, 45, 65, 45, 65, 44, 46, 29, 43,
    34, 45, 48, 64, 44, 43, 44, 65, 45, 61, 29, 29, 64, 45, 48, 47, 73, 44, 65, 44, 46, 44, 48, 64, 43, 44, 34, 44, 61,
    48, 65, 44, 43, 44, 45, 65, 65, 45, 44, 65, 34, 44, 43, 45, 48, 65, 76, 45, 45, 48, 75, 76, 65, 45, 44, 76, 48, 45};

/* YYR1[RULE-NUM] -- Symbol kind of the left-hand side of rule RULE-NUM.  */
static const yytype_int8 yyr1[] = {0,  52, 53, 53, 53, 53, 53, 53, 53, 53, 55, 54, 56, 57, 57, 57, 57, 57, 57, 57,
                                   57, 57, 58, 58, 58, 59, 59, 60, 61, 61, 61, 61, 62, 62, 62, 62, 63, 63, 63, 63,
                                   63, 64, 64, 65, 65, 65, 66, 66, 66, 66, 67, 67, 67, 67, 68, 68, 69, 69, 69, 69,
                                   70, 70, 71, 71, 71, 71, 72, 72, 72, 72, 72, 72, 72, 72, 72, 72, 72, 72, 72, 72,
                                   72, 72, 72, 72, 72, 72, 72, 72, 72, 72, 72, 73, 73, 73, 74, 74, 75, 75, 76, 76};

/* YYR2[RULE-NUM] -- Number of symbols on the right-hand side of rule RULE-NUM.  */
static const yytype_int8 yyr2[] = {0, 2, 1, 1, 1, 1, 4, 4, 3, 3, 0, 6, 1, 3, 4, 4, 10, 3,  1,  4, 3,  3, 1, 2, 2,
                                   1, 2, 3, 1, 2, 3, 4, 1, 5, 3, 7, 1, 6, 8, 5, 7, 1,  1,  1,  2, 2,  1, 3, 3, 3,
                                   1, 1, 1, 1, 2, 3, 1, 5, 3, 7, 0, 2, 1, 3, 3, 2, 1,  1,  1,  1, 4,  3, 4, 5, 3,
                                   3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 2, 3, 2, 2, 3, 1,  15, 17, 1, 15, 1, 3, 5, 7};

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
  case 2: /* fulleq: VPTT_eqend  */
#line 88 "VYacc.y"
  {
    return VPTT_eqend;
  }
#line 1314 "VYacc.tab.cpp"
  break;

  case 3: /* fulleq: VPTT_groupstar  */
#line 89 "VYacc.y"
  {
    return VPTT_groupstar;
  }
#line 1320 "VYacc.tab.cpp"
  break;

  case 4: /* fulleq: macrostart  */
#line 90 "VYacc.y"
  {
    return '|';
  }
#line 1326 "VYacc.tab.cpp"
  break;

  case 5: /* fulleq: macroend  */
#line 91 "VYacc.y"
  {
    return '|';
  }
#line 1332 "VYacc.tab.cpp"
  break;

  case 6: /* fulleq: eqn '~' unitsrange '~'  */
#line 92 "VYacc.y"
  {
    vpyy_addfulleq((yyvsp[-3].eqn), (yyvsp[-1].uni));
    return '~';
  }
#line 1338 "VYacc.tab.cpp"
  break;

  case 7: /* fulleq: eqn '~' unitsrange '|'  */
#line 93 "VYacc.y"
  {
    vpyy_addfulleq((yyvsp[-3].eqn), (yyvsp[-1].uni));
    return '|';
  }
#line 1344 "VYacc.tab.cpp"
  break;

  case 8: /* fulleq: eqn '~' '~'  */
#line 94 "VYacc.y"
  {
    vpyy_addfulleq((yyvsp[-2].eqn), NULL);
    return '~';
  }
#line 1350 "VYacc.tab.cpp"
  break;

  case 9: /* fulleq: eqn '~' '|'  */
#line 95 "VYacc.y"
  {
    vpyy_addfulleq((yyvsp[-2].eqn), NULL);
    return '|';
  }
#line 1356 "VYacc.tab.cpp"
  break;

  case 10: /* $@1: %empty  */
#line 99 "VYacc.y"
  {
    vpyy_macro_start();
  }
#line 1362 "VYacc.tab.cpp"
  break;

  case 11: /* macrostart: VPTT_macro $@1 VPTT_symbol '(' exprlist ')'  */
#line 99 "VYacc.y"
  {
    vpyy_macro_expression((yyvsp[-3].sym), (yyvsp[-1].exl));
  }
#line 1368 "VYacc.tab.cpp"
  break;

  case 12: /* macroend: VPTT_end_of_macro  */
#line 103 "VYacc.y"
  {
    (yyval.tok) = (yyvsp[0].tok);
    vpyy_macro_end();
  }
#line 1374 "VYacc.tab.cpp"
  break;

  case 13: /* eqn: lhs '=' exprlist  */
#line 110 "VYacc.y"
  {
    (yyval.eqn) = vpyy_addeq((yyvsp[-2].lhs), NULL, (yyvsp[0].exl), '=');
  }
#line 1380 "VYacc.tab.cpp"
  break;

  case 14: /* eqn: lhs '(' tablevals ')'  */
#line 111 "VYacc.y"
  {
    (yyval.eqn) = vpyy_add_lookup((yyvsp[-3].lhs), NULL, (yyvsp[-1].tbl), 0);
  }
#line 1386 "VYacc.tab.cpp"
  break;

  case 15: /* eqn: lhs '(' xytablevals ')'  */
#line 112 "VYacc.y"
  {
    (yyval.eqn) = vpyy_add_lookup((yyvsp[-3].lhs), NULL, (yyvsp[-1].tbl), 1);
  }
#line 1392 "VYacc.tab.cpp"
  break;

  case 16: /* eqn: lhs '=' VPTT_with_lookup '(' exp ',' '(' tablevals ')' ')'  */
#line 113 "VYacc.y"
  {
    (yyval.eqn) = vpyy_add_lookup((yyvsp[-9].lhs), (yyvsp[-5].exn), (yyvsp[-2].tbl), 0);
  }
#line 1398 "VYacc.tab.cpp"
  break;

  case 17: /* eqn: lhs VPTT_dataequals exp  */
#line 114 "VYacc.y"
  {
    (yyval.eqn) = vpyy_addeq((yyvsp[-2].lhs), (yyvsp[0].exn), NULL, VPTT_dataequals);
  }
#line 1404 "VYacc.tab.cpp"
  break;

  case 18: /* eqn: lhs  */
#line 115 "VYacc.y"
  {
    (yyval.eqn) = vpyy_add_lookup((yyvsp[0].lhs), NULL, NULL, 0);
  }
#line 1410 "VYacc.tab.cpp"
  break;

  case 19: /* eqn: VPTT_symbol ':' subdef maplist  */
#line 116 "VYacc.y"
  {
    (yyval.eqn) = vpyy_addeq(vpyy_addexceptinterp(vpyy_var_expression((yyvsp[-3].sym), NULL), NULL, 0),
                             (Expression *)vpyy_symlist_expression((yyvsp[-1].sml), (yyvsp[0].sml)), NULL, ':');
  }
#line 1416 "VYacc.tab.cpp"
  break;

  case 20: /* eqn: VPTT_symbol VPTT_equiv VPTT_symbol  */
#line 117 "VYacc.y"
  {
    (yyval.eqn) = vpyy_addeq(vpyy_addexceptinterp(vpyy_var_expression((yyvsp[-2].sym), NULL), NULL, 0),
                             (Expression *)vpyy_symlist_expression(vpyy_symlist(NULL, (yyvsp[0].sym), 0, NULL), NULL),
                             NULL, VPTT_equiv);
  }
#line 1422 "VYacc.tab.cpp"
  break;

  case 21: /* eqn: lhs '=' VPTT_tabbed_array  */
#line 118 "VYacc.y"
  {
    (yyval.eqn) = vpyy_addeq((yyvsp[-2].lhs), (yyvsp[0].exn), NULL, '=');
  }
#line 1428 "VYacc.tab.cpp"
  break;

  case 22: /* lhs: var  */
#line 123 "VYacc.y"
  {
    (yyval.lhs) = vpyy_addexceptinterp((yyvsp[0].var), NULL, 0);
  }
#line 1434 "VYacc.tab.cpp"
  break;

  case 23: /* lhs: var exceptlist  */
#line 124 "VYacc.y"
  {
    (yyval.lhs) = vpyy_addexceptinterp((yyvsp[-1].var), (yyvsp[0].sll), 0);
  }
#line 1440 "VYacc.tab.cpp"
  break;

  case 24: /* lhs: var interpmode  */
#line 125 "VYacc.y"
  {
    (yyval.lhs) = vpyy_addexceptinterp((yyvsp[-1].var), NULL, (yyvsp[0].tok));
  }
#line 1446 "VYacc.tab.cpp"
  break;

  case 25: /* var: VPTT_symbol  */
#line 129 "VYacc.y"
  {
    (yyval.var) = vpyy_var_expression((yyvsp[0].sym), NULL);
  }
#line 1452 "VYacc.tab.cpp"
  break;

  case 26: /* var: VPTT_symbol sublist  */
#line 130 "VYacc.y"
  {
    (yyval.var) = vpyy_var_expression((yyvsp[-1].sym), (yyvsp[0].sml));
  }
#line 1458 "VYacc.tab.cpp"
  break;

  case 27: /* sublist: '[' symlist ']'  */
#line 134 "VYacc.y"
  {
    (yyval.sml) = (yyvsp[-1].sml);
  }
#line 1464 "VYacc.tab.cpp"
  break;

  case 28: /* symlist: VPTT_symbol  */
#line 138 "VYacc.y"
  {
    (yyval.sml) = vpyy_symlist(NULL, (yyvsp[0].sym), 0, NULL);
  }
#line 1470 "VYacc.tab.cpp"
  break;

  case 29: /* symlist: VPTT_symbol '!'  */
#line 139 "VYacc.y"
  {
    (yyval.sml) = vpyy_symlist(NULL, (yyvsp[-1].sym), 1, NULL);
  }
#line 1476 "VYacc.tab.cpp"
  break;

  case 30: /* symlist: symlist ',' VPTT_symbol  */
#line 140 "VYacc.y"
  {
    (yyval.sml) = vpyy_symlist((yyvsp[-2].sml), (yyvsp[0].sym), 0, NULL);
  }
#line 1482 "VYacc.tab.cpp"
  break;

  case 31: /* symlist: symlist ',' VPTT_symbol '!'  */
#line 141 "VYacc.y"
  {
    (yyval.sml) = vpyy_symlist((yyvsp[-3].sml), (yyvsp[-1].sym), 1, NULL);
  }
#line 1488 "VYacc.tab.cpp"
  break;

  case 32: /* subdef: VPTT_symbol  */
#line 144 "VYacc.y"
  {
    (yyval.sml) = vpyy_symlist(NULL, (yyvsp[0].sym), 0, NULL);
  }
#line 1494 "VYacc.tab.cpp"
  break;

  case 33: /* subdef: '(' VPTT_symbol '-' VPTT_symbol ')'  */
#line 145 "VYacc.y"
  {
    (yyval.sml) = vpyy_symlist(NULL, (yyvsp[-3].sym), 0, (yyvsp[-1].sym));
  }
#line 1500 "VYacc.tab.cpp"
  break;

  case 34: /* subdef: subdef ',' VPTT_symbol  */
#line 146 "VYacc.y"
  {
    (yyval.sml) = vpyy_symlist((yyvsp[-2].sml), (yyvsp[0].sym), 0, NULL);
  }
#line 1506 "VYacc.tab.cpp"
  break;

  case 35: /* subdef: subdef ',' '(' VPTT_symbol '-' VPTT_symbol ')'  */
#line 147 "VYacc.y"
  {
    (yyval.sml) = vpyy_symlist((yyvsp[-6].sml), (yyvsp[-3].sym), 0, (yyvsp[-1].sym));
  }
#line 1512 "VYacc.tab.cpp"
  break;

  case 36: /* unitsrange: units  */
#line 151 "VYacc.y"
  {
    (yyval.uni) = (yyvsp[0].uni);
  }
#line 1518 "VYacc.tab.cpp"
  break;

  case 37: /* unitsrange: units '[' urangenum ',' urangenum ']'  */
#line 152 "VYacc.y"
  {
    (yyval.uni) = vpyy_unitsrange((yyvsp[-5].uni), (yyvsp[-3].num), (yyvsp[-1].num), -1);
  }
#line 1524 "VYacc.tab.cpp"
  break;

  case 38: /* unitsrange: units '[' urangenum ',' urangenum ',' urangenum ']'  */
#line 153 "VYacc.y"
  {
    (yyval.uni) = vpyy_unitsrange((yyvsp[-7].uni), (yyvsp[-5].num), (yyvsp[-3].num), (yyvsp[-1].num));
  }
#line 1530 "VYacc.tab.cpp"
  break;

  case 39: /* unitsrange: '[' urangenum ',' urangenum ']'  */
#line 154 "VYacc.y"
  {
    (yyval.uni) = vpyy_unitsrange(NULL, (yyvsp[-3].num), (yyvsp[-1].num), -1);
  }
#line 1536 "VYacc.tab.cpp"
  break;

  case 40: /* unitsrange: '[' urangenum ',' urangenum ',' urangenum ']'  */
#line 155 "VYacc.y"
  {
    (yyval.uni) = vpyy_unitsrange(NULL, (yyvsp[-5].num), (yyvsp[-3].num), (yyvsp[-1].num));
  }
#line 1542 "VYacc.tab.cpp"
  break;

  case 41: /* urangenum: number  */
#line 159 "VYacc.y"
  {
    (yyval.num) = (yyvsp[0].num);
  }
#line 1548 "VYacc.tab.cpp"
  break;

  case 42: /* urangenum: '?'  */
#line 160 "VYacc.y"
  {
    (yyval.num) = -1e30;
  }
#line 1554 "VYacc.tab.cpp"
  break;

  case 43: /* number: VPTT_number  */
#line 163 "VYacc.y"
  {
    (yyval.num) = (yyvsp[0].num);
  }
#line 1560 "VYacc.tab.cpp"
  break;

  case 44: /* number: '-' VPTT_number  */
#line 164 "VYacc.y"
  {
    (yyval.num) = -(yyvsp[0].num);
  }
#line 1566 "VYacc.tab.cpp"
  break;

  case 45: /* number: '+' VPTT_number  */
#line 165 "VYacc.y"
  {
    (yyval.num) = (yyvsp[0].num);
  }
#line 1572 "VYacc.tab.cpp"
  break;

  case 46: /* units: VPTT_units_symbol  */
#line 169 "VYacc.y"
  {
    (yyval.uni) = (yyvsp[0].uni);
  }
#line 1578 "VYacc.tab.cpp"
  break;

  case 47: /* units: units '/' units  */
#line 170 "VYacc.y"
  {
    (yyval.uni) = vpyy_unitsdiv((yyvsp[-2].uni), (yyvsp[0].uni));
  }
#line 1584 "VYacc.tab.cpp"
  break;

  case 48: /* units: units '*' units  */
#line 171 "VYacc.y"
  {
    (yyval.uni) = vpyy_unitsmult((yyvsp[-2].uni), (yyvsp[0].uni));
  }
#line 1590 "VYacc.tab.cpp"
  break;

  case 49: /* units: '(' units ')'  */
#line 172 "VYacc.y"
  {
    (yyval.uni) = (yyvsp[-1].uni);
  }
#line 1596 "VYacc.tab.cpp"
  break;

  case 50: /* interpmode: VPTT_interpolate  */
#line 177 "VYacc.y"
  {
    (yyval.tok) = (yyvsp[0].tok);
  }
#line 1602 "VYacc.tab.cpp"
  break;

  case 51: /* interpmode: VPTT_raw  */
#line 178 "VYacc.y"
  {
    (yyval.tok) = (yyvsp[0].tok);
  }
#line 1608 "VYacc.tab.cpp"
  break;

  case 52: /* interpmode: VPTT_hold_backward  */
#line 179 "VYacc.y"
  {
    (yyval.tok) = (yyvsp[0].tok);
  }
#line 1614 "VYacc.tab.cpp"
  break;

  case 53: /* interpmode: VPTT_look_forward  */
#line 180 "VYacc.y"
  {
    (yyval.tok) = (yyvsp[0].tok);
  }
#line 1620 "VYacc.tab.cpp"
  break;

  case 54: /* exceptlist: VPTT_except sublist  */
#line 184 "VYacc.y"
  {
    (yyval.sll) = vpyy_chain_sublist(NULL, (yyvsp[0].sml));
  }
#line 1626 "VYacc.tab.cpp"
  break;

  case 55: /* exceptlist: exceptlist ',' sublist  */
#line 185 "VYacc.y"
  {
    vpyy_chain_sublist((yyvsp[-2].sll), (yyvsp[0].sml));
    (yyval.sll) = (yyvsp[-2].sll);
  }
#line 1632 "VYacc.tab.cpp"
  break;

  case 56: /* mapsymlist: VPTT_symbol  */
#line 189 "VYacc.y"
  {
    (yyval.sml) = vpyy_symlist(NULL, (yyvsp[0].sym), 0, NULL);
  }
#line 1638 "VYacc.tab.cpp"
  break;

  case 57: /* mapsymlist: '(' VPTT_symbol ':' symlist ')'  */
#line 190 "VYacc.y"
  {
    (yyval.sml) = vpyy_mapsymlist(NULL, (yyvsp[-3].sym), (yyvsp[-1].sml));
  }
#line 1644 "VYacc.tab.cpp"
  break;

  case 58: /* mapsymlist: mapsymlist ',' VPTT_symbol  */
#line 191 "VYacc.y"
  {
    (yyval.sml) = vpyy_symlist((yyvsp[-2].sml), (yyvsp[0].sym), 0, NULL);
  }
#line 1650 "VYacc.tab.cpp"
  break;

  case 59: /* mapsymlist: mapsymlist ',' '(' VPTT_symbol ':' symlist ')'  */
#line 192 "VYacc.y"
  {
    (yyval.sml) = vpyy_mapsymlist((yyvsp[-6].sml), (yyvsp[-3].sym), (yyvsp[-1].sml));
  }
#line 1656 "VYacc.tab.cpp"
  break;

  case 60: /* maplist: %empty  */
#line 197 "VYacc.y"
  {
    (yyval.sml) = NULL;
  }
#line 1662 "VYacc.tab.cpp"
  break;

  case 61: /* maplist: VPTT_map mapsymlist  */
#line 198 "VYacc.y"
  {
    (yyval.sml) = (yyvsp[0].sml);
  }
#line 1668 "VYacc.tab.cpp"
  break;

  case 62: /* exprlist: exp  */
#line 203 "VYacc.y"
  {
    (yyval.exl) = vpyy_chain_exprlist(NULL, (yyvsp[0].exn));
  }
#line 1674 "VYacc.tab.cpp"
  break;

  case 63: /* exprlist: exprlist ',' exp  */
#line 204 "VYacc.y"
  {
    (yyval.exl) = vpyy_chain_exprlist((yyvsp[-2].exl), (yyvsp[0].exn));
  }
#line 1680 "VYacc.tab.cpp"
  break;

  case 64: /* exprlist: exprlist ';' exp  */
#line 205 "VYacc.y"
  {
    (yyval.exl) = vpyy_chain_exprlist((yyvsp[-2].exl), (yyvsp[0].exn));
  }
#line 1686 "VYacc.tab.cpp"
  break;

  case 65: /* exprlist: exprlist ';'  */
#line 206 "VYacc.y"
  {
    (yyval.exl) = (yyvsp[-1].exl);
  }
#line 1692 "VYacc.tab.cpp"
  break;

  case 66: /* exp: VPTT_number  */
#line 210 "VYacc.y"
  {
    (yyval.exn) = vpyy_num_expression((yyvsp[0].num));
  }
#line 1698 "VYacc.tab.cpp"
  break;

  case 67: /* exp: VPTT_na  */
#line 211 "VYacc.y"
  {
    (yyval.exn) = vpyy_num_expression(-1E38);
  }
#line 1704 "VYacc.tab.cpp"
  break;

  case 68: /* exp: var  */
#line 212 "VYacc.y"
  {
    (yyval.exn) = (Expression *)(yyvsp[0].var);
  }
#line 1710 "VYacc.tab.cpp"
  break;

  case 69: /* exp: VPTT_literal  */
#line 213 "VYacc.y"
  {
    (yyval.exn) = vpyy_literal_expression((yyvsp[0].lit));
  }
#line 1716 "VYacc.tab.cpp"
  break;

  case 70: /* exp: var '(' exprlist ')'  */
#line 214 "VYacc.y"
  {
    (yyval.exn) = vpyy_lookup_expression((yyvsp[-3].var), (yyvsp[-1].exl));
  }
#line 1722 "VYacc.tab.cpp"
  break;

  case 71: /* exp: '(' exp ')'  */
#line 215 "VYacc.y"
  {
    (yyval.exn) = vpyy_operator_expression('(', (yyvsp[-1].exn), NULL);
  }
#line 1728 "VYacc.tab.cpp"
  break;

  case 72: /* exp: VPTT_function '(' exprlist ')'  */
#line 216 "VYacc.y"
  {
    (yyval.exn) = vpyy_function_expression((yyvsp[-3].fnc), (yyvsp[-1].exl));
  }
#line 1734 "VYacc.tab.cpp"
  break;

  case 73: /* exp: VPTT_function '(' exprlist ',' ')'  */
#line 217 "VYacc.y"
  {
    (yyval.exn) =
        vpyy_function_expression((yyvsp[-4].fnc), vpyy_chain_exprlist((yyvsp[-2].exl), vpyy_literal_expression("?")));
  }
#line 1740 "VYacc.tab.cpp"
  break;

  case 74: /* exp: VPTT_function '(' ')'  */
#line 218 "VYacc.y"
  {
    (yyval.exn) = vpyy_function_expression((yyvsp[-2].fnc), NULL);
  }
#line 1746 "VYacc.tab.cpp"
  break;

  case 75: /* exp: exp '+' exp  */
#line 219 "VYacc.y"
  {
    (yyval.exn) = vpyy_operator_expression('+', (yyvsp[-2].exn), (yyvsp[0].exn));
  }
#line 1752 "VYacc.tab.cpp"
  break;

  case 76: /* exp: exp '-' exp  */
#line 220 "VYacc.y"
  {
    (yyval.exn) = vpyy_operator_expression('-', (yyvsp[-2].exn), (yyvsp[0].exn));
  }
#line 1758 "VYacc.tab.cpp"
  break;

  case 77: /* exp: exp '*' exp  */
#line 221 "VYacc.y"
  {
    (yyval.exn) = vpyy_operator_expression('*', (yyvsp[-2].exn), (yyvsp[0].exn));
  }
#line 1764 "VYacc.tab.cpp"
  break;

  case 78: /* exp: exp '/' exp  */
#line 222 "VYacc.y"
  {
    (yyval.exn) = vpyy_operator_expression('/', (yyvsp[-2].exn), (yyvsp[0].exn));
  }
#line 1770 "VYacc.tab.cpp"
  break;

  case 79: /* exp: exp '<' exp  */
#line 223 "VYacc.y"
  {
    (yyval.exn) = vpyy_operator_expression('<', (yyvsp[-2].exn), (yyvsp[0].exn));
  }
#line 1776 "VYacc.tab.cpp"
  break;

  case 80: /* exp: exp VPTT_le exp  */
#line 224 "VYacc.y"
  {
    (yyval.exn) = vpyy_operator_expression(VPTT_le, (yyvsp[-2].exn), (yyvsp[0].exn));
  }
#line 1782 "VYacc.tab.cpp"
  break;

  case 81: /* exp: exp '>' exp  */
#line 225 "VYacc.y"
  {
    (yyval.exn) = vpyy_operator_expression('>', (yyvsp[-2].exn), (yyvsp[0].exn));
  }
#line 1788 "VYacc.tab.cpp"
  break;

  case 82: /* exp: exp VPTT_ge exp  */
#line 226 "VYacc.y"
  {
    (yyval.exn) = vpyy_operator_expression(VPTT_ge, (yyvsp[-2].exn), (yyvsp[0].exn));
  }
#line 1794 "VYacc.tab.cpp"
  break;

  case 83: /* exp: exp VPTT_ne exp  */
#line 227 "VYacc.y"
  {
    (yyval.exn) = vpyy_operator_expression(VPTT_ne, (yyvsp[-2].exn), (yyvsp[0].exn));
  }
#line 1800 "VYacc.tab.cpp"
  break;

  case 84: /* exp: exp VPTT_or exp  */
#line 228 "VYacc.y"
  {
    (yyval.exn) = vpyy_operator_expression(VPTT_or, (yyvsp[-2].exn), (yyvsp[0].exn));
  }
#line 1806 "VYacc.tab.cpp"
  break;

  case 85: /* exp: exp VPTT_and exp  */
#line 229 "VYacc.y"
  {
    (yyval.exn) = vpyy_operator_expression(VPTT_and, (yyvsp[-2].exn), (yyvsp[0].exn));
  }
#line 1812 "VYacc.tab.cpp"
  break;

  case 86: /* exp: VPTT_not exp  */
#line 230 "VYacc.y"
  {
    (yyval.exn) = vpyy_operator_expression(VPTT_not, (yyvsp[0].exn), NULL);
  }
#line 1818 "VYacc.tab.cpp"
  break;

  case 87: /* exp: exp '=' exp  */
#line 231 "VYacc.y"
  {
    (yyval.exn) = vpyy_operator_expression('=', (yyvsp[-2].exn), (yyvsp[0].exn));
  }
#line 1824 "VYacc.tab.cpp"
  break;

  case 88: /* exp: '-' exp  */
#line 232 "VYacc.y"
  {
    (yyval.exn) = vpyy_operator_expression('-', NULL, (yyvsp[0].exn));
  }
#line 1830 "VYacc.tab.cpp"
  break;

  case 89: /* exp: '+' exp  */
#line 233 "VYacc.y"
  {
    (yyval.exn) = vpyy_operator_expression('+', NULL, (yyvsp[0].exn));
  }
#line 1836 "VYacc.tab.cpp"
  break;

  case 90: /* exp: exp '^' exp  */
#line 234 "VYacc.y"
  {
    (yyval.exn) = vpyy_operator_expression('^', (yyvsp[-2].exn), (yyvsp[0].exn));
  }
#line 1842 "VYacc.tab.cpp"
  break;

  case 91: /* tablevals: tablepairs  */
#line 238 "VYacc.y"
  {
    (yyval.tbl) = (yyvsp[0].tbl);
  }
#line 1848 "VYacc.tab.cpp"
  break;

  case 92: /* tablevals: '[' '(' number ',' number ')' '-' '(' number ',' number ')' ']' ',' tablepairs  */
#line 240 "VYacc.y"
  {
    (yyval.tbl) = vpyy_tablerange((yyvsp[0].tbl), (yyvsp[-12].num), (yyvsp[-10].num), (yyvsp[-6].num), (yyvsp[-4].num));
  }
#line 1854 "VYacc.tab.cpp"
  break;

  case 93: /* tablevals: '[' '(' number ',' number ')' '-' '(' number ',' number ')' ',' tablepairs ']' ',' tablepairs
            */
#line 242 "VYacc.y"
  {
    (yyval.tbl) = vpyy_tablerange((yyvsp[0].tbl), (yyvsp[-14].num), (yyvsp[-12].num), (yyvsp[-8].num), (yyvsp[-6].num));
  }
#line 1860 "VYacc.tab.cpp"
  break;

  case 94: /* xytablevals: xytablevec  */
#line 246 "VYacc.y"
  {
    (yyval.tbl) = (yyvsp[0].tbl);
  }
#line 1866 "VYacc.tab.cpp"
  break;

  case 95: /* xytablevals: '[' '(' number ',' number ')' '-' '(' number ',' number ')' ']' ',' xytablevec  */
#line 248 "VYacc.y"
  {
    (yyval.tbl) = vpyy_tablerange((yyvsp[0].tbl), (yyvsp[-12].num), (yyvsp[-10].num), (yyvsp[-6].num), (yyvsp[-4].num));
  }
#line 1872 "VYacc.tab.cpp"
  break;

  case 96: /* xytablevec: number  */
#line 252 "VYacc.y"
  {
    (yyval.tbl) = vpyy_tablevec(NULL, (yyvsp[0].num));
  }
#line 1878 "VYacc.tab.cpp"
  break;

  case 97: /* xytablevec: xytablevec ',' number  */
#line 253 "VYacc.y"
  {
    (yyval.tbl) = vpyy_tablevec((yyvsp[-2].tbl), (yyvsp[0].num));
  }
#line 1884 "VYacc.tab.cpp"
  break;

  case 98: /* tablepairs: '(' number ',' number ')'  */
#line 258 "VYacc.y"
  {
    (yyval.tbl) = vpyy_tablepair(NULL, (yyvsp[-3].num), (yyvsp[-1].num));
  }
#line 1890 "VYacc.tab.cpp"
  break;

  case 99: /* tablepairs: tablepairs ',' '(' number ',' number ')'  */
#line 259 "VYacc.y"
  {
    (yyval.tbl) = vpyy_tablepair((yyvsp[-6].tbl), (yyvsp[-3].num), (yyvsp[-1].num));
  }
#line 1896 "VYacc.tab.cpp"
  break;

#line 1900 "VYacc.tab.cpp"

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

#line 265 "VYacc.y"
