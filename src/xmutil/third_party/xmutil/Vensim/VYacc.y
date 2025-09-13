/* 
c:\tools\bison\bin\win_bison -o VYacc.tab.cpp -p vpyy -d VYacc.y


c:\tools\bison\bin\win_bison -o $(ProjectDir)src\Vensim\VYacc.tab.cpp -p vpyy -d $(ProjectDir)src\Vensim\VYacc.y
Converting VYacc.y
Outputs: VYacc.tab.cpp VYacc.tab.hpp 
*/

%{
#include "../Log.h"
#include "../Symbol/Parse.h"
#include "VensimParseFunctions.h"
extern int vpyylex (void);
extern void vpyyerror (char const *);
#define YYSTYPE ParseUnion
#define YYFPRINTF XmutilLogf
%}
     
/* tokens returned by the tokenizer (in addition to single char tokens) */
%token <tok> VPTT_dataequals
%token <tok> VPTT_with_lookup
%token <tok> VPTT_map
%token <tok> VPTT_equiv
%token <tok> VPTT_groupstar
%token <tok> VPTT_and
%token <tok> VPTT_macro
%token <tok> VPTT_end_of_macro
%token <tok> VPTT_or
%token <tok> VPTT_not
%token <tok> VPTT_hold_backward
%token <tok> VPTT_look_forward
%token <tok> VPTT_except
%token <tok> VPTT_na
%token <tok> VPTT_interpolate
%token <tok> VPTT_raw
%token <tok> VPTT_test_input
%token <tok> VPTT_the_condition
%token <tok> VPTT_implies
%token <tok> VPTT_ge
%token <tok> VPTT_le
%token <tok> VPTT_ne
%token <exn> VPTT_tabbed_array
%token <tol> VPTT_eqend /* the end of equations  */

/* tokens with types returned by the tokenizer
   for these the tokenizer needs to set yylval.var
   and so on before returning the token ID  */
%token <num> VPTT_number
%token <lit> VPTT_literal
%token <sym> VPTT_symbol
%token <uni> VPTT_units_symbol
%token <fnc> VPTT_function


/* types for the different productions - note that there are quite  a few
   of these and that reductions in addition to creating or reusing an object
   also delete the component object to cleaning up is done as the 
   parsing progresses */
%type <exn> exp 
%type <tbl> tablepairs tablevals xytablevals xytablevec
%type <exl> exprlist
%type <lhs> lhs
%type <eqn> eqn
%type <uni> units unitsrange
%type <fqn> fulleq
%type <var> var
%type <sml> symlist mapsymlist sublist subdef maplist
%type <sll> exceptlist
%type <tok> '%' '|' interpmode
%type <num> number urangenum
%type <tok> macrostart macroend /* not really used - just sets/unsets a flag */


/* precedence - low to high */     
%left '-' '+'
%left VPTT_or
%left '=' '<' '>' VPTT_le VPTT_ge VPTT_ne
%left VPTT_and
%left '*' '/'
%left VPTT_not
%right '^'      /* exponentiation */

%% /* The grammar follows.  */


fulleq :
	VPTT_eqend { return VPTT_eqend ; } /* finished no more to do */
	| VPTT_groupstar { return VPTT_groupstar ; } /* process the group name elsewhere */
	| macrostart		  { return '|'; } /* sets the context for equations that follow */
	| macroend			  {return '|'; } /* back to regular equations */
	| eqn '~' unitsrange '~' /* comment follows */{vpyy_addfulleq($1,$3) ; return '~' ; }
	| eqn '~' unitsrange '|' /* comment skipped */ {vpyy_addfulleq($1,$3) ; return '|' ; }
	| eqn '~' '~' /* units skipped */{vpyy_addfulleq($1,NULL) ; return '~' ;}
	| eqn '~' '|' /* units, comment skipped */ {vpyy_addfulleq($1,NULL) ; return '|' ;}
	;

macrostart:
	VPTT_macro { vpyy_macro_start(); } VPTT_symbol '(' exprlist ')'   { vpyy_macro_expression($3,$5) ;}
	;

macroend:
   VPTT_end_of_macro { $$ = $1; vpyy_macro_end(); }
   ;




eqn : 
   lhs '=' exprlist {$$ = vpyy_addeq($1,NULL,$3,'=') ; }
   | lhs '=' {$$ = vpyy_addeq($1,NULL,NULL,'=') ; } // treat as A_FUNCTION_OF nothing
   | lhs '(' tablevals ')' { $$ = vpyy_add_lookup($1,NULL,$3, 0) ; }
   | lhs '(' xytablevals ')' { $$ = vpyy_add_lookup($1,NULL,$3, 1) ; }
   | lhs '=' VPTT_with_lookup '(' exp ',' '(' tablevals ')' ')' { $$ = vpyy_add_lookup($1,$5,$8, 0) ; }
   | lhs VPTT_dataequals exp {$$ = vpyy_addeq($1,$3,NULL,VPTT_dataequals) ; }
   | lhs { $$ = vpyy_add_lookup($1,NULL,NULL, 0) ; } // treat as if a lookup on time - don't have numbers
   | VPTT_symbol ':' subdef maplist {$$ = vpyy_addeq(vpyy_addexceptinterp(vpyy_var_expression($1,NULL),NULL,0),(Expression *)vpyy_symlist_expression($3,$4),NULL,':') ; }
   | VPTT_symbol VPTT_equiv VPTT_symbol  {$$ = vpyy_addeq(vpyy_addexceptinterp(vpyy_var_expression($1,NULL),NULL,0),(Expression *)vpyy_symlist_expression(vpyy_symlist(NULL,$3,0,NULL),NULL),NULL,VPTT_equiv) ; }
   | lhs '=' VPTT_tabbed_array { $$ = vpyy_addeq($1,$3,NULL,'=') ; }
   ;


lhs : 
   var  { $$ = vpyy_addexceptinterp($1,NULL,0) ; }
   | var exceptlist  {$$ = vpyy_addexceptinterp($1,$2,0) ;}
   | var interpmode {$$ = vpyy_addexceptinterp($1,NULL,$2) ;}
   ;

var : 
	VPTT_symbol { $$ = vpyy_var_expression($1,NULL);}
	| VPTT_symbol sublist { $$ = vpyy_var_expression($1,$2) ;}
	;

sublist :
	'[' symlist ']' {$$ = $2 ;}
	;

symlist :
	VPTT_symbol { $$ = vpyy_symlist(NULL,$1,0,NULL) ; }
	| VPTT_symbol '!' { $$ = vpyy_symlist(NULL,$1,1,NULL) ; }
	| symlist ',' VPTT_symbol { $$ = vpyy_symlist($1,$3,0,NULL) ;}
	| symlist ',' VPTT_symbol '!' { $$ = vpyy_symlist($1,$3,1,NULL) ;}
	;
subdef :
	VPTT_symbol { $$ = vpyy_symlist(NULL,$1,0,NULL) ; }
	| '(' VPTT_symbol '-' VPTT_symbol ')' {$$ = vpyy_symlist(NULL,$2,0,$4) ;}
	| subdef ',' VPTT_symbol { $$ = vpyy_symlist($1,$3,0,NULL) ; }
	| subdef ',' '(' VPTT_symbol '-' VPTT_symbol ')' {$$ = vpyy_symlist($1,$4,0,$6) ; }
	;

unitsrange : 
	units { $$ = $1 ; }
	| units '[' urangenum ',' urangenum ']' { $$ = vpyy_unitsrange($1,$3,$5,-1) ; }
	| units '[' urangenum ',' urangenum ',' urangenum ']' { $$ = vpyy_unitsrange($1,$3,$5,$7) ; }
	| '[' urangenum ',' urangenum ']' { $$ = vpyy_unitsrange(NULL,$2,$4,-1) ; }
	| '[' urangenum ',' urangenum ',' urangenum ']' { $$ = vpyy_unitsrange(NULL,$2,$4,$6) ; }
	;

urangenum :
	number {$$ = $1 ; }
	| '?' {$$ = -1e30 ; }
	;
number :
	VPTT_number {$$ = $1 ; }
	| '-' VPTT_number {$$ = -$2 ;}
	| '+' VPTT_number {$$ = $2 ;}
	;

units :
	VPTT_units_symbol { $$ = $1 ; }
	| units '/' units {$$ = vpyy_unitsdiv($1,$3);}
	| units '*' units {$$ = vpyy_unitsmult($1,$3);}
	| '(' units ')' { $$ = $2 ; } /* don't record */
	;


interpmode :
    VPTT_interpolate { $$ = $1 ; }
	| VPTT_raw { $$ = $1 ; }
	| VPTT_hold_backward { $$ = $1 ; }
	| VPTT_look_forward { $$ = $1 ; }
	;

exceptlist :
    VPTT_except sublist { $$ = vpyy_chain_sublist(NULL,$2) ; }
	| exceptlist ',' sublist { vpyy_chain_sublist($1,$3) ; $$ = $1 ; }
	;

mapsymlist :
	VPTT_symbol { $$ = vpyy_symlist(NULL,$1,0,NULL) ; }
	| '(' VPTT_symbol ':' symlist ')' { $$ = vpyy_mapsymlist(NULL, $2, $4); }
	| mapsymlist ',' VPTT_symbol { $$ = vpyy_symlist($1,$3,0,NULL) ;}
	| mapsymlist ',' '(' VPTT_symbol ':' symlist ')' { $$ = vpyy_mapsymlist($1, $4, $6);}
	;


maplist :
    { $$ = NULL ; }
	| VPTT_map mapsymlist { $$ =  $2 ; }
	;

   // number lists can use ; to end a line
exprlist :
   exp {$$ = vpyy_chain_exprlist(NULL,$1) ;}
   | exprlist ',' exp {$$ = vpyy_chain_exprlist($1,$3) ; }
   | exprlist ';' exp {$$ = vpyy_chain_exprlist($1,$3) ; }
   | exprlist ';' {$$ = $1 ; }
   ;
    
exp:
      VPTT_number         { $$ = vpyy_num_expression($1) ; } /* since we allow unary - number not used here */
	 | VPTT_na			  { $$ = vpyy_num_expression(-1E38);}
     | var                { $$ = (Expression *)$1 ; } /* ExpressionVariable is subclassed from Expression */
	 | VPTT_literal       { $$ = vpyy_literal_expression($1) ; } // not part of XMILE - just dumped directly for editing afterward
	 | var '(' exprlist ')'    { $$ = vpyy_lookup_expression($1,$3) ; }
	 | '(' exp ')'        { $$ = vpyy_operator_expression('(',$2,NULL) ; }
     | VPTT_function '(' exprlist ')'   { $$ = vpyy_function_expression($1,$3) ;}
     | VPTT_function '(' exprlist ',' ')'   { $$ = vpyy_function_expression($1,vpyy_chain_exprlist($3,vpyy_literal_expression("?"))) ;}
     | VPTT_function '(' ')'   { $$ = vpyy_function_expression($1,NULL) ;}
     | exp '+' exp        { $$ = vpyy_operator_expression('+',$1,$3) ; }
     | exp '-' exp        { $$ = vpyy_operator_expression('-',$1,$3) ; }
     | exp '*' exp        { $$ = vpyy_operator_expression('*',$1,$3) ; }
     | exp '/' exp        { $$ = vpyy_operator_expression('/',$1,$3) ; }
     | exp '<' exp        { $$ = vpyy_operator_expression('<',$1,$3) ; }
     | exp VPTT_le exp    { $$ = vpyy_operator_expression(VPTT_le,$1,$3) ; }
     | exp '>' exp        { $$ = vpyy_operator_expression('>',$1,$3) ; }
     | exp VPTT_ge exp    { $$ = vpyy_operator_expression(VPTT_ge,$1,$3) ; }
     | exp VPTT_ne exp    { $$ = vpyy_operator_expression(VPTT_ne,$1,$3) ; }
     | exp VPTT_or exp    { $$ = vpyy_operator_expression(VPTT_or,$1,$3) ; }
     | exp VPTT_and exp    { $$ = vpyy_operator_expression(VPTT_and,$1,$3) ; }
	 | VPTT_not exp		  { $$ = vpyy_operator_expression(VPTT_not,$2,NULL) ; }
     | exp '=' exp    { $$ = vpyy_operator_expression('=',$1,$3) ; }
     | '-' exp            { $$ = vpyy_operator_expression('-',NULL, $2) ; } /* unary plus - might be used by numbers */
     | '+' exp            { $$ = vpyy_operator_expression('+',NULL, $2) ; } /* unary plus - might be used by numbers */
     | exp '^' exp        { $$ = vpyy_operator_expression('^',$1,$3) ; }
     ;

tablevals : 
	tablepairs { $$ = $1 ; }
	| '[' '(' number ',' number ')' '-' '(' number ',' number ')' ']' ',' tablepairs 
	{ $$ = vpyy_tablerange($15,$3,$5,$9,$11) ; }
	| '[' '(' number ',' number ')' '-' '(' number ',' number ')' ',' tablepairs ']' ',' tablepairs 
	{ $$ = vpyy_tablerange($17,$3,$5,$9,$11) ; }
	;

	xytablevals :
	xytablevec { $$ = $1 ; }
	| '[' '(' number ',' number ')' '-' '(' number ',' number ')' ']' ',' xytablevec 
	{ $$ = vpyy_tablerange($15,$3,$5,$9,$11) ; }
	;

	xytablevec :
	number  { $$ = vpyy_tablevec(NULL,$1) ;}
	| xytablevec ',' number   {$$ = vpyy_tablevec($1,$3) ;}
	;

	
tablepairs :
	'(' number ',' number ')' { $$ = vpyy_tablepair(NULL,$2,$4) ;}
	| tablepairs ',' '(' number ',' number ')'  {$$ = vpyy_tablepair($1,$4,$6) ;}
	;



/* End of grammar.  */
%%
