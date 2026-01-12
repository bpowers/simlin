Test Reference Capitalization
============================

This model tests case sensitivity to references to other variables:

```
Capitalized Variable=
	5
	~	
	~		|

lowercase reference=
	capitalized variable
	~	
	~		|
	
```
note the wrong-case reference to `capitalized variable`. This should parse correctly, but may not.

I had to create this test manually, as my copy of Vensim automatically fixed the capitalization issue.



![test_logicals Vensim screenshot](vensim_screenshot.png)



Contributions
-------------

| Component                          | Author          | Contact                    | Date    | Software Version        |
|:---------------------------------- |:--------------- |:-------------------------- |:------- |:----------------------- |
| `test_reference_capitalization.mdl` | James Houghton  | james.p.houghton@gmail.com | 2/4/16  | Vensim DSS 6.3E for Mac |
| `output.tab`                       | James Houghton  | james.p.houghton@gmail.com | 2/4/16  | Vensim DSS 6.3E for Mac |


TODO
----
- xmile, stella models