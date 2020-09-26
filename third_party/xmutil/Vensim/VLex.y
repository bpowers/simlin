/* scanner for a toy Pascal-like language */
  
%{
/* need this for the call to atof() below */
#include <math.h>
#include <stdio.h>      /* printf, fgets */
#include <stdlib.h>     /* atoi */
#include <malloc.h>
#define YY_NO_UNISTD_H
%}
%option never-interactive
%option noyywrap


%%

[0-9]+    {
            printf( "An integer: %s (%d)\n", yytext,
                    atoi( yytext ) );
            }

["][^"]*["] {
    printf("A quoted varname no embedded quotes - %s\n",yytext) ;

   

/* ["][^"\\]*(?:\\.[^"\\]*)*["] {
         printf("A quoted variable name - %s\n",yytext) ;
     } */
     }

"~"       {
           return 1 ;
       }

"\\\\\\---///" {
          return 0 ;
      }

[0-9]+"."[0-9]*        {
            printf( "A float: %s (%g)\n", yytext,
                    atof( yytext ) );
            }
[a-zA-Z][a-zA-Z0-9' ']* {
            printf("A variable name: %s\n",yytext) ;
      }


"+"|"-"|"*"|"/"   printf( "An operator: %s\n", yytext );

"{"[\^{}}\n]*"}"     /* eat up one-line comments */

[ \t\n]+          /* eat up whitespace */

.           printf( "Unrecognized character: %s\n", yytext );

%%