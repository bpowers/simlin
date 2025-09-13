/* 
c:\tools\bison\bin\win_bison -o DYacc.tab.cpp -p dpyy -d DYacc.y


c:\tools\bison\bin\win_bison -o $(ProjectDir)src\Dynamo\DYacc.tab.cpp -p dpyy -d $(ProjectDir)src\Dynamo\DYacc.y
Converting DYacc.y
Outputs: DYacc.tab.cpp DYacc.tab.hpp 
*/

%{
#include "../Log.h"
#include "../Symbol/Parse.h"
#include "DynamoParseFunctions.h"
extern int dpyylex (void);
extern void dpyyerror (char const *);
#define YYSTYPE ParseUnion
#define YYFPRINTF fprintf
%}
     
/* tokens returned by the tokenizer (in addition to single char tokens) */

/* equation definition tokens */
%token <tol> DPTT_aux
%token <tok> DPTT_table
%token <tok> DPTT_level
%token <tok> DPTT_init
%token <tok> DPTT_constant
/* end of equation depends on the source type - in traditionally dynamo it is any white space */
%token <tok> DPTT_eoq
/* this is actually a * but context dependent */
%token <tok> DPTT_groupstar

%token <tok> DPTT_specs
%token <tok> DPTT_save

%token <tok> DPTT_and
%token <tok> DPTT_macro
%token <tok> DPTT_end_of_macro
%token <tok> DPTT_or
%token <tok> DPTT_not
%token <tok> DPTT_ge
%token <tok> DPTT_le
%token <tok> DPTT_ne


/* tokens with types returned by the tokenizer
   for these the tokenizer needs to set yylval.var
   and so on before returning the token ID  */
%token <num> DPTT_number
%token <sym> DPTT_symbol
%token <uni> DPTT_units_symbol
%token <fnc> DPTT_function
%token <tok> DPTT_dt_to_one // special for level equations


/* types for the different productions - note that there are quite  a few
   of these and that reductions in addition to creating or reusing an object
   also delete the component object to cleaning up is done as the 
   parsing progresses */
%type <exn> exp 
%type <tbl> tablepairs tablevals xytablevals xytablevec tabledef
%type <exl> exprlist
%type <lhs> lhs
%type <eqn> eqn
%type <eqn> stock_eqn
%type <eqn> teqn
%type <fqn> fulleq
%type <var> var
%type <sml> symlist sublist subdef maplist
%type <num> number


/* precedence - low to high */     
%left '-' '+'
%left DPTT_or
%left '=' '<' '>' DPTT_le DPTT_ge DPTT_ne
%left DPTT_and
%left '*' '/'
%left DPTT_not
%right '^'      /* exponentiation */

%% /* The grammar follows.  */


fulleq :
	DPTT_eoq { return DPTT_eoq;} // end of file
    | DPTT_groupstar { return DPTT_groupstar; } // depth and name follow - manage that outside
	| DPTT_specs { return DPTT_specs; }
	| DPTT_save { return DPTT_save; }
	| DPTT_table teqn DPTT_eoq { dpyy_addfulleq($2,DPTT_table) ; return DPTT_eoq;}
	| DPTT_constant eqn DPTT_eoq { dpyy_addfulleq($2,DPTT_constant) ; return DPTT_eoq; }
	| DPTT_init eqn DPTT_eoq { dpyy_addfulleq($2,DPTT_init) ; return DPTT_eoq;} 
	| DPTT_level stock_eqn DPTT_eoq { dpyy_addfulleq($2,DPTT_level) ; return DPTT_eoq;}
	| DPTT_aux eqn DPTT_eoq { dpyy_addfulleq($2,DPTT_aux) ; return DPTT_eoq;}
	;

teqn : 
   lhs '=' tabledef {$$ = dpyy_add_lookup($1,NULL,$3,0) ; }
   ;


tabledef :
    number { $$ = dpyy_tablevec(NULL,$1) ;}
	| tabledef ',' number  { $$ = dpyy_tablevec($1,$3) ;}
	| tabledef '/' number  { $$ = dpyy_tablevec($1,$3) ;}
	;




eqn : 
   lhs '=' exprlist {$$ = dpyy_addeq($1,NULL,$3,'=') ; }
   | lhs '(' tablevals ')' { $$ = dpyy_add_lookup($1,NULL,$3, 0) ; }
   | lhs '(' xytablevals ')' { $$ = dpyy_add_lookup($1,NULL,$3, 1) ; }
   | lhs { $$ = dpyy_add_lookup($1,NULL,NULL, 0) ; } // treat as if a lookup on time - don't have numbers
   | DPTT_symbol ':' subdef maplist {$$ = dpyy_addeq(dpyy_addexceptinterp(dpyy_var_expression($1,NULL),NULL,0),(Expression *)dpyy_symlist_expression($3,$4),NULL,':') ; }
   ;

stock_eqn : 
   lhs '=' var '+' exprlist {$$ = dpyy_addstockeq($1,$3,$5,'=') ; }
   ;




lhs : 
   var  { $$ = dpyy_addexceptinterp($1,NULL,0) ; }
   ;

var : 
	DPTT_symbol { $$ = dpyy_var_expression($1,NULL);}
	| DPTT_symbol sublist { $$ = dpyy_var_expression($1,$2) ;}
	;

sublist :
	'[' symlist ']' {$$ = $2 ;}
	;

symlist :
	DPTT_symbol { $$ = dpyy_symlist(NULL,$1,0,NULL) ; }
	| DPTT_symbol '!' { $$ = dpyy_symlist(NULL,$1,1,NULL) ; }
	| symlist ',' DPTT_symbol { $$ = dpyy_symlist($1,$3,0,NULL) ;}
	| symlist ',' DPTT_symbol '!' { $$ = dpyy_symlist($1,$3,1,NULL) ;}
	;
subdef :
	DPTT_symbol { $$ = dpyy_symlist(NULL,$1,0,NULL) ; }
	| '(' DPTT_symbol '-' DPTT_symbol ')' {$$ = dpyy_symlist(NULL,$2,0,$4) ;}
	| subdef ',' DPTT_symbol { $$ = dpyy_symlist($1,$3,0,NULL) ; }
	| subdef ',' '(' DPTT_symbol '-' DPTT_symbol ')' {$$ = dpyy_symlist($1,$4,0,$6) ; }
	;

number :
	DPTT_number {$$ = $1 ; }
	| '-' DPTT_number {$$ = -$2 ;}
	| '+' DPTT_number {$$ = $2 ;}
	;


maplist :
    { $$ = NULL ; }
	;

   // number lists can use ; to end a line
exprlist :
   exp {$$ = dpyy_chain_exprlist(NULL,$1) ;}
   | exprlist ',' exp {$$ = dpyy_chain_exprlist($1,$3) ; }
   | exprlist ';' exp {$$ = dpyy_chain_exprlist($1,$3) ; }
   | exprlist ';' {$$ = $1 ; }
   ;
    
exp:
      DPTT_number         { $$ = dpyy_num_expression($1) ; } /* since we allow unary - number not used here */
     | var                { $$ = (Expression *)$1 ; } /* ExpressionVariable is subclassed from Expression */
	 | var '(' exprlist ')'    { $$ = dpyy_lookup_expression($1,$3) ; }
	 | '(' exp ')'        { $$ = dpyy_operator_expression('(',$2,NULL) ; }
     | DPTT_function '(' exprlist ')'   { $$ = dpyy_function_expression($1,$3) ;}
     | DPTT_function '(' exprlist ',' ')'   { $$ = dpyy_function_expression($1,dpyy_chain_exprlist($3,dpyy_literal_expression("?"))) ;}
     | DPTT_function '(' ')'   { $$ = dpyy_function_expression($1,NULL) ;}
     | exp '+' exp        { $$ = dpyy_operator_expression('+',$1,$3) ; }
     | exp '-' exp        { $$ = dpyy_operator_expression('-',$1,$3) ; }
     | exp '*' exp        { $$ = dpyy_operator_expression('*',$1,$3) ; }
     | exp '/' exp        { $$ = dpyy_operator_expression('/',$1,$3) ; }
     | exp '<' exp        { $$ = dpyy_operator_expression('<',$1,$3) ; }
     | exp DPTT_le exp    { $$ = dpyy_operator_expression(DPTT_le,$1,$3) ; }
     | exp '>' exp        { $$ = dpyy_operator_expression('>',$1,$3) ; }
     | exp DPTT_ge exp    { $$ = dpyy_operator_expression(DPTT_ge,$1,$3) ; }
     | exp DPTT_ne exp    { $$ = dpyy_operator_expression(DPTT_ne,$1,$3) ; }
     | exp DPTT_or exp    { $$ = dpyy_operator_expression(DPTT_or,$1,$3) ; }
     | exp DPTT_and exp    { $$ = dpyy_operator_expression(DPTT_and,$1,$3) ; }
	 | DPTT_not exp		  { $$ = dpyy_operator_expression(DPTT_not,$2,NULL) ; }
     | exp '=' exp    { $$ = dpyy_operator_expression('=',$1,$3) ; }
     | '-' exp            { $$ = dpyy_operator_expression('-',NULL, $2) ; } /* unary plus - might be used by numbers */
     | '+' exp            { $$ = dpyy_operator_expression('+',NULL, $2) ; } /* unary plus - might be used by numbers */
     | exp '^' exp        { $$ = dpyy_operator_expression('^',$1,$3) ; }
     ;

tablevals : 
	tablepairs { $$ = $1 ; }
	| '[' '(' number ',' number ')' '-' '(' number ',' number ')' ']' ',' tablepairs 
	{ $$ = dpyy_tablerange($15,$3,$5,$9,$11) ; }
	| '[' '(' number ',' number ')' '-' '(' number ',' number ')' ',' tablepairs ']' ',' tablepairs 
	{ $$ = dpyy_tablerange($17,$3,$5,$9,$11) ; }
	;

	xytablevals :
	xytablevec { $$ = $1 ; }
	| '[' '(' number ',' number ')' '-' '(' number ',' number ')' ']' ',' xytablevec 
	{ $$ = dpyy_tablerange($15,$3,$5,$9,$11) ; }
	;

	xytablevec :
	number  { $$ = dpyy_tablevec(NULL,$1) ;}
	| xytablevec ',' number   {$$ = dpyy_tablevec($1,$3) ;}
	;

	
tablepairs :
	'(' number ',' number ')' { $$ = dpyy_tablepair(NULL,$2,$4) ;}
	| tablepairs ',' '(' number ',' number ')'  {$$ = dpyy_tablepair($1,$4,$6) ;}
	;



/* End of grammar.  */
%%
