/* A Bison parser, made by GNU Bison 3.8.2.  */

/* Bison interface for Yacc-like parsers in C

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

/* DO NOT RELY ON FEATURES THAT ARE NOT DOCUMENTED in the manual,
   especially those whose name start with YY_ or yy_.  They are
   private implementation details that can be changed or removed.  */

#ifndef YY_DPYY_DYACC_TAB_HPP_INCLUDED
# define YY_DPYY_DYACC_TAB_HPP_INCLUDED
/* Debug traces.  */
#ifndef YYDEBUG
# define YYDEBUG 0
#endif
#if YYDEBUG
extern int dpyydebug;
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
    DPTT_aux = 258,                /* DPTT_aux  */
    DPTT_table = 259,              /* DPTT_table  */
    DPTT_level = 260,              /* DPTT_level  */
    DPTT_init = 261,               /* DPTT_init  */
    DPTT_constant = 262,           /* DPTT_constant  */
    DPTT_eoq = 263,                /* DPTT_eoq  */
    DPTT_groupstar = 264,          /* DPTT_groupstar  */
    DPTT_specs = 265,              /* DPTT_specs  */
    DPTT_save = 266,               /* DPTT_save  */
    DPTT_and = 267,                /* DPTT_and  */
    DPTT_macro = 268,              /* DPTT_macro  */
    DPTT_end_of_macro = 269,       /* DPTT_end_of_macro  */
    DPTT_or = 270,                 /* DPTT_or  */
    DPTT_not = 271,                /* DPTT_not  */
    DPTT_ge = 272,                 /* DPTT_ge  */
    DPTT_le = 273,                 /* DPTT_le  */
    DPTT_ne = 274,                 /* DPTT_ne  */
    DPTT_number = 275,             /* DPTT_number  */
    DPTT_symbol = 276,             /* DPTT_symbol  */
    DPTT_units_symbol = 277,       /* DPTT_units_symbol  */
    DPTT_function = 278,           /* DPTT_function  */
    DPTT_dt_to_one = 279           /* DPTT_dt_to_one  */
  };
  typedef enum yytokentype yytoken_kind_t;
#endif

/* Value type.  */


extern YYSTYPE dpyylval;


int dpyyparse (void);


#endif /* !YY_DPYY_DYACC_TAB_HPP_INCLUDED  */
