Test Subscript Up-Dimensioning
==============================

When a variable with two dimensions of subscripting is created from a variable with only one dimension, the value of the one dimensional array should be broadcast to all values of the second dimension.

![Vensim screenshot](vensim_screenshot.png)

Such that when:

```
One Dim[Dim1]=
	1, 2, 3
	~	
	~		|

Two Dims[Dim1,Dim2]=
	One Dim[Dim1]
	~	
	~		|

```
The result may be:

```
One Dim[A]	1	
One Dim[B]	2	
One Dim[C]	3	

Two Dims[A,D]	1	
Two Dims[A,E]	1
Two Dims[B,D]	2
Two Dims[B,E]	2
Two Dims[C,D]	3
Two Dims[C,E]	3
```

Contributions
-------------

| Component                         | Author          | Contact                    | Date    | Software Version        |
|:--------------------------------- |:--------------- |:-------------------------- |:------- |:----------------------- |
| `test_subscript_updimensioning.mdl`     | James Houghton  | james.p.houghton@gmail.com | 6/16/16 | Vensim DSS 6.3E for Mac |
| `output.tab`                      | James Houghton  | james.p.houghton@gmail.com | 6/16/16 | Vensim DSS 6.3E for Mac |
