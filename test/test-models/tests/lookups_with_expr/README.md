Test Inline Lookups
===================

This tests the vensim syntax for WITH LOOKUP definition as they are created inline with an expression as input.

Lookup with expression as Input = WITH LOOKUP (
	1+SIN(2*3.14*Time/30),
		([(0,0)-(2,10),(-1,10),(-0.743119,5.78947),(-0.400612,3.07018),(-0.217125,1.79825),\
		(0.0214067,1.00877),(0.327217,0.394737),(1,0)],(0,10),(0.165138,5.17544),(0.269113,\
		3.85965),(0.397554,2.67544),(0.75841,0.921053),(1.08869,0.307018),(2,0) ))
	~	 [0,10]
	~		|

![test_lookups Vensim screenshot](vensim_screenshot.png)



Contributions
-------------

| Component                         | Author          | Contact                    | Date    | Software Version            |
|:--------------------------------- |:--------------- |:-------------------------- |:------- |:--------------------------- |
| test_lookups_with_expr.mdl        | Manuel Ruh      | manuelrugue@gmail.com      | 10/4/17 | Vensim PLE 7.1  for Windows |
| output.csv                        | Manuel Ruh      | manuelrugue@gmail.com      | 10/4/17 | Vensim PLE 7.1  for Windows |

