/* A Bison parser, made by GNU Bison 3.7.4.  */

/* Bison interface for Yacc-like parsers in C

   Copyright (C) 1984, 1989-1990, 2000-2015, 2018-2020 Free Software Foundation,
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
   along with this program.  If not, see <http://www.gnu.org/licenses/>.  */

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

/* DO NOT RELY ON FEATURES THAT ARE NOT DOCUMENTED in the manual,
   especially those whose name start with YY_ or yy_.  They are
   private implementation details that can be changed or removed.  */

#ifndef YY_VPYY_VYACC_TAB_HPP_INCLUDED
# define YY_VPYY_VYACC_TAB_HPP_INCLUDED
/* Debug traces.  */
#ifndef YYDEBUG
# define YYDEBUG 0
#endif
#if YYDEBUG
extern int vpyydebug;
#endif

/* Token kinds.  */
#ifndef YYTOKENTYPE
# define YYTOKENTYPE
  enum yytokentype
  {
    YYEMPTY = -2,
    YYEOF = 0,                     /* "end of file"  */
    YYerror = 256,                 /* error  */
    YYUNDEF = 257,                 /* "invalid token"  */
    VPTT_dataequals = 258,         /* VPTT_dataequals  */
    VPTT_with_lookup = 259,        /* VPTT_with_lookup  */
    VPTT_map = 260,                /* VPTT_map  */
    VPTT_equiv = 261,              /* VPTT_equiv  */
    VPTT_groupstar = 262,          /* VPTT_groupstar  */
    VPTT_and = 263,                /* VPTT_and  */
    VPTT_macro = 264,              /* VPTT_macro  */
    VPTT_end_of_macro = 265,       /* VPTT_end_of_macro  */
    VPTT_or = 266,                 /* VPTT_or  */
    VPTT_not = 267,                /* VPTT_not  */
    VPTT_hold_backward = 268,      /* VPTT_hold_backward  */
    VPTT_look_forward = 269,       /* VPTT_look_forward  */
    VPTT_except = 270,             /* VPTT_except  */
    VPTT_na = 271,                 /* VPTT_na  */
    VPTT_interpolate = 272,        /* VPTT_interpolate  */
    VPTT_raw = 273,                /* VPTT_raw  */
    VPTT_test_input = 274,         /* VPTT_test_input  */
    VPTT_the_condition = 275,      /* VPTT_the_condition  */
    VPTT_implies = 276,            /* VPTT_implies  */
    VPTT_ge = 277,                 /* VPTT_ge  */
    VPTT_le = 278,                 /* VPTT_le  */
    VPTT_ne = 279,                 /* VPTT_ne  */
    VPTT_tabbed_array = 280,       /* VPTT_tabbed_array  */
    VPTT_eqend = 281,              /* VPTT_eqend  */
    VPTT_number = 282,             /* VPTT_number  */
    VPTT_literal = 283,            /* VPTT_literal  */
    VPTT_symbol = 284,             /* VPTT_symbol  */
    VPTT_units_symbol = 285,       /* VPTT_units_symbol  */
    VPTT_function = 286            /* VPTT_function  */
  };
  typedef enum yytokentype yytoken_kind_t;
#endif

/* Value type.  */


extern YYSTYPE vpyylval;

int vpyyparse (void);

#endif /* !YY_VPYY_VYACC_TAB_HPP_INCLUDED  */
