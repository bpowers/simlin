Test subscript subrange equality
================================

There are a number of different features that make up the full functionality we know as 
'subscripts'. We'll break them all into separate tests to ease the development effort.

This tests the case where a subscript subrange is defined to be equal to the subscript range itself. For example:

```
Dim1:
	Entry 1, Entry 2, Entry 3
	~	
	~		|
	
Subrange of Dim1:
	Entry 1, Entry 2, Entry 3
	~	
	~		|
```	

This type of construct could cause an interpreter to mix up whether it is looking at one range or the other.


![Vensim screenshot](vensim_screenshot.png)


Contributions
-------------

| Component                         | Author          | Contact                    | Date     | Software Version        |
|:--------------------------------- |:--------------- |:-------------------------- |:-------  |:----------------------- |
| test_subscript_subrange_equal.mdl | James Houghton  | James.p.houghton@gmail.com | 20160201 |:Vensim DSS for Mac 6.3E |
| output.tab                        | James Houghton  | James.p.houghton@gmail.com | 20160201 |:Vensim DSS for Mac 6.3E |
