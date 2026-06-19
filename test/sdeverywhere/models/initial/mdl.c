/*(Tue May 03 16:23:57 2016) From initial.mdl - C equations for the model */
#include "simext.c"
static COMPREAL temp0,temp1,temp2,temp3,temp4,temp5,temp6,temp7,temp8
,temp9,temp10,temp11,temp12,temp13,temp14,temp15,temp16,temp17,temp18
,temp19,temp20,temp21,temp22,temp23,temp24,temp25,temp26,temp27,temp28
,temp29,temp30,temp31 ;
static int sumind0,forind0 ; 
static int sumind1,forind1 ; 
static int sumind2,forind2 ; 
static int sumind3,forind3 ; 
static int sumind4,forind4 ; 
static int sumind5,forind5 ; 
static int sumind6,forind6 ; 
static int sumind7,forind7 ; 
static int simultid ;
#ifndef LINKEXTERN
#endif
unsigned char *mdl_desc()
{
return("(Tue May 03 16:23:57 2016) From initial.mdl") ;
}

/* compute the model rates */
void mdl_func0()
{double temp[10];
VGV->RATE[0] = 1.0 ;/* this is time */
} /* comp_rate */

/* compute the delays */
void mdl_func1()
{double temp[10];
} /* comp_delay */

/* initialize time */
void mdl_func2()
{double temp[10];
vec_arglist_init();
VGV->LEVEL[0] = VGV->LEVEL[3] ;
} /* init_time */

/* initialize time step */
void mdl_func3()
{double temp[10];
/* a constant no need to do anything */
} /* init_tstep */

/* State variable initial value computation*/
void mdl_func4()
{double temp[10];
/* Time */
 {
  VGV->lastpos = 0 ;
  VGV->LEVEL[0] = VGV->LEVEL[3] ;
}
/* x */
 {
  VGV->lastpos = 9 ;
  VGV->LEVEL[9] = VGV->LEVEL[1]*COS(6.280000*VGV->LEVEL[0]/VGV->LEVEL[5
]) ;
}
/* INITIAL x */
 {
  VGV->lastpos = 4 ;
  VGV->LEVEL[4] = VGV->LEVEL[9] ;
}
} /* comp_init */

/* State variable re-initial value computation*/
void mdl_func5()
{double temp[10];
} /* comp_reinit */

/*  Active Time Step Equation */
void mdl_func6()
{double temp[10];
} /* comp_tstep */
/*  Auxiliary variable equations*/
void mdl_func7()
{double temp[10];
/* x */
 {
  VGV->lastpos = 9 ;
  VGV->LEVEL[9] = VGV->LEVEL[1]*COS(6.280000*VGV->LEVEL[0]/VGV->LEVEL[5
]) ;
}
/* relative x */
 {
  VGV->lastpos = 6 ;
  VGV->LEVEL[6] = VGV->LEVEL[9]/VGV->LEVEL[4] ;
}
/* SAVEPER */
 {
  VGV->lastpos = 7 ;
  VGV->LEVEL[7] = VGV->LEVEL[8] ;
}
} /* comp_aux */
int execute_curloop() {return(0);}
static void vec_arglist_init()
{
}
void VEFCC comp_rate(void)
{
mdl_func0();
}

void VEFCC comp_delay(void)
{
mdl_func1();
}

void VEFCC init_time(void)
{
mdl_func2();
}

void VEFCC init_tstep(void)
{
mdl_func3();
}

void VEFCC comp_init(void)
{
mdl_func4();
}

void VEFCC comp_reinit(void)
{
mdl_func5();
}

void VEFCC comp_tstep(void)
{
mdl_func6();
}

void VEFCC comp_aux(void)
{
mdl_func7();
}

